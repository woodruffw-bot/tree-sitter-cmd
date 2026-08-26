#include "tree_sitter/parser.h"
#include "tree_sitter/alloc.h"

#include <string.h>

// External tokens for tree-sitter-cmd.
//
//   CONCAT       - zero-width join of adjacent word fragments (bare text,
//                  quoted strings, caret escapes, %VAR%/!VAR!, ...) when there
//                  is no whitespace between them (the tree-sitter-bash trick).
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
//   MISSING_STATEMENT
//                - a visible zero-width sentinel at an unescaped newline, EOF,
//                  or structural block close when an IF/ELSE/FOR body is
//                  expected.
//   ERROR_SENTINEL - unused final token that detects Tree-sitter's all-symbol
//                    error-recovery state. The scanner declines in that state so
//                    zero-width CONCAT / STRING_END tokens cannot stall recovery.
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
  MISSING_STATEMENT,
  ERROR_SENTINEL,
};

typedef struct {
  uint32_t depth; // number of currently-open structural blocks
} Scanner;

void *tree_sitter_cmd_external_scanner_create(void) {
  Scanner *s = ts_calloc(1, sizeof(Scanner));
  return s;
}

void tree_sitter_cmd_external_scanner_destroy(void *payload) { ts_free(payload); }

unsigned tree_sitter_cmd_external_scanner_serialize(void *payload, char *buffer) {
  Scanner *s = payload;
  memcpy(buffer, &s->depth, sizeof(s->depth));
  return sizeof(s->depth);
}

void tree_sitter_cmd_external_scanner_deserialize(void *payload,
                                                  const char *buffer,
                                                  unsigned length) {
  Scanner *s = payload;
  s->depth = 0;
  if (length >= sizeof(s->depth)) {
    memcpy(&s->depth, buffer, sizeof(s->depth));
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

  // A descriptor after a quoted or expanded fragment competes with CONCAT.
  // Prefer the descriptor only when the digit is followed by `<` or `>`. If it
  // is not, return a zero-width CONCAT at the marked start of the token.
  if (valid_symbols[REDIRECT_SOURCE] && valid_symbols[CONCAT] &&
      lexer->lookahead >= '0' && lexer->lookahead <= '9') {
    lexer->mark_end(lexer);
    lexer->advance(lexer, false);
    if (lexer->lookahead == '<' || lexer->lookahead == '>') {
      lexer->mark_end(lexer);
      lexer->result_symbol = REDIRECT_SOURCE;
    } else {
      lexer->result_symbol = CONCAT;
    }
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
  bool want_missing_statement = valid_symbols[MISSING_STATEMENT];
  bool want_paren = valid_symbols[BLOCK_OPEN] || valid_symbols[BLOCK_CLOSE] ||
                    valid_symbols[LPAREN] || valid_symbols[RPAREN];
  if (!want_rem && !want_redirect_source && !want_caret &&
      !want_missing_statement && !want_paren) {
    return false;
  }

  skip_ws(lexer);
  if (want_missing_statement &&
      (lexer->eof(lexer) || lexer->lookahead == '\r' ||
       lexer->lookahead == '\n' ||
       (s->depth > 0 && lexer->lookahead == ')'))) {
    lexer->result_symbol = MISSING_STATEMENT;
    return true;
  }
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
