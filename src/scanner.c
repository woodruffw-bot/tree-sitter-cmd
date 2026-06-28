#include "tree_sitter/parser.h"

#include <wctype.h>

// External tokens for tree-sitter-cmd.
//
// CONCAT is a zero-width token that lets the grammar join adjacent word
// fragments (bare text, quoted strings, caret escapes, %VAR% / !VAR!
// expansions, ...) into a single argument *only* when there is no whitespace
// between them. This is the standard technique used by tree-sitter-bash: a
// word like `C:\%ROOT%\bin` is one argument, while `a b` is two.
//
// The scanner is otherwise stateless.

enum TokenType {
  CONCAT,
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

bool tree_sitter_cmd_external_scanner_scan(void *payload, TSLexer *lexer,
                                           const bool *valid_symbols) {
  (void)payload;

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
