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
//   FOR_SINGLE_INNER_QUOTE / FOR_SINGLE_QUOTE_END
//                - an apostrophe inside a single-quoted FOR source, or the
//                  final delimiter before the FOR set's structural close.
//   FOR_BACKQUOTE_INNER_QUOTE / FOR_BACKQUOTE_END
//                - the same distinction for an active usebackq command.
//   FOR_{SINGLE,BACKQUOTE}_{PERCENT,DELAYED}_EXPANSION
//                - a paired expansion containing the active delimiter. The
//                  scanner can look past the delimiter without letting an
//                  unmatched sigil consume a later percent in the FOR body.
//   FOR_F_SINGLE_COMMAND_SOURCE_AHEAD / FOR_F_BACKQUOTE_COMMAND_SOURCE_AHEAD
//                - zero-width confirmation that the active delimiter encloses
//                  the whole FOR set rather than one neutral set item.
//   FOR_F_DEFAULT_MODE / FOR_F_USEBACKQ_MODE
//                - zero-width selection of the active FOR /F command delimiter
//                  after inspecting only a standalone usebackq option.
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
  FOR_SINGLE_INNER_QUOTE,
  FOR_SINGLE_QUOTE_END,
  FOR_BACKQUOTE_INNER_QUOTE,
  FOR_BACKQUOTE_END,
  FOR_SINGLE_PERCENT_EXPANSION,
  FOR_SINGLE_DELAYED_EXPANSION,
  FOR_BACKQUOTE_PERCENT_EXPANSION,
  FOR_BACKQUOTE_DELAYED_EXPANSION,
  FOR_F_SINGLE_COMMAND_SOURCE_AHEAD,
  FOR_F_BACKQUOTE_COMMAND_SOURCE_AHEAD,
  FOR_F_DEFAULT_MODE,
  FOR_F_USEBACKQ_MODE,
  ERROR_SENTINEL,
};

typedef struct {
  uint32_t depth; // number of currently-open structural blocks
  uint8_t body_boundaries;
} Scanner;

static bool is_for_variable_char(int32_t c) {
  return c != ' ' && c != '\t' && c != '\r' && c != '\n' && c != '%' &&
         c != '~' && c != '*' && c != '&' && c != '|' && c != '<' &&
         c != '>' && c != '(' && c != ')' && c != '=' && c != '"';
}

static bool is_tilde_modifier(int32_t c) {
  return c == 'd' || c == 'D' || c == 'p' || c == 'P' || c == 'n' ||
         c == 'N' || c == 'x' || c == 'X' || c == 'f' || c == 'F' ||
         c == 's' || c == 'S' || c == 'a' || c == 'A' || c == 't' ||
         c == 'T' || c == 'z' || c == 'Z';
}

static bool is_env_name_start(int32_t c) {
  return (c >= 'A' && c <= 'Z') || (c >= 'a' && c <= 'z') || c == '_';
}

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

// Return the end of a complete paired expansion on one physical line. When an
// expansion containing the active delimiter reaches the set's `)` before its
// apparent closing sigil, accept that close only if a later delimiter can still
// terminate the quoted item. This keeps an unmatched source sigil from pairing
// with `%`/`!` in the FOR body.
static size_t for_source_paired_expansion_end(const int32_t *line,
                                              size_t length, size_t start,
                                              int32_t delimiter) {
  int32_t sigil = line[start];
  if (start + 1 == length || line[start + 1] == sigil) return 0;
  if (sigil == '%' &&
      (line[start + 1] == '~' || line[start + 1] == '*' ||
       (line[start + 1] >= '0' && line[start + 1] <= '9'))) {
    return 0;
  }

  bool saw_delimiter = false;
  bool saw_structural_close = false;
  for (size_t i = start + 1; i < length; i++) {
    if (line[i] == delimiter) saw_delimiter = true;
    if (line[i] == ')') saw_structural_close = true;
    if (line[i] != sigil) continue;

    if (saw_delimiter && saw_structural_close) {
      for (size_t j = i + 1; j < length; j++) {
        if (line[j] == delimiter) return i + 1;
      }
      return 0;
    }
    return i + 1;
  }
  return 0;
}

// Confirm that an active FOR /F delimiter encloses the entire parenthesized
// set. A delimiter followed by another set value remains a neutral quoted item.
// Physical lines are buffered so paired expansions can protect an internal
// active delimiter without making unmatched tails consume the later FOR body.
static bool scan_for_f_command_source_ahead(const Scanner *s, TSLexer *lexer,
                                            int32_t delimiter,
                                            enum TokenType symbol) {
  lexer->mark_end(lexer);
  for (;;) {
    while (lexer->lookahead == ' ' || lexer->lookahead == '\t') {
      lexer->advance(lexer, true);
    }
    if (lexer->lookahead == '^') {
      lexer->advance(lexer, true);
      if (lexer->lookahead == '\r') lexer->advance(lexer, true);
      if (lexer->lookahead != '\n') return false;
      lexer->advance(lexer, true);
      continue;
    }
    if (lexer->lookahead == ',' || lexer->lookahead == ';' ||
        lexer->lookahead == '=' || lexer->lookahead == '\n') {
      lexer->advance(lexer, false);
      continue;
    }
    if (lexer->lookahead == '\r') {
      lexer->advance(lexer, false);
      if (lexer->lookahead != '\n') return false;
      lexer->advance(lexer, false);
      continue;
    }
    break;
  }

  if (s->depth == 0 || lexer->lookahead != delimiter) return false;
  lexer->advance(lexer, false);

  bool saw_delimiter = false;
  bool delimiter_is_last = false;
  for (;;) {
    int32_t *line = NULL;
    size_t length = 0;
    size_t capacity = 0;
    while (!lexer->eof(lexer) && lexer->lookahead != '\r' &&
           lexer->lookahead != '\n') {
      if (length == capacity) {
        size_t next_capacity = capacity == 0 ? 64 : capacity * 2;
        int32_t *next = ts_realloc(line, next_capacity * sizeof(*line));
        if (next == NULL) {
          ts_free(line);
          return false;
        }
        line = next;
        capacity = next_capacity;
      }
      line[length++] = lexer->lookahead;
      lexer->advance(lexer, false);
    }

    bool has_line_break = !lexer->eof(lexer);
    bool in_double_quote = false;
    size_t i = 0;
    while (i < length) {
      int32_t c = line[i];
      if (!in_double_quote && c == '^') {
        if (i + 1 == length) {
          if (!has_line_break) delimiter_is_last = false;
          i++;
          continue;
        }
        delimiter_is_last = false;
        i += 2;
        continue;
      }

      if (c == '"') {
        delimiter_is_last = false;
        in_double_quote = !in_double_quote;
        i++;
        continue;
      }

      if (!in_double_quote && (c == '%' || c == '!')) {
        int32_t sigil = c;
        delimiter_is_last = false;

        if (sigil == '%' && i + 1 < length &&
            (line[i + 1] == '*' ||
             (line[i + 1] >= '0' && line[i + 1] <= '9'))) {
          i += 2;
          continue;
        }
        if (sigil == '%' && i + 1 < length && line[i + 1] == '%') {
          i += 2;
          if (i < length && line[i] != delimiter && line[i] != '^' &&
              line[i] != '!' && is_for_variable_char(line[i])) {
            i++;
          }
          continue;
        }
        if (sigil == '%' && i + 1 < length && line[i + 1] == '~') {
          i += 2;
          while (i < length && is_tilde_modifier(line[i])) i++;
          if (i < length && line[i] == '$') {
            i++;
            if (i < length && is_env_name_start(line[i])) {
              while (i < length && line[i] != ':' && line[i] != '"' &&
                     line[i] != '^' && line[i] != delimiter) {
                i++;
              }
              if (i < length && line[i] == ':') i++;
            }
          }
          if (i < length && line[i] >= '0' && line[i] <= '9') i++;
          continue;
        }

        size_t expansion_end = for_source_paired_expansion_end(
            line, length, i, delimiter);
        if (expansion_end > 0) {
          i = expansion_end;
          continue;
        }

        size_t delimiter_index = i + 1;
        while (delimiter_index < length &&
               line[delimiter_index] != delimiter) {
          delimiter_index++;
        }
        if (delimiter_index == length) {
          ts_free(line);
          lexer->result_symbol = symbol;
          return true;
        }
        i = delimiter_index;
        continue;
      }

      if (!in_double_quote && c == delimiter) {
        saw_delimiter = true;
        delimiter_is_last = true;
        i++;
        continue;
      }

      if (!in_double_quote && c == ')') {
        bool result = !saw_delimiter || delimiter_is_last;
        ts_free(line);
        if (result) lexer->result_symbol = symbol;
        return result;
      }

      if (!in_double_quote &&
          (c == '&' || c == '|' || c == '<' || c == '>')) {
        ts_free(line);
        lexer->result_symbol = symbol;
        return true;
      }

      if (!in_double_quote &&
          (c == ' ' || c == '\t' || c == ',' || c == ';' || c == '=')) {
        i++;
        continue;
      }

      delimiter_is_last = false;
      i++;
    }
    ts_free(line);

    if (lexer->eof(lexer)) break;
    if (lexer->lookahead == '\r') lexer->advance(lexer, false);
    if (lexer->lookahead == '\n') lexer->advance(lexer, false);
  }

  if (saw_delimiter && !delimiter_is_last) return false;
  lexer->result_symbol = symbol;
  return true;
}

// Match a closed expansion only when it actually contains the surrounding
// FOR /F delimiter. Internal grammar tokens handle the unambiguous case. If a
// structural close appears before the expansion closes, require a later outer
// delimiter; this prevents an unmatched source sigil from pairing with a `%`
// or `!` in the FOR body after the set has already ended.
static bool scan_for_source_delimited_expansion(TSLexer *lexer, int32_t sigil,
                                                int32_t delimiter,
                                                enum TokenType symbol) {
  if (lexer->lookahead != sigil) return false;
  lexer->advance(lexer, false);

  if (lexer->eof(lexer) || lexer->lookahead == '\r' ||
      lexer->lookahead == '\n' || lexer->lookahead == sigil) {
    return false;
  }
  if (sigil == '%' &&
      (lexer->lookahead == '~' || lexer->lookahead == '*' ||
       (lexer->lookahead >= '0' && lexer->lookahead <= '9'))) {
    return false;
  }

  bool saw_delimiter = false;
  bool saw_structural_close = false;
  while (!lexer->eof(lexer) && lexer->lookahead != '\r' &&
         lexer->lookahead != '\n') {
    int32_t c = lexer->lookahead;
    lexer->advance(lexer, false);
    if (c == delimiter) saw_delimiter = true;
    if (c == ')') saw_structural_close = true;
    if (c != sigil) continue;
    if (!saw_delimiter) return false;

    lexer->mark_end(lexer);
    if (!saw_structural_close) {
      lexer->result_symbol = symbol;
      return true;
    }

    while (!lexer->eof(lexer) && lexer->lookahead != '\r' &&
           lexer->lookahead != '\n') {
      if (lexer->lookahead == delimiter) {
        lexer->result_symbol = symbol;
        return true;
      }
      lexer->advance(lexer, false);
    }
    return false;
  }
  return false;
}

// A FOR /F command source ends at the final active delimiter before the set's
// structural close. ParseFor tokenizes the whole parenthesized set first, so
// horizontal whitespace, newlines, caret line continuations, and standard
// separators may follow the delimiter. Mark the token end immediately after
// the quote so lookahead never consumes those bytes.
static bool scan_for_command_quote(const Scanner *s, TSLexer *lexer,
                                   const bool *valid_symbols,
                                   int32_t delimiter,
                                   enum TokenType inner_symbol,
                                   enum TokenType end_symbol) {
  if (s->depth == 0 || lexer->lookahead != delimiter) return false;

  lexer->advance(lexer, false);
  lexer->mark_end(lexer);

  while (!lexer->eof(lexer)) {
    switch (lexer->lookahead) {
      case ' ':
      case '\t':
      case ',':
      case ';':
      case '=':
      case '\n':
        lexer->advance(lexer, false);
        continue;
      case '\r':
        lexer->advance(lexer, false);
        if (lexer->lookahead != '\n') return false;
        lexer->advance(lexer, false);
        continue;
      case '^':
        lexer->advance(lexer, false);
        if (lexer->lookahead == '\r') {
          lexer->advance(lexer, false);
          if (lexer->lookahead != '\n') return false;
        }
        if (lexer->lookahead != '\n') {
          if (!valid_symbols[inner_symbol]) return false;
          lexer->result_symbol = inner_symbol;
          return true;
        }
        lexer->advance(lexer, false);
        continue;
      case ')':
        if (!valid_symbols[end_symbol]) return false;
        lexer->result_symbol = end_symbol;
        return true;
      default:
        if (!valid_symbols[inner_symbol]) return false;
        lexer->result_symbol = inner_symbol;
        return true;
    }
  }

  if (!valid_symbols[inner_symbol]) return false;
  lexer->result_symbol = inner_symbol;
  return true;
}

static int32_t ascii_lower(int32_t c) {
  return c >= 'A' && c <= 'Z' ? c + ('a' - 'A') : c;
}

static void finish_for_f_option_word(bool *found, size_t *length,
                                     bool *matches) {
  static const char usebackq[] = "usebackq";
  if (*matches && *length == sizeof(usebackq) - 1) *found = true;
  *length = 0;
  *matches = true;
}

static void add_for_f_option_char(int32_t c, bool *found, size_t *length,
                                  bool *matches) {
  static const char usebackq[] = "usebackq";
  if (c == ' ' || c == '\t') {
    finish_for_f_option_word(found, length, matches);
    return;
  }
  if (*length >= sizeof(usebackq) - 1 ||
      ascii_lower(c) != usebackq[*length]) {
    *matches = false;
  }
  (*length)++;
}

// Inspect the optional FOR /F argument without consuming it. Quotes and
// unquoted caret escapes are removed for the one question that changes cmd's
// grammar: whether one whitespace-delimited option word is exactly `usebackq`.
// Carets inside double quotes stay literal, matching cmd. Everything else
// remains an opaque argument for the internal lexer.
static bool scan_for_f_mode(TSLexer *lexer, const bool *valid_symbols) {
  lexer->mark_end(lexer);

  bool started = false;
  bool in_quote = false;
  bool found = false;
  bool word_matches = true;
  size_t word_length = 0;

  while (!lexer->eof(lexer)) {
    int32_t c = lexer->lookahead;
    if (c == '\r' || c == '\n') break;

    if (!in_quote && c == '^') {
      lexer->advance(lexer, false);
      if (lexer->lookahead == '\r') {
        lexer->advance(lexer, false);
        if (lexer->lookahead != '\n') break;
      }
      if (lexer->lookahead == '\n') {
        lexer->advance(lexer, false);
        continue;
      }
      if (lexer->eof(lexer)) break;
      started = true;
      c = lexer->lookahead;
      lexer->advance(lexer, false);
      add_for_f_option_char(c, &found, &word_length, &word_matches);
      continue;
    }

    if (!started &&
        (c == ' ' || c == '\t' || c == ',' || c == ';' || c == '=')) {
      lexer->advance(lexer, false);
      continue;
    }

    if (c == '"') {
      started = true;
      in_quote = !in_quote;
      lexer->advance(lexer, false);
      continue;
    }

    if (started && !in_quote &&
        (c == ' ' || c == '\t' || c == ',' || c == ';' || c == '=')) {
      break;
    }

    started = true;
    lexer->advance(lexer, false);
    add_for_f_option_char(c, &found, &word_length, &word_matches);
  }

  finish_for_f_option_word(&found, &word_length, &word_matches);
  enum TokenType mode = found ? FOR_F_USEBACKQ_MODE : FOR_F_DEFAULT_MODE;
  if (!valid_symbols[mode]) return false;
  lexer->result_symbol = mode;
  return true;
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

  if (valid_symbols[FOR_F_DEFAULT_MODE] ||
      valid_symbols[FOR_F_USEBACKQ_MODE]) {
    return scan_for_f_mode(lexer, valid_symbols);
  }

  if (lexer->lookahead == '%' &&
      valid_symbols[FOR_SINGLE_PERCENT_EXPANSION]) {
    return scan_for_source_delimited_expansion(
        lexer, '%', '\'', FOR_SINGLE_PERCENT_EXPANSION);
  }
  if (lexer->lookahead == '!' &&
      valid_symbols[FOR_SINGLE_DELAYED_EXPANSION]) {
    return scan_for_source_delimited_expansion(
        lexer, '!', '\'', FOR_SINGLE_DELAYED_EXPANSION);
  }
  if (lexer->lookahead == '%' &&
      valid_symbols[FOR_BACKQUOTE_PERCENT_EXPANSION]) {
    return scan_for_source_delimited_expansion(
        lexer, '%', '`', FOR_BACKQUOTE_PERCENT_EXPANSION);
  }
  if (lexer->lookahead == '!' &&
      valid_symbols[FOR_BACKQUOTE_DELAYED_EXPANSION]) {
    return scan_for_source_delimited_expansion(
        lexer, '!', '`', FOR_BACKQUOTE_DELAYED_EXPANSION);
  }

  if (valid_symbols[FOR_F_SINGLE_COMMAND_SOURCE_AHEAD]) {
    return scan_for_f_command_source_ahead(
        s, lexer, '\'', FOR_F_SINGLE_COMMAND_SOURCE_AHEAD);
  }
  if (valid_symbols[FOR_F_BACKQUOTE_COMMAND_SOURCE_AHEAD]) {
    return scan_for_f_command_source_ahead(
        s, lexer, '`', FOR_F_BACKQUOTE_COMMAND_SOURCE_AHEAD);
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

  // Only the final apostrophe before the FOR set close terminates a
  // single-quoted command source. Earlier apostrophes stay opaque content.
  if (valid_symbols[FOR_SINGLE_INNER_QUOTE] ||
      valid_symbols[FOR_SINGLE_QUOTE_END]) {
    return scan_for_command_quote(s, lexer, valid_symbols, '\'',
                                  FOR_SINGLE_INNER_QUOTE,
                                  FOR_SINGLE_QUOTE_END);
  }
  if (valid_symbols[FOR_BACKQUOTE_INNER_QUOTE] ||
      valid_symbols[FOR_BACKQUOTE_END]) {
    return scan_for_command_quote(s, lexer, valid_symbols, '`',
                                  FOR_BACKQUOTE_INNER_QUOTE,
                                  FOR_BACKQUOTE_END);
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
