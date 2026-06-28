#include "tree_sitter/parser.h"

// External tokens for tree-sitter-cmd.
//
//   CONCAT - a zero-width token that joins adjacent word fragments (bare text,
//            quoted strings, caret escapes, %VAR% / !VAR! expansions, ...) into
//            a single argument *only* when there is no whitespace between them.
//            This is the standard tree-sitter-bash technique: `C:\%ROOT%\bin`
//            is one argument, while `a b` is two.
//
//   REM    - the `rem` comment keyword, emitted only when `rem` (any case) is
//            followed by a word boundary. tree-sitter's built-in keyword
//            extraction handles `if`/`for`/`set`/... but declines to extract
//            `rem`, so without this `rem comment` would parse as a command named
//            `rem`. The boundary check keeps `remote` a command.
//
// The scanner is otherwise stateless.

enum TokenType {
  CONCAT,
  REM,
};

void *tree_sitter_cmd_external_scanner_create(void) { return NULL; }

void tree_sitter_cmd_external_scanner_destroy(void *payload) { (void)payload; }

unsigned tree_sitter_cmd_external_scanner_serialize(void *payload, char *buffer) {
  (void)payload;
  (void)buffer;
  return 0;
}

void tree_sitter_cmd_external_scanner_deserialize(void *payload,
                                                  const char *buffer,
                                                  unsigned length) {
  (void)payload;
  (void)buffer;
  (void)length;
}

// Characters that terminate a word: whitespace, newlines, the command
// operators, parentheses and `=`. `=` is a separator in cmd and, crucially,
// must not be joined onto the previous fragment or the zero-width CONCAT token
// would starve the `==` / `name=value` operators (external tokens take lexing
// priority). Plain text still includes `=` within a single token, so only
// fragment-to-fragment joins across `=` are affected. Everything else
// (letters, digits, `"`, `%`, `!`, `^`, `:`, `,`, `;`, path separators, ...)
// continues the current word.
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

// A character that may continue an identifier/command word. `rem` is only a
// comment keyword when it is *not* immediately followed by one of these (so
// `remote`, `rem2` stay commands but `rem foo`, `rem.`, `rem&x`, bare `rem`
// are comments).
static bool is_ident_char(int32_t c) {
  return (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') ||
         (c >= '0' && c <= '9') || c == '_';
}

static bool scan_rem(TSLexer *lexer) {
  // The external scanner runs before leading whitespace is skipped, so skip it
  // here (an indented `rem` inside a block must still be recognised).
  while (lexer->lookahead == ' ' || lexer->lookahead == '\t') {
    lexer->advance(lexer, true);
  }
  int32_t c = lexer->lookahead;
  if (c != 'r' && c != 'R') {
    return false;
  }
  lexer->advance(lexer, false);
  c = lexer->lookahead;
  if (c != 'e' && c != 'E') {
    return false;
  }
  lexer->advance(lexer, false);
  c = lexer->lookahead;
  if (c != 'm' && c != 'M') {
    return false;
  }
  lexer->advance(lexer, false);
  // The token ends after `rem`; the boundary character is not consumed.
  lexer->mark_end(lexer);
  if (lexer->eof(lexer) || !is_ident_char(lexer->lookahead)) {
    lexer->result_symbol = REM;
    return true;
  }
  return false;
}

bool tree_sitter_cmd_external_scanner_scan(void *payload, TSLexer *lexer,
                                           const bool *valid_symbols) {
  (void)payload;

  // Recognise the REM comment keyword at a statement boundary. CONCAT is never
  // valid at that point, so this also keeps us out of error-recovery states
  // (where every symbol is marked valid).
  if (valid_symbols[REM] && !valid_symbols[CONCAT]) {
    return scan_rem(lexer);
  }

  if (!valid_symbols[CONCAT]) {
    return false;
  }

  // At end of input there is nothing to join onto.
  if (lexer->eof(lexer)) {
    return false;
  }

  // Join the next fragment only when it is directly adjacent (no intervening
  // whitespace) and is not the start of an operator/paren that ends the word.
  if (is_word_boundary(lexer->lookahead)) {
    return false;
  }

  lexer->result_symbol = CONCAT;
  // Zero-width: do not advance the lexer.
  return true;
}
