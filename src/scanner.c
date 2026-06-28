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
//
// `cmd.exe` parentheses are context-sensitive: `(` is structural where a
// command/set is expected and literal in an argument; `)` closes a block only
// when one is open (depth > 0) — and a literal `(` in an argument must protect
// its matching `)` even when nested inside a block. We mirror cmd by tracking a
// stack of open parens, recording for each whether it is a structural block or
// a literal paren, and emitting BLOCK_CLOSE vs RPAREN according to the stack
// top. `(` vs literal is chosen from `valid_symbols` (the grammar only allows a
// block-open where a command/set may begin).

enum TokenType {
  CONCAT,
  REM,
  BLOCK_OPEN,
  BLOCK_CLOSE,
  LPAREN,
  RPAREN,
};

// Up to 64 levels of paren nesting are tracked precisely (a bit per level:
// 1 = structural block, 0 = literal). Deeper nesting degrades to "block",
// which real scripts never reach.
typedef struct {
  uint32_t depth;
  uint64_t kinds;
} Scanner;

void *tree_sitter_cmd_external_scanner_create(void) {
  Scanner *s = calloc(1, sizeof(Scanner));
  return s;
}

void tree_sitter_cmd_external_scanner_destroy(void *payload) { free(payload); }

unsigned tree_sitter_cmd_external_scanner_serialize(void *payload, char *buffer) {
  Scanner *s = payload;
  unsigned n = 0;
  memcpy(buffer + n, &s->depth, sizeof(s->depth));
  n += sizeof(s->depth);
  memcpy(buffer + n, &s->kinds, sizeof(s->kinds));
  n += sizeof(s->kinds);
  return n;
}

void tree_sitter_cmd_external_scanner_deserialize(void *payload,
                                                  const char *buffer,
                                                  unsigned length) {
  Scanner *s = payload;
  s->depth = 0;
  s->kinds = 0;
  if (length >= sizeof(s->depth) + sizeof(s->kinds)) {
    unsigned n = 0;
    memcpy(&s->depth, buffer + n, sizeof(s->depth));
    n += sizeof(s->depth);
    memcpy(&s->kinds, buffer + n, sizeof(s->kinds));
  }
}

static void push_paren(Scanner *s, bool is_block) {
  if (s->depth < 64) {
    if (is_block) {
      s->kinds |= (uint64_t)1 << s->depth;
    } else {
      s->kinds &= ~((uint64_t)1 << s->depth);
    }
  }
  s->depth++;
}

static bool top_is_block(Scanner *s) {
  uint32_t i = s->depth - 1;
  if (i < 64) {
    return (s->kinds >> i) & 1;
  }
  return true; // overflow: treat as block
}

static void pop_paren(Scanner *s) {
  if (s->depth > 0) {
    s->depth--;
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
  bool want_paren = valid_symbols[BLOCK_OPEN] || valid_symbols[BLOCK_CLOSE] ||
                    valid_symbols[LPAREN] || valid_symbols[RPAREN];
  if (!want_rem && !want_paren) {
    return false;
  }

  skip_ws(lexer);
  if (lexer->eof(lexer)) {
    return false;
  }

  int32_t c = lexer->lookahead;

  if (want_rem && (c == 'r' || c == 'R')) {
    if (scan_rem(lexer)) return true;
    return false;
  }

  if (c == '(') {
    if (valid_symbols[BLOCK_OPEN]) {
      lexer->advance(lexer, false);
      push_paren(s, true);
      lexer->result_symbol = BLOCK_OPEN;
      return true;
    }
    if (valid_symbols[LPAREN]) {
      lexer->advance(lexer, false);
      push_paren(s, false);
      lexer->result_symbol = LPAREN;
      return true;
    }
    return false;
  }

  if (c == ')') {
    if (s->depth > 0) {
      bool block = top_is_block(s);
      if (block && valid_symbols[BLOCK_CLOSE]) {
        lexer->advance(lexer, false);
        pop_paren(s);
        lexer->result_symbol = BLOCK_CLOSE;
        return true;
      }
      if (!block && valid_symbols[RPAREN]) {
        lexer->advance(lexer, false);
        pop_paren(s);
        lexer->result_symbol = RPAREN;
        return true;
      }
      // Fallbacks for states where only one form is offered.
      if (valid_symbols[BLOCK_CLOSE]) {
        lexer->advance(lexer, false);
        pop_paren(s);
        lexer->result_symbol = BLOCK_CLOSE;
        return true;
      }
      if (valid_symbols[RPAREN]) {
        lexer->advance(lexer, false);
        pop_paren(s);
        lexer->result_symbol = RPAREN;
        return true;
      }
      return false;
    }
    // depth 0: a `)` can only be literal text.
    if (valid_symbols[RPAREN]) {
      lexer->advance(lexer, false);
      lexer->result_symbol = RPAREN;
      return true;
    }
    return false;
  }

  return false;
}
