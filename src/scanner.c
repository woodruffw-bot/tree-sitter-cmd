#include "tree_sitter/parser.h"

#include <stdlib.h>
#include <string.h>

// External tokens for tree-sitter-cmd.
//
//   CONCAT       - zero-width join of adjacent word fragments (bare text,
//                  quoted strings, caret escapes, %VAR%/!VAR!, ...) when there
//                  is no whitespace between them (the tree-sitter-bash trick).
//   REM          - the `rem` comment keyword (whole-word; tree-sitter declines
//                  to keyword-extract it).
//   BLOCK_OPEN   - `(` that opens a command block or FOR set (structural).
//   BLOCK_CLOSE  - `)` that closes a structural block / FOR set.
//   LPAREN/RPAREN- a literal `(` / `)` appearing in an argument or IF operand.
//   CARET_ESCAPE - a lone `^` that escapes a following `%`/`!` expansion. In cmd
//                  `^%VAR%` expands `%VAR%` first and the caret escapes the
//                  result, so the caret must not swallow the `%`/`!`; we emit it
//                  as a one-char token (the grammar's `escape_sequence` handles
//                  `^X` for any other X, and `^`-newline is line continuation).
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
  BLOCK_OPEN,
  BLOCK_CLOSE,
  LPAREN,
  RPAREN,
  CARET_ESCAPE,
};

typedef struct {
  uint32_t depth; // number of currently-open structural blocks
} Scanner;

void *tree_sitter_cmd_external_scanner_create(void) {
  Scanner *s = calloc(1, sizeof(Scanner));
  return s;
}

void tree_sitter_cmd_external_scanner_destroy(void *payload) { free(payload); }

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

// A character that terminates a word: whitespace, newlines, command operators
// and parentheses. `=` is also a boundary so CONCAT cannot starve `==` /
// `name=value`.
static bool is_word_boundary(int32_t c) {
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
    case '=':
      return true;
    default:
      return false;
  }
}

// A character that may continue an identifier/command word; `rem` is only a
// comment keyword when not directly followed by one of these.
static bool is_ident_char(int32_t c) {
  return (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') ||
         (c >= '0' && c <= '9') || c == '_';
}

static void skip_ws(TSLexer *lexer) {
  while (lexer->lookahead == ' ' || lexer->lookahead == '\t') {
    lexer->advance(lexer, true);
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
  if (lexer->eof(lexer) || !is_ident_char(lexer->lookahead)) {
    lexer->result_symbol = REM;
    return true;
  }
  return false;
}

bool tree_sitter_cmd_external_scanner_scan(void *payload, TSLexer *lexer,
                                           const bool *valid_symbols) {
  Scanner *s = payload;

  // CONCAT: adjacency only, no whitespace skipping.
  if (valid_symbols[CONCAT] && !lexer->eof(lexer) &&
      !is_word_boundary(lexer->lookahead)) {
    lexer->result_symbol = CONCAT;
    return true;
  }

  bool want_rem = valid_symbols[REM];
  bool want_caret = valid_symbols[CARET_ESCAPE];
  bool want_paren = valid_symbols[BLOCK_OPEN] || valid_symbols[BLOCK_CLOSE] ||
                    valid_symbols[LPAREN] || valid_symbols[RPAREN];
  if (!want_rem && !want_caret && !want_paren) {
    return false;
  }

  skip_ws(lexer);
  if (lexer->eof(lexer)) {
    return false;
  }

  int32_t c = lexer->lookahead;

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
