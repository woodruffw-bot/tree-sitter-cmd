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
//   HELP_COMMAND_NAME
//                - `if`, `for`, or `rem` only when followed by the exact
//                  documented `/?` help argument at a command boundary.
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
  HELP_COMMAND_NAME,
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

static bool is_help_command_boundary(const Scanner *s, int32_t c) {
  return c == '\r' || c == '\n' || c == '&' || c == '|' ||
         (s->depth > 0 && c == ')');
}

static bool is_help_target_boundary(const Scanner *s, int32_t c) {
  return c == ' ' || c == '\t' || c == ',' || c == ';' || c == '=' ||
         c == '<' || c == '>' || (s->depth > 0 && c == '(') ||
         is_help_command_boundary(s, c);
}

static bool skip_help_line_continuation(const int32_t *tail, size_t length,
                                        size_t *index) {
  size_t i = *index;
  if (i == length || tail[i] != '^') return false;
  i++;
  if (i < length && tail[i] == '\r') i++;
  if (i == length || tail[i] != '\n') return false;
  *index = i + 1;
  return true;
}

static bool skip_help_spacing(const int32_t *tail, size_t length,
                              size_t *index) {
  bool skipped_continuation = false;
  for (;;) {
    while (*index < length &&
           (tail[*index] == ' ' || tail[*index] == '\t')) {
      (*index)++;
    }
    if (!skip_help_line_continuation(tail, length, index)) {
      return skipped_continuation;
    }
    skipped_continuation = true;
  }
}

static bool skip_help_target_prefix(const int32_t *tail, size_t length,
                                    size_t *index, bool *had_spacing) {
  bool had_separator = false;
  for (;;) {
    size_t before = *index;
    while (*index < length) {
      if (tail[*index] == ' ' || tail[*index] == '\t') {
        *had_spacing = true;
      } else if (tail[*index] == ',' || tail[*index] == ';' ||
                 tail[*index] == '=') {
        had_separator = true;
      } else {
        break;
      }
      (*index)++;
    }
    if (skip_help_line_continuation(tail, length, index)) {
      *had_spacing = true;
      continue;
    }
    if (*index == before) return had_separator;
  }
}

static size_t paired_help_expansion_end(const int32_t *tail, size_t length,
                                        size_t start, int32_t sigil) {
  for (size_t i = start + 1; i < length; i++) {
    if (tail[i] == '\r' || tail[i] == '\n') return 0;
    if (tail[i] == sigil) return i + 1;
  }
  return 0;
}

static bool is_tilde_modifier(int32_t c) {
  return c == 'd' || c == 'D' || c == 'p' || c == 'P' || c == 'n' ||
         c == 'N' || c == 'x' || c == 'X' || c == 'f' || c == 'F' ||
         c == 's' || c == 'S' || c == 'a' || c == 'A' || c == 't' ||
         c == 'T' || c == 'z' || c == 'Z';
}

static size_t parameter_tilde_end(const int32_t *tail, size_t length,
                                  size_t start) {
  size_t i = start + 2;
  while (i < length && is_tilde_modifier(tail[i])) i++;

  if (i < length && tail[i] == '$') {
    i++;
    if (i == length ||
        !((tail[i] >= 'A' && tail[i] <= 'Z') ||
          (tail[i] >= 'a' && tail[i] <= 'z') || tail[i] == '_')) {
      return 0;
    }
    while (i < length && tail[i] != ':' && tail[i] != '\r' &&
           tail[i] != '\n') {
      i++;
    }
    if (i == length) return 0;
    i++;
  }

  if (i < length && tail[i] >= '0' && tail[i] <= '9') return i + 1;
  return 0;
}

static bool is_for_variable_char(int32_t c) {
  return c != ' ' && c != '\t' && c != '\r' && c != '\n' && c != '%' &&
         c != '~' && c != '*' && c != '&' && c != '|' && c != '<' &&
         c != '>' && c != '(' && c != ')' && c != '=' && c != '"';
}

static size_t loop_variable_end(const int32_t *tail, size_t length,
                                size_t start) {
  size_t i = start + 2;
  if (i == length || tail[i] != '~') {
    return i < length && is_for_variable_char(tail[i]) ? i + 1 : start + 2;
  }

  i++;
  size_t modifiers_start = i;
  while (i < length && is_tilde_modifier(tail[i])) i++;
  if (i < length && tail[i] == '$') {
    size_t dollar = i++;
    if (i < length &&
        ((tail[i] >= 'A' && tail[i] <= 'Z') ||
         (tail[i] >= 'a' && tail[i] <= 'z') || tail[i] == '_')) {
      while (i < length && tail[i] != ':' && tail[i] != '\r' &&
             tail[i] != '\n') {
        i++;
      }
      if (i < length && tail[i] == ':' && i + 1 < length &&
          is_for_variable_char(tail[i + 1])) {
        return i + 2;
      }
    }
    // A failed PATH_SEARCH can still leave '$' as the final FOR variable.
    return dollar + 1;
  }

  if (i < length && is_for_variable_char(tail[i])) return i + 1;
  // The regex backtracks one modifier when the variable itself is d/p/n/etc.
  return i > modifiers_start ? i : start + 2;
}

static size_t help_expansion_end(const int32_t *tail, size_t length,
                                 size_t start) {
  if (start == length) return 0;
  int32_t c = tail[start];
  if (c == '%') {
    if (start + 1 == length) return 0;
    int32_t next = tail[start + 1];
    if (next == '*' || (next >= '0' && next <= '9')) return start + 2;
    if (next == '%') return loop_variable_end(tail, length, start);
    if (next == '~') return parameter_tilde_end(tail, length, start);
    if (next == '\r' || next == '\n') return 0;
    return paired_help_expansion_end(tail, length, start, '%');
  }
  if (c == '!' && start + 1 < length && tail[start + 1] != '!') {
    size_t end = paired_help_expansion_end(tail, length, start, '!');
    return end > start + 2 ? end : 0;
  }
  return 0;
}

// Mirror REDIRECT_TARGET_SEPARATOR_AHEAD against the buffered help tail. A
// separator selects the standard-argument target only before a real word
// boundary; separators protected by quotes, carets, or paired expansions stay
// inside the general argument.
static bool help_redirect_target_has_separator(const Scanner *s,
                                               const int32_t *tail,
                                               size_t length, size_t start) {
  if (start == length || tail[start] == ',' || tail[start] == ';' ||
      tail[start] == '=') {
    return false;
  }
  bool in_quote = false;
  size_t i = start;

  while (i < length) {
    int32_t c = tail[i];
    if (c == '\r' || c == '\n') return false;

    if (c == '^') {
      i++;
      if (i < length && tail[i] != '\r' && tail[i] != '\n') i++;
      continue;
    }

    if (c == '"') {
      in_quote = !in_quote;
      i++;
      continue;
    }

    if (!in_quote && (c == '%' || c == '!')) {
      int32_t sigil = c;
      bool saw_boundary = false;
      bool saw_separator = false;
      i++;
      while (i < length && tail[i] != '\r' && tail[i] != '\n') {
        c = tail[i++];
        if (c == sigil) {
          saw_separator = false;
          break;
        }
        if (!saw_boundary && (c == ',' || c == ';' || c == '=')) {
          saw_separator = true;
        }
        if (!saw_boundary && is_word_boundary(s, c)) saw_boundary = true;
      }
      if (saw_separator) return true;
      continue;
    }

    if (!in_quote) {
      if (c == ',' || c == ';' || c == '=') return true;
      if (is_word_boundary(s, c)) return false;
    }
    i++;
  }
  return false;
}

// Return the end of one redirection target. Lookahead is buffered so an
// unmatched sigil can remain literal without losing the boundary that follows
// it, while a complete paired expansion can still protect that same byte.
enum HelpTargetFragment {
  HELP_TARGET_FRAGMENT_NONE,
  HELP_TARGET_FRAGMENT_LITERAL,
  HELP_TARGET_FRAGMENT_PAREN,
  HELP_TARGET_FRAGMENT_STRAY_SIGIL,
  HELP_TARGET_FRAGMENT_OTHER,
};

static size_t help_redirect_target_end(const Scanner *s, const int32_t *tail,
                                       size_t length, size_t start,
                                       bool standard_target) {
  size_t i = start;
  enum HelpTargetFragment previous_fragment = HELP_TARGET_FRAGMENT_NONE;
  bool after_continuation = false;
  while (i < length) {
    int32_t c = tail[i];
    bool separator_prefixed_lparen =
        standard_target && i == start && c == '(';
    if (!separator_prefixed_lparen && is_help_target_boundary(s, c) &&
        (c != ',' && c != ';' && c != '=')) {
      break;
    }
    if (standard_target && (c == ',' || c == ';' || c == '=')) break;

    if (c == '^') {
      i++;
      if (i < length && (tail[i] == '%' || tail[i] == '!')) continue;
      if (i < length && tail[i] == '\r') {
        if (i + 1 == length || tail[i + 1] != '\n') return i - 1;
        i++;
      }
      if (i < length && tail[i] == '\n') {
        i++;
        while (i < length && (tail[i] == ' ' || tail[i] == '\t')) i++;
        after_continuation = true;
        continue;
      }
      if (i < length) i++;
      previous_fragment = HELP_TARGET_FRAGMENT_OTHER;
      after_continuation = false;
      continue;
    }

    if (c == '"') {
      i++;
      while (i < length) {
        size_t expansion_end = help_expansion_end(tail, length, i);
        if (expansion_end > 0) {
          i = expansion_end;
          continue;
        }
        if (tail[i] == '"') break;
        i++;
      }
      if (i < length) i++;
      previous_fragment = HELP_TARGET_FRAGMENT_OTHER;
      after_continuation = false;
      continue;
    }

    size_t expansion_end = help_expansion_end(tail, length, i);
    if (expansion_end > 0) {
      i = expansion_end;
      previous_fragment = HELP_TARGET_FRAGMENT_OTHER;
      after_continuation = false;
      continue;
    }

    if (c == '%' || c == '!') {
      i++;
      previous_fragment = HELP_TARGET_FRAGMENT_STRAY_SIGIL;
      after_continuation = false;
      continue;
    }

    if (c == '(' || c == ')') {
      i++;
      previous_fragment = HELP_TARGET_FRAGMENT_PAREN;
      after_continuation = false;
      continue;
    }

    // At a general-fragment boundary, an adjacent `=` continues the argument
    // only when its immediate text token includes at least one following byte.
    // A lone `=` loses to the higher-precedence standard-separator token.
    if (!standard_target && c == '=') {
      if (!after_continuation &&
          previous_fragment != HELP_TARGET_FRAGMENT_LITERAL) {
        break;
      }
      size_t text_end = i + 1;
      while (text_end < length &&
             !is_help_target_boundary(s, tail[text_end]) &&
             tail[text_end] != '^' && tail[text_end] != '"' &&
             tail[text_end] != '%' && tail[text_end] != '!') {
        text_end++;
      }
      while (text_end < length &&
             (tail[text_end] == ',' || tail[text_end] == ';' ||
              tail[text_end] == '=')) {
        text_end++;
      }
      if (text_end == i + 1 &&
          !(after_continuation &&
            previous_fragment != HELP_TARGET_FRAGMENT_NONE)) {
        break;
      }
      i = text_end;
      previous_fragment = HELP_TARGET_FRAGMENT_LITERAL;
      after_continuation = false;
      continue;
    }

    // Consume one literal fragment at a time. In the general target form this
    // naturally keeps comma/semicolon/equals bytes that occur after preceding
    // literal text, matching `argument`; the standard form stops before them.
    size_t text_end = i;
    while (text_end < length &&
           !is_help_target_boundary(s, tail[text_end]) &&
           tail[text_end] != '^' && tail[text_end] != '"' &&
           tail[text_end] != '%' && tail[text_end] != '!' &&
           tail[text_end] != '(' && tail[text_end] != ')') {
      text_end++;
    }
    if (!standard_target) {
      while (text_end < length &&
             (tail[text_end] == ',' || tail[text_end] == ';' ||
              tail[text_end] == '=')) {
        text_end++;
      }
    }
    if (text_end == i) break;
    i = text_end;
    previous_fragment = HELP_TARGET_FRAGMENT_LITERAL;
    after_continuation = false;
  }
  return i;
}

static size_t help_dup_target_end(const int32_t *tail, size_t length,
                                  size_t start) {
  if (start == length) return 0;
  int32_t c = tail[start];
  if (c >= '0' && c <= '9') return start + 1;
  if (c == '%') {
    return help_expansion_end(tail, length, start);
  }
  if (c == '!' && start + 1 < length && tail[start + 1] != '!') {
    return help_expansion_end(tail, length, start);
  }
  return 0;
}

static bool help_tail_has_only_redirects(const Scanner *s,
                                         const int32_t *tail,
                                         size_t length) {
  size_t i = 0;
  bool after_redirect = false;
  for (;;) {
    size_t spacing_start = i;
    bool descriptor_blocked_by_continuation =
        skip_help_spacing(tail, length, &i);
    bool separated_from_help_argument = i > spacing_start;
    if (after_redirect) {
      for (;;) {
        bool consumed_punctuation = false;
        while (i < length &&
               (tail[i] == ',' || tail[i] == ';' || tail[i] == '=')) {
          i++;
          consumed_punctuation = true;
        }
        if (consumed_punctuation) descriptor_blocked_by_continuation = false;
        if (skip_help_spacing(tail, length, &i)) {
          descriptor_blocked_by_continuation = true;
        }
        if (i == length ||
            (tail[i] != ',' && tail[i] != ';' && tail[i] != '=')) {
          break;
        }
      }
    }
    if (i == length || is_help_command_boundary(s, tail[i])) return true;

    if (tail[i] >= '0' && tail[i] <= '9') {
      if (!after_redirect && !separated_from_help_argument) return false;
      // The grammar's external descriptor token cannot skip a preceding
      // continuation without either widening the descriptor's source range or
      // letting the digit become an ordinary argument. Preserve malformed CST
      // state instead of selecting a clean but structurally false help form.
      if (descriptor_blocked_by_continuation) return false;
      i++;
      if (i == length || (tail[i] != '<' && tail[i] != '>')) return false;
    }
    if (tail[i] != '<' && tail[i] != '>') return false;

    int32_t operator = tail[i++];
    if (operator == '>' && i < length && tail[i] == '>') i++;
    bool duplicate = i < length && tail[i] == '&';
    if (duplicate) i++;

    bool standard_target = true;
    if (duplicate) {
      while (i < length &&
             (tail[i] == ' ' || tail[i] == '\t' || tail[i] == ',' ||
              tail[i] == ';' || tail[i] == '=')) {
        i++;
      }
    } else {
      bool had_spacing = false;
      bool had_separator =
          skip_help_target_prefix(tail, length, &i, &had_spacing);
      standard_target =
          had_separator ||
          (!had_spacing &&
           help_redirect_target_has_separator(s, tail, length, i));
    }

    size_t target_end = duplicate ? help_dup_target_end(tail, length, i)
                                  : help_redirect_target_end(
                                        s, tail, length, i, standard_target);
    if ((duplicate && target_end == 0) ||
        (!duplicate && target_end == i)) {
      return false;
    }
    i = target_end;
    after_redirect = true;
  }
}

static bool help_line_ends_in_quote(const int32_t *tail, size_t start,
                                    size_t length) {
  bool in_quote = false;
  size_t i = start;
  while (i < length) {
    if (!in_quote && tail[i] == '^') {
      i++;
      if (i < length && tail[i] != '%' && tail[i] != '!') {
        if (tail[i] == '\r' && i + 1 < length && tail[i + 1] == '\n') {
          i += 2;
        } else {
          i++;
        }
      }
      continue;
    }

    size_t expansion_end = help_expansion_end(tail, length, i);
    if (expansion_end > 0) {
      i = expansion_end;
      continue;
    }
    if (tail[i] == '"') in_quote = !in_quote;
    i++;
  }
  return in_quote;
}

static bool help_tail_has_odd_trailing_carets(const int32_t *tail,
                                               size_t length) {
  size_t i = length;
  if (i > 0 && tail[i - 1] == '\r') i--;
  size_t carets = 0;
  while (i > 0 && tail[i - 1] == '^') {
    carets++;
    i--;
  }
  return carets % 2 != 0;
}

// Accept an exact help tail followed only by redirects. Buffering the logical
// line makes paired-expansion lookahead reversible: when no close exists, the
// sigil is literal and parsing resumes at its real target boundary.
static bool scan_help_command_tail(const Scanner *s, TSLexer *lexer) {
  int32_t *tail = NULL;
  size_t length = 0;
  size_t capacity = 0;
  size_t line_start = 0;

  while (!lexer->eof(lexer)) {
    int32_t c = lexer->lookahead;
    if (c == '\r' || c == '\n') {
      if (help_line_ends_in_quote(tail, line_start, length) ||
          !help_tail_has_odd_trailing_carets(tail, length)) {
        break;
      }
    }

    if (length == capacity) {
      size_t next_capacity = capacity == 0 ? 64 : capacity * 2;
      int32_t *next = ts_realloc(tail, next_capacity * sizeof(*tail));
      if (next == NULL) {
        ts_free(tail);
        return false;
      }
      tail = next;
      capacity = next_capacity;
    }
    tail[length++] = c;
    lexer->advance(lexer, false);
    if (c == '\n') line_start = length;
  }

  bool result = help_tail_has_only_redirects(s, tail, length);
  ts_free(tail);
  return result;
}

// Match the name of an exact documented help command. The token ends before
// the required horizontal space. Looking through the `/?` tail prevents the
// generic command rule from accepting extra arguments after these keywords.
static bool scan_help_command_name(const Scanner *s, TSLexer *lexer,
                                   bool want_rem) {
  int32_t c = lexer->lookahead;
  bool is_rem_name = false;
  if (c == 'i' || c == 'I') {
    lexer->advance(lexer, false);
    c = lexer->lookahead;
    if (c != 'f' && c != 'F') return false;
    lexer->advance(lexer, false);
  } else if (c == 'f' || c == 'F') {
    lexer->advance(lexer, false);
    c = lexer->lookahead;
    if (c != 'o' && c != 'O') return false;
    lexer->advance(lexer, false);
    c = lexer->lookahead;
    if (c != 'r' && c != 'R') return false;
    lexer->advance(lexer, false);
  } else if (c == 'r' || c == 'R') {
    is_rem_name = true;
    lexer->advance(lexer, false);
    c = lexer->lookahead;
    if (c != 'e' && c != 'E') return false;
    lexer->advance(lexer, false);
    c = lexer->lookahead;
    if (c != 'm' && c != 'M') return false;
    lexer->advance(lexer, false);
  } else {
    return false;
  }

  lexer->mark_end(lexer);
  bool rem_boundary =
      is_rem_name &&
      (lexer->eof(lexer) || is_rem_boundary(lexer->lookahead));

  bool has_space = false;
  while (lexer->lookahead == ' ' || lexer->lookahead == '\t') {
    lexer->advance(lexer, false);
    has_space = true;
  }
  if (has_space && lexer->lookahead == '/') {
    lexer->advance(lexer, false);
    if (lexer->lookahead == '?') {
      lexer->advance(lexer, false);
      if (scan_help_command_tail(s, lexer)) {
        lexer->result_symbol = HELP_COMMAND_NAME;
        return true;
      }
    }
  }

  if (want_rem && rem_boundary) {
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
    // Tree-sitter normally consumes caret-newline as an extra before asking
    // the external scanner about a body. Look through it here so an exact help
    // command can still win over the missing-body/concatenation alternatives.
    // If the following bytes are not a help form, declining this scan restores
    // the input and leaves ordinary body parsing unchanged.
    while (lexer->lookahead == '^') {
      lexer->advance(lexer, true);
      if (lexer->lookahead == '\r') lexer->advance(lexer, true);
      if (lexer->lookahead != '\n') return false;
      lexer->advance(lexer, true);
      skipped_space = true;
      while (lexer->lookahead == ' ' || lexer->lookahead == '\t') {
        lexer->advance(lexer, true);
      }
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
    // IF and FOR help commands can begin a controller body in the same parser
    // state that offers the missing-body boundary markers.
    if (skipped_space && valid_symbols[HELP_COMMAND_NAME] &&
        (c == 'i' || c == 'I' || c == 'f' || c == 'F' || c == 'r' ||
         c == 'R')) {
      if (scan_help_command_name(s, lexer, valid_symbols[REM])) return true;
      return false;
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

  bool want_help_command_name = valid_symbols[HELP_COMMAND_NAME];
  bool want_rem = valid_symbols[REM];
  bool want_redirect_source = valid_symbols[REDIRECT_SOURCE];
  bool want_caret = valid_symbols[CARET_ESCAPE];
  bool want_paren = valid_symbols[BLOCK_OPEN] || valid_symbols[BLOCK_CLOSE] ||
                    valid_symbols[LPAREN] || valid_symbols[RPAREN];
  if (!want_help_command_name && !want_rem && !want_redirect_source &&
      !want_caret && !want_paren) {
    return false;
  }

  skip_ws(lexer);
  if (lexer->eof(lexer)) {
    return false;
  }

  int32_t c = lexer->lookahead;

  if (want_help_command_name &&
      (c == 'i' || c == 'I' || c == 'f' || c == 'F' || c == 'r' ||
       c == 'R')) {
    if (scan_help_command_name(s, lexer, want_rem)) return true;
    return false;
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
