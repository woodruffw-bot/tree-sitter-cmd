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
//   IF_ATTACHED_OPERAND
//                - zero-width selection of the RHS word attached to `==`.
//   IF_FLAG / IF_NOT / IF_CONDITION_KEYWORD
//                - IF control words followed by a standard source separator.
//   REDIRECT_CONCAT
//                - adjacent filename fragments; standard separators end the
//                  target, while an attached opening parenthesis is literal.
//   REM          - the `rem` comment keyword (whole-word; tree-sitter declines
//                  to keyword-extract it).
//   REM_TEXT     - the opaque body of a REM comment through end of line.
//   ESCAPED_REDIRECT_DIGIT
//                - a digit before redirection that follows escaped ordinary
//                  text and remains part of that word.
//   REDIRECT_SOURCE
//                - a source file descriptor digit immediately followed by a
//                  redirection operator.
//   EMPTY_BLOCK_OPEN
//                - `(` that opens a command block whose next non-whitespace
//                  byte is its closing `)`.
//   BLOCK_OPEN   - any other `(` that opens a command block or FOR set.
//   BLOCK_CLOSE  - `)` that closes a structural block / FOR set.
//   LPAREN/RPAREN- a literal `(` / `)` appearing in an argument or IF operand.
//   CARET_ESCAPE - the source caret that escapes a following `%`/`!`
//                  expansion. It covers either `^` or a continued `^` plus
//                  newline, but not the sigil. Keeping the sigil outside this
//                  token preserves the following expansion node.
//   STRING_END   - the terminator of a double-quoted string. Inside a string the
//                  grammar offers this token; we consume a closing `"`, or match
//                  zero-width at end of line / end of input so an unterminated
//                  quote still closes (cmd runs an open quote to end of line).
//   DELAYED_QUOTE_TEXT
//                - source text for a delayed reference that opens a quote in a
//                  caret-SET payload or filename, including its quoted suffix.
//   SET_STRING_START
//                - opening SET wrapper quote; resets its outer quote phase.
//   SET_INNER_QUOTE / SET_STRING_END
//                - quotes inside a quoted SET binding and its last wrapper
//                  quote. Like STRING_END, SET_STRING_END also matches
//                  zero-width at end of line / end of input.
//   SET_NAME_TEXT / SET_NAME_TAIL_TEXT
//                - quoted name fragments that honor the outer quote phase.
//   SET_VALUE_TEXT
//                - a quoted SET value fragment, stopping at outer operators.
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
//   BLOCK_BODY_BOUNDARY
//                - a zero-width marker immediately before a block close.
//   COMMAND_START
//                - deliberately declined after BODY_BOUNDARY, so Tree-sitter
//                  records a genuine anonymous MISSING command for the body.
//   ERROR_SENTINEL - unused final token that detects Tree-sitter's all-symbol
//                    error-recovery state. The scanner declines in that state so
//                    zero-width CONCAT / STANDARD_CONCAT / REDIRECT_CONCAT /
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
  IF_ATTACHED_OPERAND,
  IF_FLAG,
  IF_NOT,
  IF_CONDITION_KEYWORD,
  KEYWORD_BOUNDARY,
  REDIRECT_CONCAT,
  REM,
  REM_TEXT,
  REDIRECT_SOURCE,
  ESCAPED_REDIRECT_DIGIT,
  EMPTY_BLOCK_OPEN,
  BLOCK_OPEN,
  BLOCK_CLOSE,
  LPAREN,
  RPAREN,
  CARET_ESCAPE,
  DELAYED_VARIABLE,
  DELAYED_QUOTE_TEXT,
  STRING_END,
  SET_STRING_START,
  SET_INNER_QUOTE,
  SET_STRING_END,
  SET_NAME_TEXT,
  SET_NAME_TAIL_TEXT,
  SET_VALUE_TEXT,
  SET_IGNORED_SUFFIX,
  LABEL_LEADING_SPACE,
  SET_BINDING_END,
  BODY_BOUNDARY,
  BODY_BOUNDARY_AGAIN,
  BLOCK_BODY_BOUNDARY,
  COMMAND_START,
  ERROR_SENTINEL,
};

typedef struct {
  uint32_t depth; // number of currently-open structural blocks
  uint8_t body_boundaries;
  bool set_in_quote;
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
  buffer[sizeof(s->depth) + 1] = (char)s->set_in_quote;
  return sizeof(s->depth) + 2;
}

void tree_sitter_cmd_external_scanner_deserialize(void *payload,
                                                  const char *buffer,
                                                  unsigned length) {
  Scanner *s = payload;
  s->depth = 0;
  s->body_boundaries = 0;
  s->set_in_quote = false;
  if (length >= sizeof(s->depth)) {
    memcpy(&s->depth, buffer, sizeof(s->depth));
  }
  if (length > sizeof(s->depth)) {
    s->body_boundaries = (uint8_t)buffer[sizeof(s->depth)];
  }
  if (length > sizeof(s->depth) + 1) {
    s->set_in_quote = buffer[sizeof(s->depth) + 1] != 0;
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

// An opening parenthesis immediately following an argument fragment remains
// part of that argument even while a structural block is open. Command names
// use STANDARD_CONCAT instead, so keeping this specific to CONCAT preserves
// command-position idioms such as `(echo()`.
static bool is_argument_concat_boundary(const Scanner *s, int32_t c) {
  return c != '(' && is_word_boundary(s, c);
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

// Track only quote lookahead. This state is local to one scanner call.
typedef struct {
  enum { SET_SUFFIX_TEXT, SET_REDIRECT_OPERATOR, SET_REDIRECT_START,
         SET_REDIRECT_TEXT } phase;
  bool quoted;
  bool append;
  bool duplicate;
  unsigned escape;
  bool done;
  bool found_quote;
} SetQuoteLookahead;

static void advance_set_quote_lookahead(const Scanner *s, SetQuoteLookahead *look,
                                        int32_t c) {
  if (look->done || look->found_quote) return;
  if (look->escape) {
    // A caret continuation in a filename also protects the next character.
    if (look->phase == SET_REDIRECT_TEXT) {
      look->escape = look->escape == 1 && c == '\r' ? 2
                   : look->escape < 3 && c == '\n' ? 3 : 0;
      return;
    }
    look->escape = 0;
    if (c != '\r' && c != '\n') return;
  }
  if (c == '\r' || c == '\n') {
    look->done = true;
    return;
  }
  for (;;) {
    switch (look->phase) {
      case SET_SUFFIX_TEXT:
        if (c == '"') {
          look->found_quote = true;
        } else if (!look->quoted && (c == '<' || c == '>')) {
          look->phase = SET_REDIRECT_OPERATOR;
          look->append = c == '>';
          look->duplicate = false;
        } else if (!look->quoted && is_set_boundary(s, c)) {
          look->done = true;
        } else if (!look->quoted && c == '^') {
          look->escape = 1;
        }
        return;
      case SET_REDIRECT_OPERATOR:
        if (c == '>' && look->append) {
          look->append = false;
          return;
        }
        look->phase = SET_REDIRECT_START;
        if (c == '&') {
          look->duplicate = true;
          return;
        }
        break;
      case SET_REDIRECT_START:
        if (c == ' ' || c == '\t' || c == ',' || c == ';' || c == '=') return;
        if (look->duplicate && c >= '0' && c <= '9') {
          look->phase = SET_SUFFIX_TEXT;
          return;
        }
        look->phase = SET_REDIRECT_TEXT;
        break;
      case SET_REDIRECT_TEXT:
        if (!look->quoted && (is_set_boundary(s, c) || c == ' ' || c == '\t' ||
                              c == ',' || c == ';' || c == '=')) {
          look->phase = SET_SUFFIX_TEXT;
          break;
        }
        if (c == '"') look->quoted = !look->quoted;
        if (c == '^' && !look->quoted) look->escape = 1;
        return;
    }
  }
}

// Quotes in a removed redirect target cannot close SET. Percent variables are
// opaque here, matching the grammar's variable token. Keep the literal reading
// in parallel until a closing percent confirms the token, since an unmatched
// percent must not hide a later filename boundary or wrapper quote.
static bool has_later_set_quote(const Scanner *s, TSLexer *lexer, bool quoted) {
  SetQuoteLookahead look = {.quoted = quoted};
  SetQuoteLookahead literal = {0};
  bool percent_dup_target = false;
  enum { NO_PERCENT, PERCENT_START, PERCENT_VARIABLE } percent = NO_PERCENT;
  while (!lexer->eof(lexer)) {
    int32_t c = lexer->lookahead;
    lexer->advance(lexer, false);
    if (percent != NO_PERCENT) {
      advance_set_quote_lookahead(s, &literal, c);
      if (percent == PERCENT_START &&
          (c == '%' || (c >= '0' && c <= '9') || c == '~' || c == '*' ||
           c == '\r' || c == '\n')) {
        look = literal;
        if (percent_dup_target && c >= '0' && c <= '9') {
          look.phase = SET_SUFFIX_TEXT;
        }
        percent = NO_PERCENT;
      } else if (percent == PERCENT_VARIABLE && c == '%') {
        // Duplication consumes one expansion, unlike a filename word.
        if (percent_dup_target) look.phase = SET_SUFFIX_TEXT;
        percent = NO_PERCENT;
      } else if (c == '\r' || c == '\n') {
        look = literal;
        percent = NO_PERCENT;
      } else {
        percent = PERCENT_VARIABLE;
        continue;
      }
    } else {
      bool starts_dup_target = look.phase == SET_REDIRECT_START && look.duplicate;
      advance_set_quote_lookahead(s, &look, c);
      if (c == '%' && look.phase == SET_REDIRECT_TEXT) {
        literal = look;
        percent_dup_target = starts_dup_target;
        percent = PERCENT_START;
      }
    }
    if (look.found_quote || look.done) return look.found_quote;
  }
  return percent == NO_PERCENT ? look.found_quote : literal.found_quote;
}

static bool is_standard_word_boundary(const Scanner *s, int32_t c) {
  return is_word_boundary(s, c) || c == ',' || c == ';';
}

// Delayed expansion happens after outer CMD tokenization. A pair of bangs
// cannot hide an active operator or structural block close. Quotes can occur
// in substitution payloads, but still toggle protection of outer operators.
static bool scan_delayed_variable(Scanner *s, TSLexer *lexer,
                                  bool quoted, bool set_context,
                                  bool conservative_quote_context) {
  lexer->advance(lexer, false);
  bool has_content = false;
  while (!lexer->eof(lexer)) {
    int32_t c = lexer->lookahead;
    if (c == '\r' || c == '\n') return false;
    if (c == '!') {
      if (!has_content) return false;
      lexer->advance(lexer, false);
      // A delayed reference can open a quote in an otherwise unquoted
      // fragment. Keep that reference and its protected suffix as source text
      // through the quote's close. Splitting at the bangs would hide the quote
      // change or pair later bangs with the wrong reference.
      if (conservative_quote_context && quoted) {
        while (!lexer->eof(lexer) && lexer->lookahead != '\r' &&
               lexer->lookahead != '\n') {
          int32_t suffix = lexer->lookahead;
          lexer->advance(lexer, false);
          if (suffix == '"') break;
        }
        lexer->mark_end(lexer);
        lexer->result_symbol = DELAYED_QUOTE_TEXT;
        return true;
      }
      lexer->mark_end(lexer);
      if (set_context) s->set_in_quote = quoted;
      lexer->result_symbol = DELAYED_VARIABLE;
      return true;
    }
    if (c == '"') {
      quoted = !quoted;
      has_content = true;
      lexer->advance(lexer, false);
      continue;
    }
    if (!quoted) {
      if (c == '&' || c == '|' || c == '<' || c == '>' ||
          (c == ')' && s->depth > 0)) return false;
      if (c == '^') {
        lexer->advance(lexer, false);
        if (lexer->lookahead == '\r') {
          lexer->advance(lexer, false);
          if (lexer->lookahead != '\n') return false;
        }
        if (lexer->lookahead == '\n') lexer->advance(lexer, false);
        if (lexer->eof(lexer)) return false;
      }
    }
    has_content = true;
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

// Reject a keyword before emitting it when another operand fragment follows.
// Checking only after an internal keyword token would lose that prefix from
// operands such as not"x", exist%N%, or /i^x.
static bool scan_if_keyword(TSLexer *lexer, const bool *valid_symbols) {
  char word[sizeof("cmdextversion")];
  size_t length = 0;
  while (length < sizeof(word) - 1) {
    int32_t c = lexer->lookahead;
    if (c >= 'A' && c <= 'Z') c += 'a' - 'A';
    if ((c < 'a' || c > 'z') && c != '/') break;
    word[length++] = (char)c;
    lexer->advance(lexer, false);
  }
  word[length] = '\0';
  int32_t c = lexer->lookahead;
  if (c != ' ' && c != '\t' && c != ',' && c != ';' && c != '=') return false;

  if (valid_symbols[IF_FLAG] && strcmp(word, "/i") == 0) {
    lexer->result_symbol = IF_FLAG;
  } else if (valid_symbols[IF_NOT] && strcmp(word, "not") == 0) {
    lexer->result_symbol = IF_NOT;
  } else if (valid_symbols[IF_CONDITION_KEYWORD] &&
             (strcmp(word, "exist") == 0 || strcmp(word, "defined") == 0 ||
              strcmp(word, "errorlevel") == 0 || strcmp(word, "cmdextversion") == 0)) {
    lexer->result_symbol = IF_CONDITION_KEYWORD;
  } else {
    return false;
  }
  lexer->mark_end(lexer);
  return true;
}

// Classify a command-position `(` before returning its source-width token.
// EMPTY_BLOCK_OPEN is selected only when whitespace leads directly to `)`;
// otherwise the same byte remains an ordinary BLOCK_OPEN.
static bool scan_empty_or_block_open(Scanner *s, TSLexer *lexer,
                                     const bool *valid_symbols) {
  lexer->advance(lexer, false);
  lexer->mark_end(lexer);
  while (true) {
    while (lexer->lookahead == ' ' || lexer->lookahead == '\t') {
      lexer->advance(lexer, false);
    }
    if (lexer->lookahead == '\n') {
      lexer->advance(lexer, false);
      continue;
    }
    if (lexer->lookahead == '\r') {
      lexer->advance(lexer, false);
      if (lexer->lookahead == '\n') {
        lexer->advance(lexer, false);
        continue;
      }
    }
    break;
  }
  if (lexer->lookahead != ')' && !valid_symbols[BLOCK_OPEN]) return false;

  s->depth++;
  lexer->result_symbol =
      lexer->lookahead == ')' ? EMPTY_BLOCK_OPEN : BLOCK_OPEN;
  return true;
}

bool tree_sitter_cmd_external_scanner_scan(void *payload, TSLexer *lexer,
                                           const bool *valid_symbols) {
  Scanner *s = payload;
  bool skipped_body_space = false;

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

  // Check immediately after an ordinary escape, before any branch can skip
  // whitespace. A separated digit may begin the next command's redirection.
  if (valid_symbols[ESCAPED_REDIRECT_DIGIT] &&
      lexer->lookahead >= '0' && lexer->lookahead <= '9') {
    lexer->mark_end(lexer);
    lexer->advance(lexer, false);
    if (lexer->lookahead == '<' || lexer->lookahead == '>') {
      lexer->mark_end(lexer);
      lexer->result_symbol = ESCAPED_REDIRECT_DIGIT;
      return true;
    }
    if (valid_symbols[REDIRECT_CONCAT] || valid_symbols[STANDARD_CONCAT] ||
        valid_symbols[CONCAT]) {
      lexer->result_symbol = valid_symbols[REDIRECT_CONCAT] ? REDIRECT_CONCAT
                           : valid_symbols[STANDARD_CONCAT] ? STANDARD_CONCAT
                           : CONCAT;
      return true;
    }
    return false;
  }

  // ParseIf retains equals signs in the token attached to `==`, but skips
  // standard separators when fetching a separate operand. Do not skip extras
  // here: the source gap decides which word rule applies.
  if (valid_symbols[IF_ATTACHED_OPERAND]) {
    int32_t c = lexer->lookahead;
    if (!lexer->eof(lexer) && c != ' ' && c != '\t' && c != '\r' &&
        c != '\n' && c != ',' && c != ';' && c != '&' && c != '|' &&
        c != '<' && c != '>') {
      lexer->result_symbol = IF_ATTACHED_OPERAND;
      return true;
    }
    return false;
  }

  // ELSE, IN, and DO are compared against whole fetched tokens. A following quote,
  // expansion, caret, or opening parenthesis still belongs to that token.
  // Keep EOF/newline available so a bare control keyword retains its error.
  if (valid_symbols[KEYWORD_BOUNDARY]) {
    int32_t c = lexer->lookahead;
    if (lexer->eof(lexer) || (c != '(' && is_standard_word_boundary(s, c))) {
      lexer->result_symbol = KEYWORD_BOUNDARY;
      return true;
    }
    return false;
  }

  if (valid_symbols[BLOCK_BODY_BOUNDARY]) {
    while (lexer->lookahead == ' ' || lexer->lookahead == '\t') {
      lexer->advance(lexer, true);
    }
    if (s->depth > 0 && lexer->lookahead == ')') {
      lexer->mark_end(lexer);
      lexer->result_symbol = BLOCK_BODY_BOUNDARY;
      return true;
    }
  }

  if ((valid_symbols[BODY_BOUNDARY] || valid_symbols[BODY_BOUNDARY_AGAIN]) &&
      s->body_boundaries < 2) {
    if (s->body_boundaries == 0 && !valid_symbols[BODY_BOUNDARY]) return false;
    enum TokenType boundary_symbol =
        s->body_boundaries == 1 && valid_symbols[BODY_BOUNDARY_AGAIN]
            ? BODY_BOUNDARY_AGAIN
            : BODY_BOUNDARY;
    while (lexer->lookahead == ' ' || lexer->lookahead == '\t') {
      lexer->advance(lexer, true);
      skipped_body_space = true;
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
    if (!skipped_body_space && valid_symbols[STANDARD_CONCAT] &&
        !is_standard_word_boundary(s, lexer->lookahead)) {
      lexer->result_symbol = STANDARD_CONCAT;
      return true;
    }
    if (!skipped_body_space && valid_symbols[CONCAT] &&
        !is_argument_concat_boundary(s, lexer->lookahead)) {
      lexer->result_symbol = CONCAT;
      return true;
    }

    // Usually decline here so the internal lexer can distinguish the final IF
    // operand from the following command. Continue only for body starts whose
    // external tokens must be considered in this same scanner call.
    int32_t c = lexer->lookahead;
    if (c == '(' && skipped_body_space && valid_symbols[EMPTY_BLOCK_OPEN]) {
      return scan_empty_or_block_open(s, lexer, valid_symbols);
    }
    if (c == '(' && skipped_body_space && valid_symbols[BLOCK_OPEN]) {
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
    if (c != '(' && c != ')' && c != '^' && c != '!' && c != 'r' && c != 'R' &&
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



  // A descriptor after a quoted or expanded fragment competes with CONCAT.
  // Prefer the descriptor only when the digit is followed by `<` or `>`. If it
  // is not, return a zero-width CONCAT at the marked start of the token.
  // The body-boundary check may already have skipped whitespace. Keep that
  // source boundary for every adjacency check below.
  if (!skipped_body_space && valid_symbols[REDIRECT_SOURCE] &&
      (valid_symbols[CONCAT] || valid_symbols[STANDARD_CONCAT] ||
       valid_symbols[REDIRECT_CONCAT]) &&
      lexer->lookahead >= '0' && lexer->lookahead <= '9') {
    lexer->mark_end(lexer);
    lexer->advance(lexer, false);
    if (lexer->lookahead == '<' || lexer->lookahead == '>') {
      lexer->mark_end(lexer);
      lexer->result_symbol = REDIRECT_SOURCE;
    } else {
      lexer->result_symbol = valid_symbols[REDIRECT_CONCAT] ? REDIRECT_CONCAT
                           : valid_symbols[STANDARD_CONCAT] ? STANDARD_CONCAT
                           : CONCAT;
    }
    return true;
  }

  if (!skipped_body_space && valid_symbols[REDIRECT_CONCAT] &&
      !lexer->eof(lexer) &&
      !is_argument_concat_boundary(s, lexer->lookahead) &&
      lexer->lookahead != ',' && lexer->lookahead != ';') {
    lexer->result_symbol = REDIRECT_CONCAT;
    return true;
  }

  // STANDARD_CONCAT: the same adjacency rule in a standard-separator slot.
  if (!skipped_body_space && valid_symbols[STANDARD_CONCAT] &&
      !lexer->eof(lexer) &&
      !is_standard_word_boundary(s, lexer->lookahead)) {
    lexer->result_symbol = STANDARD_CONCAT;
    return true;
  }

  // CONCAT: adjacency only, no whitespace skipping.
  if (!skipped_body_space && valid_symbols[CONCAT] && !lexer->eof(lexer) &&
      !is_argument_concat_boundary(s, lexer->lookahead)) {
    lexer->result_symbol = CONCAT;
    return true;
  }

  if (valid_symbols[DELAYED_VARIABLE] && lexer->lookahead == '!') {
    bool set_name_context = valid_symbols[SET_NAME_TEXT] ||
                            valid_symbols[SET_NAME_TAIL_TEXT];
    bool set_context = valid_symbols[SET_STRING_END] || set_name_context;
    return scan_delayed_variable(s, lexer,
        valid_symbols[STRING_END] || (set_context && s->set_in_quote),
        set_context,
        valid_symbols[DELAYED_QUOTE_TEXT]);
  }

  // Name text is quoted initially, but expansions can expose outer syntax.
  // Keep the leading-space and post-expansion ranges distinct as before.
  if (valid_symbols[SET_NAME_TEXT] || valid_symbols[SET_NAME_TAIL_TEXT]) {
    bool tail = valid_symbols[SET_NAME_TAIL_TEXT];
    bool has_content = false;
    if (!tail && (lexer->lookahead == ' ' || lexer->lookahead == '\t')) {
      return false;
    }
    while (!lexer->eof(lexer)) {
      int32_t c = lexer->lookahead;
      if (c == '"' || c == '%' || c == '!' || c == '=' || c == '\r' ||
          c == '\n' || (!s->set_in_quote && is_set_boundary(s, c))) break;
      lexer->advance(lexer, false);
      if (c == '^' && !s->set_in_quote && !lexer->eof(lexer)) {
        if (lexer->lookahead == '\r') lexer->advance(lexer, false);
        if (lexer->lookahead == '\n') lexer->advance(lexer, false);
        if (!lexer->eof(lexer) && lexer->lookahead != '%' && lexer->lookahead != '!') {
          lexer->advance(lexer, false);
        }
      }
      has_content = true;
    }
    if (has_content) {
      lexer->mark_end(lexer);
      lexer->result_symbol = tail ? SET_NAME_TAIL_TEXT : SET_NAME_TEXT;
      return true;
    }
  }

  if (valid_symbols[SET_STRING_START]) {
    skip_ws(lexer);
    if (lexer->lookahead == '"') {
      lexer->advance(lexer, false);
      s->set_in_quote = true;
      lexer->result_symbol = SET_STRING_START;
      return true;
    }
  }

  // SET_INNER_QUOTE / SET_STRING_END: cmd uses the last quote in a quoted SET
  // binding as the wrapper close. Earlier quotes are literal value fragments.
  // Track the outer tokenizer's quote phase: operators are boundaries only
  // outside quotes, even though SET later treats earlier quotes as value text.
  if (valid_symbols[SET_INNER_QUOTE] || valid_symbols[SET_STRING_END]) {
    if (lexer->eof(lexer)) {
      if (valid_symbols[SET_STRING_END]) {
        s->set_in_quote = false;
        lexer->result_symbol = SET_STRING_END;
        return true;
      }
      return false;
    }
    int32_t la = lexer->lookahead;
    // A delayed name reference may already contain the closing source quote.
    // End that SET at the next active boundary without inserting a quote.
    if (la == '\r' || la == '\n' ||
        (!s->set_in_quote &&
         (valid_symbols[SET_NAME_TEXT] || valid_symbols[SET_NAME_TAIL_TEXT]) &&
         is_set_boundary(s, la))) {
      if (valid_symbols[SET_STRING_END]) {
        s->set_in_quote = false;
        lexer->result_symbol = SET_STRING_END;
        return true;
      }
      return false;
    }
    if (la == '"') {
      lexer->advance(lexer, false);
      lexer->mark_end(lexer);

      bool quoted = !s->set_in_quote;
      bool has_later_quote = has_later_set_quote(s, lexer, quoted);

      if (has_later_quote && valid_symbols[SET_INNER_QUOTE]) {
        s->set_in_quote = quoted;
        lexer->result_symbol = SET_INNER_QUOTE;
        return true;
      }
      if (!has_later_quote && valid_symbols[SET_STRING_END]) {
        s->set_in_quote = quoted;
        lexer->result_symbol = SET_STRING_END;
        return true;
      }
      return false;
    }
  }

  // Values use the outer quote phase for operators, but retain every quote
  // before SET's final wrapper quote as source text. Stop at active redirects
  // so the grammar can keep each surviving value segment in its own range.
  if (valid_symbols[SET_VALUE_TEXT]) {
    bool has_content = false;
    bool word_start = true;
    while (!lexer->eof(lexer)) {
      int32_t c = lexer->lookahead;
      if (c == '"' || c == '%' || c == '!' || c == '\r' || c == '\n' ||
          (!s->set_in_quote && is_set_boundary(s, c))) break;
      if (!s->set_in_quote && word_start && c >= '0' && c <= '9') {
        lexer->mark_end(lexer);
        lexer->advance(lexer, false);
        if (lexer->lookahead == '<' || lexer->lookahead == '>') {
          if (has_content) {
            lexer->result_symbol = SET_VALUE_TEXT;
            return true;
          }
          if (valid_symbols[REDIRECT_SOURCE]) {
            lexer->mark_end(lexer);
            lexer->result_symbol = REDIRECT_SOURCE;
            return true;
          }
          return false;
        }
      } else {
        lexer->advance(lexer, false);
      }
      if (c == '^' && !s->set_in_quote && !lexer->eof(lexer)) {
        if (lexer->lookahead == '\r') lexer->advance(lexer, false);
        if (lexer->lookahead == '\n') lexer->advance(lexer, false);
        if (!lexer->eof(lexer) && lexer->lookahead != '%' && lexer->lookahead != '!') {
          c = lexer->lookahead;
          lexer->advance(lexer, false);
        }
      }
      word_start = c == ' ' || c == '\t' || c == ',' || c == ';' || c == '=' ||
                   c == '(' || c == ')' || c == '&' || c == '|' || c == '"';
      has_content = true;
      lexer->mark_end(lexer);
    }
    if (has_content) {
      lexer->result_symbol = SET_VALUE_TEXT;
      return true;
    }
  }

  // Text after the final wrapper quote is still part of the source parameter,
  // but cmd discards it after finding that quote. Keep it in one opaque node so
  // analyzers neither attach it to the value nor mistake it for another
  // command. Whitespace alone remains an extra. Caret escapes keep an operator
  // or close parenthesis inside the ignored suffix.
  if (valid_symbols[SET_IGNORED_SUFFIX]) {
    bool has_content = false;
    while (!lexer->eof(lexer) && lexer->lookahead != '\r' &&
           lexer->lookahead != '\n' &&
           (s->set_in_quote || !is_set_boundary(s, lexer->lookahead))) {
      int32_t la = lexer->lookahead;
      if (la != ' ' && la != '\t') has_content = true;
      lexer->advance(lexer, false);
      if (la == '^' && !s->set_in_quote && !lexer->eof(lexer)) {
        if (lexer->lookahead == '\r') {
          lexer->advance(lexer, false);
          if (lexer->lookahead != '\n') break;
        }
        if (lexer->lookahead == '\n') lexer->advance(lexer, false);
        // The first character after a continuation is a forced literal too.
        // A physical CRLF represents one newline, as in escape_sequence.
        if (!lexer->eof(lexer)) {
          bool forced_cr = lexer->lookahead == '\r';
          lexer->advance(lexer, false);
          if (forced_cr && lexer->lookahead == '\n') lexer->advance(lexer, false);
        }
      }
    }
    if (has_content) {
      s->set_in_quote = false;
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
  bool want_delayed = valid_symbols[DELAYED_VARIABLE];
  bool want_if_keyword = valid_symbols[IF_FLAG] || valid_symbols[IF_NOT] ||
                         valid_symbols[IF_CONDITION_KEYWORD];
  bool want_paren = valid_symbols[EMPTY_BLOCK_OPEN] ||
                    valid_symbols[BLOCK_OPEN] || valid_symbols[BLOCK_CLOSE] ||
                    valid_symbols[LPAREN] || valid_symbols[RPAREN];
  if (!want_rem && !want_redirect_source && !want_caret && !want_delayed &&
      !want_if_keyword && !want_paren) {
    return false;
  }

  skip_ws(lexer);
  if (lexer->eof(lexer)) {
    return false;
  }

  int32_t c = lexer->lookahead;

  if (want_if_keyword && (c == '/' || (c >= 'a' && c <= 'z') ||
                         (c >= 'A' && c <= 'Z'))) {
    return scan_if_keyword(lexer, valid_symbols);
  }

  if (want_delayed && c == '!') {
    return scan_delayed_variable(s, lexer, false, false,
                                 valid_symbols[DELAYED_QUOTE_TEXT]);
  }

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

  // Keep a caret before `%`/`!` separate from the expansion. The caret may be
  // directly adjacent, or it may first escape a physical newline. In the
  // continued form, consume through the newline but leave the sigil for the
  // grammar's expansion token.
  if (want_caret && c == '^') {
    lexer->advance(lexer, false);
    lexer->mark_end(lexer);
    if (lexer->lookahead == '%' || lexer->lookahead == '!') {
      lexer->result_symbol = CARET_ESCAPE;
      return true;
    }

    if (lexer->lookahead == '\r') {
      lexer->advance(lexer, false);
      if (lexer->lookahead != '\n') return false;
    }
    if (lexer->lookahead == '\n') {
      lexer->advance(lexer, false);
      lexer->mark_end(lexer);
      if (lexer->lookahead == '%' || lexer->lookahead == '!') {
        lexer->result_symbol = CARET_ESCAPE;
        return true;
      }
    }
    return false;
  }

  if (want_rem && (c == 'r' || c == 'R')) {
    if (scan_rem(lexer)) return true;
    return false;
  }

  if (c == '(') {
    // Select the error-preserving empty-block branch before returning the
    // opening token. Looking ahead only across whitespace keeps this decision
    // local and leaves the token range on the source `(`.
    if (valid_symbols[EMPTY_BLOCK_OPEN]) {
      return scan_empty_or_block_open(s, lexer, valid_symbols);
    }

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
