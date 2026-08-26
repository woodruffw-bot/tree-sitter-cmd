#include "tree_sitter/parser.h"
#include "tree_sitter/alloc.h"

#include <string.h>

// External tokens for tree-sitter-cmd.
//
//   CONCAT       - zero-width join of adjacent word fragments (bare text,
//                  quoted strings, caret escapes, %VAR%/!VAR!, ...) when there
//                  is no whitespace between them (the tree-sitter-bash trick).
//   STANDARD_CONCAT
//                - the same join in parser slots where `,`, `;`, and `=` are
//                  token separators.
//   REDIRECT_TARGET_SEPARATOR_AHEAD
//                - zero-width selection of a separator-aware redirection
//                  target when standard punctuation occurs before its end.
//   REM          - the `rem` comment keyword (whole-word; tree-sitter declines
//                  to keyword-extract it).
//   REM_TEXT     - the opaque body of a REM comment through end of line.
//   REDIRECT_SOURCE
//                - a source file descriptor digit immediately followed by a
//                  redirection operator.
//   BLOCK_OPEN   - `(` that opens a command block or FOR set (structural).
//   BLOCK_CLOSE  - `)` that closes a structural block / FOR set.
//   LPAREN/RPAREN- a literal `(` / `)` appearing in an argument or IF operand.
//   CARET_ESCAPE - a lone `^` that escapes a following `%`/`!` expansion. In cmd
//                  `^%VAR%` expands `%VAR%` first and the caret escapes the
//                  result, so the caret must not swallow the `%`/`!`; we emit it
//                  as a one-char token (the grammar's `escape_sequence` handles
//                  `^X` for any other X, and `^`-newline is line continuation).
//   STRING_END   - the terminator of a double-quoted string. Inside a string the
//                  grammar offers this token; we consume a closing `"`, or match
//                  zero-width at end of line / end of input so an unterminated
//                  quote still closes (cmd runs an open quote to end of line).
//   SET_INNER_QUOTE / SET_STRING_END
//                - quotes inside a quoted SET binding and its last wrapper
//                  quote. Like STRING_END, SET_STRING_END also matches
//                  zero-width at end of line / end of input.
//   SET_IGNORED_SUFFIX
//                - opaque text after the last quote of a quoted SET binding.
//                  cmd discards this text when it truncates at the last quote.
//   LABEL_LEADING_SPACE
//                - horizontal space after a definition colon, emitted only
//                  when a valid label-name byte follows. This keeps a `:` line
//                  containing only whitespace on the colon-comment path.
//   SET_BINDING_END
//                - zero-width confirmation that the next non-horizontal byte
//                  after a redirected unquoted SET name is its `=` delimiter.
//   BODY_BOUNDARY
//                - zero-width marker at newline or EOF when an IF/ELSE/FOR body
//                  is absent. The grammar aliases it to an anonymous
//                  implementation terminal; its visibility to error recovery
//                  makes skipping the real boundary costlier than recording
//                  the required MISSING command.
//   BODY_BOUNDARY_AGAIN
//                - the second hidden marker at the same physical boundary.
//   COMMAND_START
//                - deliberately declined after BODY_BOUNDARY, so Tree-sitter
//                  records a genuine anonymous MISSING command for the body.
//   ERROR_SENTINEL - unused final token that detects Tree-sitter's all-symbol
//                    error-recovery state. The scanner declines in that state so
//                    zero-width CONCAT / STANDARD_CONCAT / target-lookahead /
//                    binding-end / STRING_END tokens cannot stall recovery.
//
// `cmd.exe` parentheses are context-sensitive: `(` is structural where a
// command/set is expected and literal in an argument; `)` closes a block only
// when one is open (depth > 0). This mirrors cmd's tokenizer (see ReactOS
// base/shell/cmd/parser.c): a `(` only begins a block at the start of a command
// token; a `(` appearing mid-argument is just a literal character and does NOT
// increase the block-nesting depth. Conversely, while inside a block, the first
// unescaped `)` always closes it — a literal `(` in an argument never protects a
// later `)`. (That is exactly why cmd requires `^)` to echo a close-paren inside
// a block.) So we track only a single depth counter of open *blocks*: `(` vs
// literal is chosen from `valid_symbols` (the grammar offers a block-open only
// where a command/set may begin), and a `)` is BLOCK_CLOSE whenever a block is
// open (preferred over a literal RPAREN), else a literal RPAREN at depth 0.

enum TokenType {
  CONCAT,
  STANDARD_CONCAT,
  REDIRECT_TARGET_SEPARATOR_AHEAD,
  REM,
  REM_TEXT,
  REDIRECT_SOURCE,
  BLOCK_OPEN,
  BLOCK_CLOSE,
  LPAREN,
  RPAREN,
  CARET_ESCAPE,
  STRING_END,
  SET_INNER_QUOTE,
  SET_STRING_END,
  SET_IGNORED_SUFFIX,
  LABEL_LEADING_SPACE,
  SET_BINDING_END,
  BODY_BOUNDARY,
  BODY_BOUNDARY_AGAIN,
  COMMAND_START,
  ERROR_SENTINEL,
};

typedef struct {
  uint32_t depth; // number of currently-open structural blocks
  uint8_t body_boundaries;
} Scanner;

void *tree_sitter_cmd_external_scanner_create(void) {
  Scanner *s = ts_calloc(1, sizeof(Scanner));
  return s;
}

void tree_sitter_cmd_external_scanner_destroy(void *payload) { ts_free(payload); }

unsigned tree_sitter_cmd_external_scanner_serialize(void *payload, char *buffer) {
  Scanner *s = payload;
  memcpy(buffer, &s->depth, sizeof(s->depth));
  buffer[sizeof(s->depth)] = (char)s->body_boundaries;
  return sizeof(s->depth) + 1;
}

void tree_sitter_cmd_external_scanner_deserialize(void *payload,
                                                  const char *buffer,
                                                  unsigned length) {
  Scanner *s = payload;
  s->depth = 0;
  s->body_boundaries = 0;
  if (length >= sizeof(s->depth)) {
    memcpy(&s->depth, buffer, sizeof(s->depth));
  }
  if (length > sizeof(s->depth)) {
    s->body_boundaries = (uint8_t)buffer[sizeof(s->depth)];
  }
}

// A character that terminates a word. At block depth zero, parentheses may be
// literal parts of an argument. Inside a block, a close parenthesis remains a
// boundary because it closes the innermost structural block.
static bool is_word_boundary(const Scanner *s, int32_t c) {
  switch (c) {
    case ' ':
    case '\t':
    case '\r':
    case '\n':
    case '&':
    case '|':
    case '<':
    case '>':
    case '=':
      return true;
    case '(':
    case ')':
      return s->depth > 0;
    default:
      return false;
  }
}

// A boundary for quoted SET parameters. A close parenthesis only ends the
// parameter while a structural block is open; at depth zero it is ordinary
// command text, like it is for generic arguments.
static bool is_set_boundary(const Scanner *s, int32_t c) {
  switch (c) {
    case '\r':
    case '\n':
    case '&':
    case '|':
    case '<':
    case '>':
      return true;
    case ')':
      return s->depth > 0;
    default:
      return false;
  }
}

static bool is_standard_word_boundary(const Scanner *s, int32_t c) {
  return is_word_boundary(s, c) || c == ',' || c == ';';
}

// Look ahead without consuming input. This selects the separator-aware target
// branch only when the current redirection filename contains a real standard
// separator. Separators inside quotes, caret escapes, and paired expansions
// stay part of the filename.
static bool redirect_target_has_separator(const Scanner *s, TSLexer *lexer) {
  lexer->mark_end(lexer);
  if (lexer->lookahead == ',' || lexer->lookahead == ';' ||
      lexer->lookahead == '=') {
    return false;
  }
  bool in_quote = false;

  while (!lexer->eof(lexer)) {
    int32_t c = lexer->lookahead;
    if (c == '\r' || c == '\n') return false;

    if (c == '^') {
      lexer->advance(lexer, false);
      if (!lexer->eof(lexer) && lexer->lookahead != '\r' &&
          lexer->lookahead != '\n') {
        lexer->advance(lexer, false);
      }
      continue;
    }

    if (c == '"') {
      in_quote = !in_quote;
      lexer->advance(lexer, false);
      continue;
    }

    if (!in_quote && (c == '%' || c == '!')) {
      int32_t sigil = c;
      bool saw_boundary = false;
      bool saw_separator = false;
      lexer->advance(lexer, false);
      while (!lexer->eof(lexer) && lexer->lookahead != '\r' &&
             lexer->lookahead != '\n') {
        c = lexer->lookahead;
        if (c == sigil) {
          lexer->advance(lexer, false);
          saw_separator = false;
          break;
        }
        if (!saw_boundary && (c == ',' || c == ';' || c == '=')) {
          saw_separator = true;
        }
        if (!saw_boundary && is_word_boundary(s, c)) saw_boundary = true;
        lexer->advance(lexer, false);
      }
      if (saw_separator) return true;
      continue;
    }

    if (!in_quote) {
      if (c == ',' || c == ';' || c == '=') return true;
      if (is_word_boundary(s, c)) return false;
    }
    lexer->advance(lexer, false);
  }
  return false;
}

// cmd recognizes REM at an internal-command separator. Other punctuation, such
// as the hyphen in `rem-foo`, continues the command name.
static bool is_rem_boundary(int32_t c) {
  switch (c) {
    case ' ':
    case '\t':
    case '\r':
    case '\n':
    case '&':
    case '|':
    case '<':
    case '>':
    case '(':
    case ')':
    case ':':
    case '.':
    case ',':
    case '/':
    case ';':
    case '=':
    case '[':
    case ']':
    case '\\':
      return true;
    default:
      return false;
  }
}

static void skip_ws(TSLexer *lexer) {
  while (lexer->lookahead == ' ' || lexer->lookahead == '\t') {
    lexer->advance(lexer, true);
  }
}

static bool is_label_name_start(int32_t c) {
  switch (c) {
    case 0:
    case ' ':
    case '\t':
    case '\r':
    case '\n':
    case ':':
    case '+':
    case ';':
    case ',':
    case '=':
    case '&':
    case '|':
    case '<':
    case '>':
    case '(':
    case ')':
      return false;
    default:
      return true;
  }
}

// Match `rem` (any case) followed by a word boundary. Leading whitespace is
// assumed already skipped.
static bool scan_rem(TSLexer *lexer) {
  int32_t c = lexer->lookahead;
  if (c != 'r' && c != 'R') return false;
  lexer->advance(lexer, false);
  c = lexer->lookahead;
  if (c != 'e' && c != 'E') return false;
  lexer->advance(lexer, false);
  c = lexer->lookahead;
  if (c != 'm' && c != 'M') return false;
  lexer->advance(lexer, false);
  lexer->mark_end(lexer);
  if (lexer->eof(lexer) || is_rem_boundary(lexer->lookahead)) {
    lexer->result_symbol = REM;
    return true;
  }
  return false;
}

bool tree_sitter_cmd_external_scanner_scan(void *payload, TSLexer *lexer,
                                           const bool *valid_symbols) {
  Scanner *s = payload;

  if (s->body_boundaries > 0 && !lexer->eof(lexer) &&
      lexer->lookahead != '\r' && lexer->lookahead != '\n') {
    s->body_boundaries = 0;
  }

  // During error recovery Tree-sitter makes every external symbol valid. Do not
  // emit zero-width tokens in that state. A real close parenthesis must still
  // close an open block so a malformed statement does not consume later lines.
  if (valid_symbols[ERROR_SENTINEL]) {
    skip_ws(lexer);
    if (s->depth > 0 && lexer->lookahead == ')') {
      lexer->advance(lexer, false);
      s->depth--;
      lexer->result_symbol = BLOCK_CLOSE;
      return true;
    }
    return false;
  }

  if ((valid_symbols[BODY_BOUNDARY] || valid_symbols[BODY_BOUNDARY_AGAIN]) &&
      s->body_boundaries < 2) {
    if (s->body_boundaries == 0 && !valid_symbols[BODY_BOUNDARY]) return false;
    enum TokenType boundary_symbol =
        s->body_boundaries == 1 && valid_symbols[BODY_BOUNDARY_AGAIN]
            ? BODY_BOUNDARY_AGAIN
            : BODY_BOUNDARY;
    bool skipped_space = false;
    while (lexer->lookahead == ' ' || lexer->lookahead == '\t') {
      lexer->advance(lexer, true);
      skipped_space = true;
    }
    if (lexer->lookahead == '\r') {
      lexer->mark_end(lexer);
      lexer->advance(lexer, false);
      if (lexer->lookahead == '\n') {
        s->body_boundaries = boundary_symbol == BODY_BOUNDARY_AGAIN
                                 ? 0
                                 : s->body_boundaries + 1;
        lexer->result_symbol = boundary_symbol;
        return true;
      }
      return false;
    }
    if (lexer->eof(lexer) || lexer->lookahead == '\n') {
      lexer->mark_end(lexer);
      s->body_boundaries = boundary_symbol == BODY_BOUNDARY_AGAIN
                               ? 0
                               : s->body_boundaries + 1;
      lexer->result_symbol = boundary_symbol;
      return true;
    }

    // The missing-body branch can be valid while the condition still has an
    // adjacent fragment. Preserve that adjacency before considering a body;
    // spacing means the operand has ended and must not synthesize a join.
    if (!skipped_space && valid_symbols[STANDARD_CONCAT] &&
        !is_standard_word_boundary(s, lexer->lookahead)) {
      lexer->result_symbol = STANDARD_CONCAT;
      return true;
    }
    if (!skipped_space && valid_symbols[CONCAT] &&
        !is_word_boundary(s, lexer->lookahead)) {
      lexer->result_symbol = CONCAT;
      return true;
    }

    // Usually decline here so the internal lexer can distinguish the final IF
    // operand from the following command. Continue only for body starts whose
    // external tokens must be considered in this same scanner call.
    int32_t c = lexer->lookahead;
    if (c == '(' && skipped_space && valid_symbols[BLOCK_OPEN]) {
      lexer->advance(lexer, false);
      lexer->mark_end(lexer);
      if (lexer->lookahead == ')' && valid_symbols[LPAREN]) {
        lexer->result_symbol = LPAREN;
        return true;
      }
      s->depth++;
      lexer->result_symbol = BLOCK_OPEN;
      return true;
    }
    if (c != '(' && c != ')' && c != '^' && c != 'r' && c != 'R' &&
        (c < '0' || c > '9')) {
      return false;
    }
  }

  // A label definition may ignore horizontal space after its colon, but only
  // when a real name follows. Looking ahead here prevents the higher-precedence
  // label rule from consuming a whitespace-only colon line and recovering a
  // missing name instead of using the colon-comment rule.
  if (valid_symbols[LABEL_LEADING_SPACE]) {
    bool has_space = false;
    while (lexer->lookahead == ' ' || lexer->lookahead == '\t') {
      lexer->advance(lexer, false);
      has_space = true;
    }
    if (has_space && !lexer->eof(lexer) &&
        is_label_name_start(lexer->lookahead)) {
      lexer->mark_end(lexer);
      lexer->result_symbol = LABEL_LEADING_SPACE;
      return true;
    }
    return false;
  }

  // A redirect may split an unquoted SET binding name. Confirm the final gap
  // only when an actual assignment delimiter follows; otherwise a display's
  // terminal redirect must remain on the statement rather than inducing a
  // recovered, missing `=`.
  if (valid_symbols[SET_BINDING_END]) {
    skip_ws(lexer);
    if (lexer->lookahead == '=') {
      lexer->mark_end(lexer);
      lexer->result_symbol = SET_BINDING_END;
      return true;
    }
    return false;
  }

  if (valid_symbols[REDIRECT_TARGET_SEPARATOR_AHEAD]) {
    if (redirect_target_has_separator(s, lexer)) {
      lexer->result_symbol = REDIRECT_TARGET_SEPARATOR_AHEAD;
      return true;
    }
    return false;
  }

  // A descriptor after a quoted or expanded fragment competes with CONCAT.
  // Prefer the descriptor only when the digit is followed by `<` or `>`. If it
  // is not, return a zero-width CONCAT at the marked start of the token.
  if (valid_symbols[REDIRECT_SOURCE] &&
      (valid_symbols[CONCAT] || valid_symbols[STANDARD_CONCAT]) &&
      lexer->lookahead >= '0' && lexer->lookahead <= '9') {
    lexer->mark_end(lexer);
    lexer->advance(lexer, false);
    if (lexer->lookahead == '<' || lexer->lookahead == '>') {
      lexer->mark_end(lexer);
      lexer->result_symbol = REDIRECT_SOURCE;
    } else {
      lexer->result_symbol = valid_symbols[STANDARD_CONCAT]
                                 ? STANDARD_CONCAT
                                 : CONCAT;
    }
    return true;
  }

  // STANDARD_CONCAT: the same adjacency rule in a standard-separator slot.
  if (valid_symbols[STANDARD_CONCAT] && !lexer->eof(lexer) &&
      !is_standard_word_boundary(s, lexer->lookahead)) {
    lexer->result_symbol = STANDARD_CONCAT;
    return true;
  }

  // CONCAT: adjacency only, no whitespace skipping.
  if (valid_symbols[CONCAT] && !lexer->eof(lexer) &&
      !is_word_boundary(s, lexer->lookahead)) {
    lexer->result_symbol = CONCAT;
    return true;
  }

  // SET_INNER_QUOTE / SET_STRING_END: cmd uses the last quote in a quoted SET
  // binding as the wrapper close. Earlier quotes are literal value fragments.
  // Stop lookahead at command operators so quotes in a later command do not
  // extend the SET binding.
  if (valid_symbols[SET_INNER_QUOTE] || valid_symbols[SET_STRING_END]) {
    if (lexer->eof(lexer)) {
      if (valid_symbols[SET_STRING_END]) {
        lexer->result_symbol = SET_STRING_END;
        return true;
      }
      return false;
    }
    int32_t la = lexer->lookahead;
    if (la == '\r' || la == '\n') {
      if (valid_symbols[SET_STRING_END]) {
        lexer->result_symbol = SET_STRING_END;
        return true;
      }
      return false;
    }
    if (la == '"') {
      lexer->advance(lexer, false);
      lexer->mark_end(lexer);

      bool has_later_quote = false;
      while (!lexer->eof(lexer)) {
        la = lexer->lookahead;
        if (is_set_boundary(s, la)) {
          break;
        }
        if (la == '^') {
          lexer->advance(lexer, false);
          if (!lexer->eof(lexer) && lexer->lookahead != '\r' &&
              lexer->lookahead != '\n') {
            lexer->advance(lexer, false);
          }
          continue;
        }
        if (la == '"') {
          has_later_quote = true;
          break;
        }
        lexer->advance(lexer, false);
      }

      if (has_later_quote && valid_symbols[SET_INNER_QUOTE]) {
        lexer->result_symbol = SET_INNER_QUOTE;
        return true;
      }
      if (!has_later_quote && valid_symbols[SET_STRING_END]) {
        lexer->result_symbol = SET_STRING_END;
        return true;
      }
      return false;
    }
  }

  // Text after the final wrapper quote is still part of the source parameter,
  // but cmd discards it after finding that quote. Keep it in one opaque node so
  // analyzers neither attach it to the value nor mistake it for another
  // command. Whitespace alone remains an extra. Caret escapes keep an operator
  // or close parenthesis inside the ignored suffix.
  if (valid_symbols[SET_IGNORED_SUFFIX]) {
    bool has_content = false;
    while (!lexer->eof(lexer) &&
           !is_set_boundary(s, lexer->lookahead)) {
      int32_t la = lexer->lookahead;
      if (la != ' ' && la != '\t') has_content = true;
      lexer->advance(lexer, false);
      if (la == '^' && !lexer->eof(lexer)) {
        if (lexer->lookahead == '\r') {
          lexer->advance(lexer, false);
          if (lexer->lookahead == '\n') {
            lexer->advance(lexer, false);
            continue;
          }
          break;
        }
        lexer->advance(lexer, false);
      }
    }
    if (has_content) {
      lexer->mark_end(lexer);
      lexer->result_symbol = SET_IGNORED_SUFFIX;
      return true;
    }
    return false;
  }

  // STRING_END: terminate a double-quoted string. Checked before whitespace
  // skipping because an interior space is string text, not a separator. Consume
  // a closing `"`, or match zero-width at a newline / end of input so an
  // unterminated quote still closes. Any other character is left for the
  // interior string-part tokens.
  if (valid_symbols[STRING_END]) {
    if (lexer->eof(lexer)) {
      lexer->result_symbol = STRING_END;
      return true;
    }
    int32_t la = lexer->lookahead;
    if (la == '"') {
      lexer->advance(lexer, false);
      lexer->result_symbol = STRING_END;
      return true;
    }
    if (la == '\r' || la == '\n') {
      lexer->result_symbol = STRING_END;
      return true;
    }
    return false;
  }

  // REM text is opaque. Consume it before operator and parenthesis tokens can
  // compete with a one-character comment body.
  if (valid_symbols[REM_TEXT]) {
    skip_ws(lexer);
    if (lexer->eof(lexer) || lexer->lookahead == '\r' ||
        lexer->lookahead == '\n') {
      return false;
    }
    while (!lexer->eof(lexer) && lexer->lookahead != '\r' &&
           lexer->lookahead != '\n') {
      lexer->advance(lexer, false);
    }
    lexer->mark_end(lexer);
    lexer->result_symbol = REM_TEXT;
    return true;
  }

  bool want_rem = valid_symbols[REM];
  bool want_redirect_source = valid_symbols[REDIRECT_SOURCE];
  bool want_caret = valid_symbols[CARET_ESCAPE];
  bool want_paren = valid_symbols[BLOCK_OPEN] || valid_symbols[BLOCK_CLOSE] ||
                    valid_symbols[LPAREN] || valid_symbols[RPAREN];
  if (!want_rem && !want_redirect_source && !want_caret && !want_paren) {
    return false;
  }

  skip_ws(lexer);
  if (lexer->eof(lexer)) {
    return false;
  }

  int32_t c = lexer->lookahead;

  // A source file descriptor is one digit directly adjacent to `<` or `>`.
  // Looking ahead here avoids stealing ordinary numeric arguments such as the
  // `2` in `echo 2 >file` or the `22` in `echo 22>file`.
  if (want_redirect_source && c >= '0' && c <= '9') {
    lexer->advance(lexer, false);
    lexer->mark_end(lexer);
    if (lexer->lookahead == '<' || lexer->lookahead == '>') {
      lexer->result_symbol = REDIRECT_SOURCE;
      return true;
    }
    return false;
  }

  // A lone caret escaping a following `%`/`!` expansion: consume just the `^`
  // (so `%VAR%` / `!VAR!` is still recognised as its own token). Any other `^X`
  // is left to the grammar's `escape_sequence`, and `^`-newline to the line
  // continuation, so decline unless the very next char is `%` or `!`.
  if (want_caret && c == '^') {
    lexer->advance(lexer, false);
    lexer->mark_end(lexer);
    if (lexer->lookahead == '%' || lexer->lookahead == '!') {
      lexer->result_symbol = CARET_ESCAPE;
      return true;
    }
    return false;
  }

  if (want_rem && (c == 'r' || c == 'R')) {
    if (scan_rem(lexer)) return true;
    return false;
  }

  if (c == '(') {
    // A block-open only where the grammar expects a command/set to begin;
    // otherwise the `(` is a literal paren that does not nest.
    if (valid_symbols[BLOCK_OPEN]) {
      lexer->advance(lexer, false);
      s->depth++;
      lexer->result_symbol = BLOCK_OPEN;
      return true;
    }
    if (valid_symbols[LPAREN]) {
      lexer->advance(lexer, false);
      lexer->result_symbol = LPAREN;
      return true;
    }
    return false;
  }

  if (c == ')') {
    // While a block is open, the first unescaped `)` closes it — prefer
    // BLOCK_CLOSE over a literal RPAREN so literal `(` in arguments never
    // protect a later `)` inside a block (matching cmd).
    if (s->depth > 0 && valid_symbols[BLOCK_CLOSE]) {
      lexer->advance(lexer, false);
      s->depth--;
      lexer->result_symbol = BLOCK_CLOSE;
      return true;
    }
    // Otherwise (depth 0, or a state that only admits a literal here) a `)` is
    // literal text and does not change the block depth.
    if (valid_symbols[RPAREN]) {
      lexer->advance(lexer, false);
      lexer->result_symbol = RPAREN;
      return true;
    }
    return false;
  }

  return false;
}
