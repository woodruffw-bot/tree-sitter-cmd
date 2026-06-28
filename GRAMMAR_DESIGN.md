# tree-sitter-cmd — Grammar Design Document

Status: **implemented**. This document was written pre-implementation as the
guide; the shipped grammar follows it closely, with these notable divergences
decided during implementation:

- Expansions (`%VAR%`, `!VAR!`, `%1`, `%*`, `%~…`, `%%i`, `%%`) are **single
  tokens** rather than sub-noded `seq`s — maximal munch then cleanly prefers a
  real expansion over a stray `%`/`!`, with no GLR ambiguity. Substring/
  substitution operators are therefore not sub-noded (see §8 limitations).
- Strings are a **single opaque token** (`"…"`), avoiding the empty-string /
  outside-fragment ambiguity; interiors are not sub-noded.
- Word concatenation uses the external `_concat` token (as planned); `rem` is
  also an external token (tree-sitter's keyword extraction handles every other
  keyword but declines `rem`).
- `=` is a word boundary in the scanner so `_concat` cannot starve `==` /
  `name=value`.

Validated against 12 real-world scripts (11 parse error-free). See `README.md`
for the conformance table and the live limitation list.

Target package: `tree-sitter-cmd` (`scope: source.dosbatch`, file types
`.bat`, `.cmd`). License: MIT.

This document guides implementation of `grammar.js` and (where unavoidable)
`src/scanner.c`. It is grounded in the ReactOS cmd.exe reimplementation
(`base/shell/cmd/parser.c`, `cmd.c`, `batch.c`, `for.c`, `if.c`, `goto.c`,
`call.c`), the dBenham/jeb "BatchLineParser" phase model (DosTips t=3587),
ss64/MS-Learn documentation, and prior tree-sitter art (wharflab/tree-sitter-batch,
tree-sitter-bash).

---

## 1. Overview & scope

### 1.1 What the grammar covers

A `.bat`/`.cmd` file is a newline-separated sequence of physical lines. cmd.exe
has **no whole-file grammar**: each logical line is read, expanded, parsed, and
executed before the next is read (ReactOS `ParseCommand(NULL)` per line). We
nonetheless model the whole file as one syntax tree because a *static* tool
(editor, highlighter, linter) wants a tree for the entire buffer.

In scope:

- Simple commands (command name + argument tail), the `@` quiet prefix.
- Command sequencing / boolean operators: `&`, `&&`, `||`, and pipes `|`.
- Redirections: `>`, `>>`, `<`, `N>`, `N>>`, `N<`, handle duplication `N>&M`/`N<&M`.
- Parenthesized command blocks `( … )`, multi-line, with attached redirections.
- Control flow: `IF`/`ELSE` (single-line and block), `FOR` (plain, `/D`, `/R`,
  `/L`, `/F`), `GOTO`, `CALL`, labels.
- Comments: `REM` and `::` (degenerate label), inline `& rem …`.
- All three expansion sigils: `%VAR%` / `%N` / `%*` / `%~mods` immediate,
  `!VAR!` delayed, and `%%x` FOR variables, plus the `:~off,len` substring and
  `:search=replace` substitution operators.
- `SET`, `SET /A` (arithmetic sub-language), `SET /P`.
- Caret escaping `^x` and caret line continuation `^\n`.
- Double-quoted strings (grouping only — cmd does not strip quotes).

Out of scope (documented as runtime-only, not syntactic):

- Actual variable *values*, environment state, file-system globbing results.
- Receiving-program `argv` parsing (CommandLineToArgvW / MSVCRT backslash-quote
  rules) — a **separate grammar** downstream of cmd; we stop at the cmd token boundary.
- Runtime semantics of `&&`/`||` (exit-code gating), `GOTO` target resolution,
  `SHIFT` renumbering, `SETLOCAL` scoping.

### 1.2 Conformance philosophy

We target **Windows cmd.exe** behavior (the batch-file dialect), not the
interactive command line and not ReactOS quirks where they diverge. Concretely:

- Assume **batch context** always (`%%` → `%`, positional `%1`, `%%x` FOR vars).
  The interactive single-`%` FOR-var dialect is *not* a goal; if cheaply
  accommodated, fine, but batch wins on conflict.
- **Over-accept** runtime-gated constructs that cannot be tracked statically:
  `!VAR!` is always parsed as a delayed reference even though it is literal text
  unless `SETLOCAL ENABLEDELAYEDEXPANSION`/`cmd /V:ON` is active. Document this.
- Keep the **label-vs-comment distinction** that Windows makes (`:name` is a jump
  target, `::…` is a skipped/comment label). ReactOS collapses both into "comment";
  we do *not* follow ReactOS here.
- Goal is a **recognizer + highlightable CST**, not an executable AST. We do **not**
  reproduce ReactOS's right-leaning operator AST shape (see §4.4); we use the
  conventional left-associative tree-sitter shape, which preserves observable
  left-to-right reading order and is what tooling expects.

### 1.3 Known hard parts of cmd lexing (why this is not a normal language)

1. **Phased, not single-pass.** cmd runs each line through an ordered pipeline
   (percent → caret/tokenize → delayed `!`). The *same byte* (`^ % ! "`) means
   different things in different phases. A tree-sitter grammar is a single
   context-free pass and **cannot** reproduce phase ordering; it can only
   approximate the surface syntax.
2. **Context-sensitive tokenization.** The same characters lex differently
   depending on quote state, parenthesis depth (`InsideBlock`), whether a digit
   is at a token boundary (redirection vs text), and which FOR variables are in
   scope (the `%~dpnxg` greedy-backtrack ambiguity). There is no regular
   tokenization independent of parser state.
3. **Separators are not universal whitespace.** `, ; =` act like spaces in most
   contexts but **not** in IF's left operand (where `=` must survive for `==`) or
   in a command's argument tail (read with *no* separators). A grammar cannot put
   them in `extras`.
4. **Caret rebinds the role of the next character**, including separators and
   newlines (line continuation resolved *before* tokenization). This is the
   single strongest motivation for an external scanner.
5. **Expansion happens before parsing.** `%VAR%` content can introduce or destroy
   operators/quotes invisibly to a syntactic grammar. We parse the *unexpanded*
   source and accept this imprecision.

---

## 2. Lexing strategy

### 2.1 The cmd per-line parsing phases (reference model)

Per dBenham's phase numbering (informative — we do **not** execute these, we
just need to know which byte means what):

| Phase | Action | Affects |
|------:|--------|---------|
| 1 | Percent (`%`) expansion (batch: `%%`→`%`, `%N`, `%*`, `%~`, `%VAR%`) | `%` |
| 1.5 | Strip `\r` (CRs never separate tokens, never stored) | `\r` |
| 2 | Caret/quote/special-char tokenization; `^` removed, `^\n` continuation; recognize `& && \| \|\| ( ) < > >>`; detect REM/IF/FOR | `^ " & \| < > ( )` |
| 3 | ECHO if echo on | — |
| 4 | FOR `%%x` loop-variable substitution (inside executing FOR) | `%%x` |
| 5 | Delayed (`!`) expansion — **only if** enabled **and** line contains `!`; a *second* caret-removal pass that ignores quote state | `! ^` |
| 6 | CALL extra pass (re-parse 1–2 with extra caret/percent doubling) | — |
| 7 | Execute | — |

Key consequences for the grammar: percent is resolved *before* caret; delayed `!`
*after* caret, in a separate pass; `\r` is invisible.

### 2.2 What goes in `grammar.js` vs `src/scanner.c`

Prior art (wharflab/tree-sitter-batch) proves **most of cmd can be done with no
external scanner**: `[ \t]`-only extras, significant newlines, `token.immediate`
for tight expansion lexing, `ci()`/`kw()` case-insensitive helpers, and `prec`
to resolve command/pipe/operator/control-flow ambiguity. We adopt that
architecture as the baseline.

**Plan: ship Milestones 1–6 with NO external scanner.** Introduce a scanner only
if/when grammar-only token regexes prove brittle (Milestone 7+). The scanner is
an optimization/fidelity tool, not a prerequisite.

Handled by **plain `grammar.js`**:

- Statement structure, operators, precedence (`prec.left`/`prec.right`).
- Redirections (fixed lexical patterns; `token.immediate` binds the fd digit).
- Variable references (`%VAR%`, `!VAR!`, `%N`, `%*`, `%~mods`, `%%x`,
  `:~`/`:=` operators) as a `choice`, wrapped with `token.immediate` for
  adjacency/concatenation.
- Line continuation `token(/\^\r?\n[ \t]*/)`.
- Caret escapes of a single following metacharacter `^[&|<>()^"]` as a fixed
  token.
- Case-insensitive keywords via `ci()`.
- REM / echo / label bodies as free-form-to-EOL token regexes.

**Candidates for `src/scanner.c`** (only if needed), each gated on
`valid_symbols[X]`:

- `_concat` — general no-whitespace fragment concatenation into one `argument`
  (mirror tree-sitter-bash). Needed only if `token.immediate` adjacency proves
  insufficient for arbitrary `bareword%VAR%"q"!V!` runs.
- `_line_continuation` — if the regex form interacts badly with mid-token joins
  (the caret can splice *inside* a token).
- `_caret_escape` — caret that rebinds the role of the *following separator*
  (the genuinely context-sensitive case the regex `^[&|<>()^"]` does not cover,
  e.g. `^ ` producing a non-delimiter space).
- `_echo_body` / `_rem_body` / `_label_body` — free-form-to-unescaped-newline
  capture if regexes are brittle (REM must stop before an unescaped `\n` but a
  trailing `^\n` continues).
- `__error_recovery` — sentinel external, true when **all** `valid_symbols` are
  set, to bail during error recovery (exactly as tree-sitter-bash does).

### 2.3 Anticipated external-scanner token list

If/when a scanner is written, these are the tokens it would own (names are the
`externals` array):

```
externals: $ => [
  $._concat,            // adjacency join (no whitespace) of argument fragments
  $._line_continuation, // ^ \r? \n [ \t]*  spliced mid-token
  $._caret_escape,      // ^ <char> where <char>'s ROLE changes (esp. separators)
  $._echo_body,         // free text to unescaped EOL after ECHO
  $._rem_body,          // free text to unescaped EOL after REM
  $._label_body,        // label name + ignored tail
  $._newline,           // significant statement terminator (if regex insufficient)
  $.__error_recovery,   // sentinel: true iff every valid_symbol is set
]
```

Scanner discipline (from tree-sitter-bash + docs): gate every token on
`valid_symbols[X]`; call `eof()` inside every loop (docs warn externals "can
easily create infinite loops"); use `lexer->mark_end` for lookahead; distinguish
`advance(skip=true/false)`; keep `serialize()` within
`TREE_SITTER_SERIALIZATION_BUFFER_SIZE` (~1024 bytes); the only state needed is
parenthesis depth + quote flag (a couple of bytes).

### 2.4 `extras`, `word`, `conflicts` config

```js
extras: $ => [ /[ \t]/ ],          // NOT newline (significant), NOT comments (statement-level)
word:   $ => $._word,              // keyword extraction over the bareword token
conflicts: $ => [
  [$.command, $._argument],        // greedy arg consumption
  [$.block, $.set_arith_paren],    // grouping paren vs SET /A paren
],
```

- `extras` is `[ \t]` **only**. Newline is a statement terminator and must be
  structural. Comments are full-line constructs, not free-floating extras.
- `word` enables keyword extraction so `IF`/`FOR`/`REM`/`SET` are recognized
  robustly against barewords (and so a file literally named `if` works as an arg).
- `\r` is *not* in extras; instead every newline pattern is `/\r?\n/` so CRs are
  swallowed at line ends (matching cmd's unconditional CR strip).

---

## 3. Node taxonomy

Named nodes (CST rules). `_`-prefixed = hidden/inline. Fields in `field(...)`.
RHS sketches use tree-sitter DSL (`seq`/`choice`/`repeat`/`optional`/`prec`/
`token`/`alias`).

### 3.1 Top level & statements

| Node | Description | RHS sketch |
|------|-------------|-----------|
| `program` | whole file: newline-separated lines | `repeat($._line)` |
| `_line` (hidden) | one physical line of statements or blank | `choice(seq($._statement_list, $._newline), $._newline, $.label, $.comment)` |
| `_statement_list` (hidden) | commands joined by separators on one logical line | recursion via operator rules below |
| `_newline` (hidden) | significant terminator | `token(/\r?\n/)` |
| `_separator` (hidden) | inline command separator inside blocks | `choice($._newline, '&', ...)` |

### 3.2 Commands & prefixes

| Node | Description | RHS sketch |
|------|-------------|-----------|
| `command` | a simple command: name + interleaved args/redirs | `prec.right(seq(optional($.quiet), field('name', $.command_name), repeat(choice($._argument, $.redirection))))` |
| `quiet` | `@` echo-suppress prefix (stackable) | `repeat1('@')` (or `prec` unary; see §4) |
| `command_name` | first token; larger break set | `alias($._word_cmdname, $.command_name)` |
| `_argument` (hidden) | one whitespace-delimited argument = concat of fragments | `repeat1(choice($.string, $._variable, $._bareword_frag, $._caret_seq))` |
| `string` | double-quoted span (cmd does not strip; we keep quotes) | `seq('"', repeat(choice($._variable, $._dq_text)), optional('"'))` (unbalanced tolerated) |
| `_bareword_frag` (hidden) | run of ordinary chars, no metachars/ws | `token(/[^ \t\r\n&|<>()^"%!]+/)` |
| `_caret_seq` (hidden) | escaped metachar `^x` | `token(/\^[\s\S]/)` |

Note `command` uses `prec.right` so it greedily consumes its argument tail and
trailing redirections before an operator at the enclosing level binds.

### 3.3 Operators / sequencing (precedence ladder)

cmd binding, LOWEST→HIGHEST: `&` < `||` < `&&` < `|`. We use left-assoc
tree-sitter precedence (conventional shape; see §4.4 on AST shape vs ReactOS).

| Node | Description | RHS sketch |
|------|-------------|-----------|
| `pipeline` | `cmd \| cmd` (tightest) | `prec.left(4, seq($._unit, '\|', $._unit))` |
| `and_list` | `cmd && cmd` | `prec.left(3, seq($._unit, '&&', $._unit))` |
| `or_list` | `cmd \|\| cmd` | `prec.left(2, seq($._unit, '\|\|', $._unit))` |
| `seq_list` | `cmd & cmd` (loosest; empty RHS allowed) | `prec.left(1, seq($._unit, '&', optional($._unit)))` |
| `_unit` (hidden) | operand of an operator | `choice($.command, $.block, $.if, $.for, ...)` |

`&` allows an empty RHS (ReactOS `MSCMD_MULTI_EMPTY_RHS` default off → LHS alone),
hence `optional` on its right. `&&`/`\|\|`/`\|` require both sides.

### 3.4 Redirections

| Node | Description | RHS sketch |
|------|-------------|-----------|
| `redirection` | one redirect bound to a command/block | `seq(choice($.redirect_file, $.redirect_dup))` |
| `redirect_file` | `[N]( > \| >> \| < )target` | `seq(optional(field('fd', $.file_descriptor)), field('op', choice('>>','>','<')), field('target', $._argument))` |
| `redirect_dup` | handle duplication `[N]>&M` / `[N]<&M` | `seq(optional(field('fd',$.file_descriptor)), field('op', choice('>&','<&')), field('target', $.file_descriptor))` |
| `file_descriptor` | single digit fd, immediate-bound | `token.immediate(/[0-9]/)` (leading fd: see §4.5) |

The fd digit must be `token.immediate` against the operator (no space). A
*leading* fd at a token boundary (`2>file`) needs the boundary rule in §4.5.

### 3.5 Blocks

| Node | Description | RHS sketch |
|------|-------------|-----------|
| `block` | `( … )` compound, multi-line, optional trailing redirs | `seq('(', optional($._block_body), ')', repeat($.redirection))` |
| `_block_body` (hidden) | commands separated by newline/`&`/`&&`/`\|\|`/`\|` | `seq(repeat($._newline), $._statement_list, repeat(seq($._sep_in_block, optional($._statement_list))))` |

An empty `( )` is a ParseError in cmd; we may accept it leniently or mark it.
Newlines inside `(…)` act like `&`. Closing `)` may be on a later line.

### 3.6 Control flow — IF

| Node | Description | RHS sketch |
|------|-------------|-----------|
| `if` | full IF with optional ELSE | `prec.right(8, seq(ci('if'), optional($.if_flag), optional($.not), field('cond', $._if_condition), field('then', $._body), optional($._else)))` |
| `if_flag` | `/I` case-insensitive | `ci('/i')` |
| `not` | `NOT` negation | `ci('not')` |
| `_else` (hidden) | `ELSE` branch (same physical line as `)` ELSE `(`) | `seq(ci('else'), field('else', $._body))` |
| `_if_condition` (hidden) | one condition form | `choice($.cond_compare, $.cond_exist, $.cond_defined, $.cond_errorlevel, $.cond_cmdextversion)` |
| `cond_compare` | `lhs OP rhs` (`==` or EQU/NEQ/LSS/LEQ/GTR/GEQ) | `seq(field('left',$._if_operand), field('op',$.compare_op), field('right',$._if_operand))` |
| `compare_op` | comparison operator | `choice('==', ci('equ'), ci('neq'), ci('lss'), ci('leq'), ci('gtr'), ci('geq'))` |
| `cond_exist` | `EXIST filename` | `seq(ci('exist'), field('arg', $._argument))` |
| `cond_defined` | `DEFINED name` (bare, no `%`) | `seq(ci('defined'), field('arg', $._argument))` |
| `cond_errorlevel` | `ERRORLEVEL n` (>= test) | `seq(ci('errorlevel'), field('arg', $._argument))` |
| `cond_cmdextversion` | `CMDEXTVERSION n` | `seq(ci('cmdextversion'), field('arg', $._argument))` |
| `_if_operand` (hidden) | operand for `==`/relational | `$._argument` |
| `_body` (hidden) | single command or block | `choice($.block, $._unit)` |

`prec.right` on `if` attaches `else` to the nearest `if` (dangling-else). The
`)` ELSE `(` same-physical-line rule (§4.3) is enforced by the grammar shape
where `_else` follows `then` without an intervening `_newline`.

### 3.7 Control flow — FOR

| Node | Description | RHS sketch |
|------|-------------|-----------|
| `for` | any FOR variant | `prec(8, seq(ci('for'), optional($.for_option), field('var', $.for_variable), ci('in'), '(', field('set', $.for_set), ')', ci('do'), field('body', $._body)))` |
| `for_option` | `/D`, `/R [path]`, `/L`, `/F ["opts"]` | `choice(ci('/d'), seq(ci('/r'), optional($._argument)), ci('/l'), seq(ci('/f'), optional($.for_f_options)))` |
| `for_f_options` | the single quoted options string | `token(prec(10, /"(tokens|delims|skip|eol|usebackq|useback)[^"]*"/))` or a `string` |
| `for_variable` | `%%x` (batch) / `%x` (cmdline); one char | `token(/%%?[?@A-Z\[\\\]_`a-z{0-9]/)` |
| `for_set` | space/comma/semicolon-separated items, or backquote command, or quoted literal | `repeat(choice($._argument, $.backq_command, $.string, $._newline))` |
| `backq_command` | `` `command` `` (FOR /F command source) | `seq('`', repeat($._argument), '`')` |

The FOR variable must be `%`+exactly one char (post-substitution; source `%%x`
in batch). The `/F` options string must be tokenized with high precedence so it
is not mis-parsed as a generic quoted argument. FOR and IF may **not** have
leading redirections (enforced by grammar shape).

### 3.8 GOTO / CALL / labels

| Node | Description | RHS sketch |
|------|-------------|-----------|
| `goto` | `GOTO label` / `GOTO :EOF` | `seq(ci('goto'), field('target', choice($.eof_label, $.label_ref)))` |
| `call` | `CALL :label args` / `CALL file args` / `CALL command` | `seq(ci('call'), choice(seq(field('label',$.label_ref), repeat($._argument)), $.command))` |
| `eof_label` | `:EOF` special target | `token(prec(1, /:[eE][oO][fF]/))` |
| `label_ref` | a goto/call target | `seq(optional(':'), $._label_name)` |
| `label` | label *definition* line `:name` | `seq(optional($._lead_delims), ':', field('name', $._label_name), optional($._label_tail))` |
| `_label_name` (hidden) | name up to delimiter | `token(/[^ \t\r\n:,;=+&|<>]+/)` |
| `_label_tail` (hidden) | ignored trailing text on label line | `/[^\r\n]*/` |

Labels are statement-level (start of `_line`), with optional leading
delimiters stripped (`;:label`, `   :label`). Distinguish `::…` (comment, §3.9)
from `:name` (label) by what follows the first colon.

### 3.9 Comments

| Node | Description | RHS sketch |
|------|-------------|-----------|
| `comment` | REM line or `::` line | `choice($.rem_comment, $.colon_comment)` |
| `rem_comment` | `REM <text>` | `seq(ci_word('rem'), optional($._rem_body))` |
| `colon_comment` | `:: <text>` (degenerate label) | `seq(optional($._lead_delims), token(/::/), optional($._rem_body))` |
| `_rem_body` (hidden) | free text to (unescaped) EOL, surfacing `%VAR%`/`!VAR!` for highlight | `repeat(choice($._variable, /[^\r\n]/))` or external `_rem_body` |

`rem` requires a delimiter after it (`remxyz` is *not* a comment) — enforce with
`token.immediate` boundary or by matching `rem` as a keyword via `word`.

### 3.10 SET family

| Node | Description | RHS sketch |
|------|-------------|-----------|
| `set` | dispatch on option | `prec(8, seq(ci('set'), choice($.set_assign, $.set_arith, $.set_prompt, $.set_display)))` |
| `set_assign` | `[ "]NAME=VALUE[" ]` | `seq(optional('"'), field('name',$._set_name), '=', field('value', optional($._argument)), optional('"'))` |
| `set_arith` | `/A expression` | `seq(ci('/a'), field('expr', $.arith_expr))` |
| `set_prompt` | `/P NAME=prompt` | `seq(ci('/p'), field('name',$._set_name), '=', optional($._argument))` |
| `set_display` | `SET` / `SET pfx` (no `=`) | `optional($._argument)` |
| `arith_expr` | SET /A sub-language (see §4.7) | precedence-climbing expr of `arith_var`/`number`/ops |
| `arith_var` | bare identifier = env var (NO `%`) | `/[A-Za-z_][A-Za-z0-9_]*/` |

`SET /A`'s RHS is a **distinct expression sub-grammar**, not a generic argument.

### 3.11 Literals & shared

| Node | Description | RHS sketch |
|------|-------------|-----------|
| `_variable` (hidden) | any expansion reference | `choice($.var_immediate, $.var_delayed, $.param, $.param_tilde, $.all_params, $.for_var_ref)` |
| `var_immediate` | `%NAME%` w/ optional operator | `seq('%', $.var_name, optional($._var_op), '%')` |
| `var_delayed` | `!NAME!` w/ optional operator | `seq('!', $.var_name, optional($._var_op), '!')` |
| `param` | `%0`–`%9` | `token(/%[0-9]/)` |
| `all_params` | `%*` | `token(/%\*/)` |
| `param_tilde` | `%~mods[$ENV:]N` | `seq(token(/%~/), optional($.tilde_mods), optional($.tilde_pathsearch), $._param_or_forvar)` |
| `for_var_ref` | `%%x` reference in body | `token(/%%[?@A-Z\[\\\]_`a-z{0-9]/)` |
| `var_name` | env var name | `token.immediate(/[^%!:\r\n]+/)` (name stops per §4.2) |
| `_var_op` (hidden) | substring or substitution | `choice($.substring_op, $.substitute_op)` |
| `substring_op` | `:~start[,len]` | `seq(token.immediate(':~'), $._signed_int, optional(seq(',', $._signed_int)))` |
| `substitute_op` | `:[*]search=replace` | `seq(token.immediate(':'), optional('*'), $._search, '=', optional($._replace))` |
| `tilde_mods` | run of `dpnxfsatz` (case-insens) | `token(/[dpnxfsatzDPNXFSATZ]+/)` |
| `tilde_pathsearch` | `$ENV:` clause | `seq('$', $.var_name, ':')` |

---

## 4. Tricky areas — concrete plans

### 4.1 Caret escaping & line continuation

Two distinct mechanisms:

- **Line continuation** `^\n`: caret as last char before newline splices the
  next physical line. Model as `_line_continuation = token(/\^\r?\n[ \t]*/)` and
  allow it inside `_argument` and between fragments. Because cmd resolves this in
  phase 2 *before* tokenization, it can join *mid-token*; the regex token handles
  the common case, but the mid-token splice (`echo first^\nsecond` → one token
  `firstsecond`) is the canonical reason to promote this to the external scanner
  if the regex creates spurious node boundaries.
- **Mid-line escape** `^x`: makes the following metachar literal. Model the
  common cases as a fixed token `token(/\^[&|<>()^"%!]/)` inside arguments. The
  genuinely hard case — `^` before a **separator** (`^ `, `^,`) turning it into
  non-splitting literal text — is context-sensitive and is the prime
  external-scanner candidate (`_caret_escape`). Document that without the
  scanner, `^ ` may be approximated as `^` + space.

Caret-inside-quotes nuance (document as known imprecision): `^` is literal inside
`"…"` in phase 2, but under delayed expansion with a `!` on the line, phase 5
removes carets even inside quotes (the `^^!` idiom). A CF grammar cannot model
the phase-5 conditional pass; we accept `^^!` as two caret-seqs + `!` token.

### 4.2 Expansions: `%VAR%`, `!VAR!`, `%~mods`, substring, substitution

- **`%VAR%`** — name runs from after `%` up to the next `%` **or** to a `:` that
  is *not* immediately followed by the closing `%` (ReactOS scan). So `%VAR:%`
  is the variable `VAR` (the `:` ends the name), **not** a modifier. Implement
  `var_name` to stop at `%`, `!`, or `:` *unless* the char after `:` is the
  closing delimiter — easiest as a single `token.immediate` regex with a
  negative-lookahead-ish split, or two productions tried in order
  (`%VAR:%` before `%VAR:op%`).
- **`%%` collapse** — batch-context literal percent. Try `PERCENT_ESCAPE` (`%%`
  → literal) *before* `var_immediate`. Since `%%x` is also a FOR var, ordering:
  `param` (`%[0-9]`) → `all_params` (`%*`) → `param_tilde` (`%~`) →
  `for_var_ref` (`%%` + one var char) → `var_immediate` (`%name%`) → literal `%%`.
- **`!VAR!`** — same shape as `%VAR%` with `!` delimiters and the same operators.
  Always parsed (over-accept; document).
- **`%~mods[$ENV:]N`** — modifier letters `dpnxfsatz` (case-insensitive), optional
  `$ENV:` path-search clause, terminated by the param digit (`%~dp1`) or FOR var
  letter (`%%~fI`). The **greedy-then-backtrack ambiguity** (`%~dpnxg` depends on
  which FOR vars are in scope) is *unresolvable statically* — we cannot know the
  in-scope vars. Plan: lex modifiers greedily over `[dpnxfsatz]+`, then take the
  final char as the param/var. Accept that `%~dpnxg` will be parsed one fixed way
  regardless of scope; document the imprecision. `%~$PATH:1` handled by
  `tilde_pathsearch`.
- **Substring `:~start[,len]`** — `start`/`len` are signed ints (base-0). Model
  as `token.immediate(':~')` + signed-int + optional `,` + signed-int. No need to
  compute clamping (runtime).
- **Substitution `:search=replace`** — `:` (not `:~`, not `:` before delimiter),
  optional leading `*`, search (non-empty, no `=`), `=`, optional replace. `=`
  is the hard delimiter.

### 4.3 IF / ELSE — single-line vs block

- Single-line: `IF cond cmd` and `IF cond cmd ELSE cmd`.
- Block: `IF cond ( … ) ELSE ( … )`.
- **Hard rule**: the IF body's closing `)`, the `ELSE` keyword, and ELSE's
  opening `(` must be on the **same physical line**. A `)` alone on a line then
  `ELSE` next line is `ELSE was unexpected at this time.` Enforce by **not**
  allowing a `_newline` between `then` and `_else` in the rule. Because `_body`
  ends at `)` (no trailing newline consumed) and `_else` follows immediately,
  newline between them naturally terminates the `if` without an else, matching
  cmd's error surface (we accept the no-else form; an ELSE on the next line then
  parses as a stray/error node).
- `prec.right(8)` on `if` resolves dangling-else to nearest IF and lets
  `IF c1 IF c2 cmd` chain (nested IF is a command).
- IF may not carry leading redirections — not offered by the grammar (IF is a
  `_unit`, redirs attach only to `command`/`block`).

### 4.4 FOR bodies, `%%` vars, and AST shape note

- All variants share `FOR [opt] %%v IN (set) DO body`. Variants differ in
  `for_option` and how `for_set` reads (plain glob list / `/F` source which may
  be `(files)`, `("literal")`, or `` (`command`) ``).
- The IN list is read with `InsideBlock` so `)` ends it and inner newlines are
  skipped — model `for_set` as `repeat(choice($._argument, $.string,
  $.backq_command, $._newline))` inside the literal `(` … `)`.
- `for_variable` and `for_var_ref` are `%%`+one char in batch. Body references
  use `for_var_ref` and `param_tilde` with a letter terminator.
- **AST-shape note**: ReactOS builds a *right-leaning* operator tree (binary op
  recurses at the same level). We deliberately use **left-associative**
  tree-sitter precedence instead. Rationale: tree-sitter tooling and
  highlighting expect conventional left-assoc; observable left-to-right reading
  order is preserved; reproducing ReactOS's right-recursion would add complexity
  with no benefit to a static tool. This is a conscious divergence, documented.

### 4.5 Redirection binding & the leading-digit rule

- A leading digit is a redirection fd **only** at a token boundary **and**
  immediately followed by `<`/`>`. Boundary = digit is first char of token, or
  preceding char is a separator or one of `()&|"`. So `2>file` redirects,
  `echo 2>file` splits (`echo`, then `2>`), `abc2>file`/`hello2>file` keep `2`
  as text (modern cmd — do **not** swallow the trailing digit).
- Implement with `token.immediate`: the fd digit in `redirect_file`/`redirect_dup`
  is `token.immediate(/[0-9]/)` so it only binds when adjacent to the operator,
  and a digit *preceded by bareword* is consumed by `_bareword_frag` first
  (greedy), keeping it as text. A standalone `2` after whitespace followed
  immediately by `>` is offered to `redirect_file` because the bareword frag
  cannot include `>`.
- Redirections are collected as a `repeat` on `command`/`block`; ordering is
  preserved positionally in the tree (last-wins and stream-merge semantics are
  runtime). `<<` is lexable but a cmd error — we may either not offer it or offer
  it and let it be an error node.

### 4.6 REM and `::` comments

- `REM`: keyword (via `word` extraction) + delimiter + free body. The body bears
  `%VAR%`/`!VAR!` for highlighting but is otherwise opaque to EOL. A trailing
  `^\n` does **not** splice in `ParseRem` (continuations off), and `^` is
  consumed as literal — so `_rem_body` should *not* honor line continuation.
- `::`: degenerate label, statement-start, allows leading delimiters. Mark it as
  `colon_comment`. Document that `::` inside `(…)` blocks is unsafe in real cmd;
  a linter layer (not the grammar) may warn. The grammar still parses it.
- Inline `& rem …` falls out naturally: `&` separates, then `rem_comment` as the
  RHS command-position.
- Keep the Windows label-vs-comment distinction (not ReactOS's collapse).

### 4.7 SET /A sub-language

`SET /A`'s RHS is its own expression grammar — **not** a generic argument:

- Bare identifiers are env var **names** (no `%`); `arith_var = /[A-Za-z_]\w*/`.
- `%` is the **modulus** operator (written `%%` in a batch file) — must be a
  distinct token here, not a variable sigil.
- Operators (C-like): `= += -= *= /= %= &= |= ^= <<= >>=`, `+ - * / %`,
  `& | ^ ~`, `<< >>`, unary `- ~ !`, grouping `( )`, comma sequences. Bitwise
  `& | < > ^` collide with cmd's special-char tokenizer, so in raw source they
  are often caret-escaped/quoted — accept both escaped and quoted forms.
- Model with precedence climbing:

```js
arith_expr: $ => $._arith_comma,
_arith_comma: $ => choice(seq($._arith_assign, ',', $._arith_comma), $._arith_assign),
_arith_assign: $ => prec.right(1, choice(
  seq($.arith_var, choice('=','+=','-=','*=','/=','%%=','&=','|=','^=','<<=','>>='), $._arith_assign),
  $._arith_bitor)),
_arith_bitor:  $ => prec.left(2, ...),  // | ... down through ^, &, <<>>, +-, */%%, unary, primary
arith_primary: $ => choice($.number, $.arith_var, seq('(', $._arith_comma, ')')),
number: $ => token(/0[xX][0-9a-fA-F]+|0[0-7]*|[1-9][0-9]*/),  // base-0
```

Declare the `[$.block, $.set_arith_paren]` conflict so the GLR parser
distinguishes a grouping paren in a command block from an arithmetic paren.

---

## 5. Prior art

| Project | Approach | License | Verdict |
|---------|----------|---------|---------|
| **wharflab/tree-sitter-batch** | Pure `grammar.js`, **no external scanner**; `extras=[ \t]`; significant newlines; `token.immediate`; `ci()`/`kw()` helpers; `word: $.command_name` (`/[$a-zA-Z_0-9][$a-zA-Z0-9_.#-]*/`); `_line_continuation = token(/\^\r?\n[ \t]*/)`; REM `/[rR][eE][mM]/`; echo via `alias(kw('echo'), command_name)`; IF `prec.right(8)`, FOR/SET `prec(8)`; `conflicts: [parenthesized, paren_expression]`; ships `highlights.scm`; tolerates polyglot headers. ~12 stars, ~18 releases, v0.11.x. | MIT | **Primary architectural reference.** Adopt its config skeleton and helper patterns wholesale. Its proof that cmd is largely doable with no scanner sets our Milestone 1–6 plan. Reuse `highlights.scm` patterns and `ci()/kw()`. |
| **davidevofficial/tree-sitter-batch** | Has `grammar.js` + multi-language bindings but immature/naive. ~2 stars. | MIT | **Avoid as base.** May skim for test corpus ideas only. |
| **imDMG/tree-sitter-bat** | Early-stage. | (early) | **Avoid.** |
| **tree-sitter/tree-sitter-bash** | Canonical context-sensitive shell grammar with `src/scanner.c`: externals incl. `_concat`, `file_descriptor`, `variable_name`, heredoc family, `__error_recovery`; `extras=[comment,/\s/, /\\\r?\n/, …]`; `word: $.word`. | MIT | **Scanner-discipline reference** if we add `src/scanner.c`: copy the `_concat` technique, `eof()`-in-loops, `mark_end`, `serialize()` bounds, and the `__error_recovery` sentinel. Do **not** copy bash *string/quoting semantics* (cmd quotes are grouping-only, no single quotes, `%`/`!` expand inside quotes). |

Reuse: wharflab config + helpers + highlights; bash scanner *discipline*.
Avoid: copying bash string semantics; davidevofficial/imDMG as bases; ReactOS's
right-leaning AST shape and its `::`-as-comment collapse.

---

## 6. Implementation plan (incremental milestones)

Each milestone is independently testable via `tree-sitter test` corpus files
under `test/corpus/`.

**M1 — Skeleton: program / commands / words / comments / labels.**
`grammar.js` with `extras=[ \t]`, `word`, `_newline`, `program=repeat(_line)`,
`command` (name + bareword args), `string` (quoted), `quiet` (`@`),
`rem_comment`, `colon_comment`, `label`. No operators, no expansions.
Corpus: `command.txt`, `comment.txt`, `label.txt`, `quiet.txt`, `blank_lines.txt`.

**M2 — Operators & redirection.** Add `pipeline`/`and_list`/`or_list`/`seq_list`
with the precedence ladder; `block` (`(…)`); `redirection` (`redirect_file`,
`redirect_dup`, `file_descriptor`) with `token.immediate` fd binding and the
leading-digit boundary handling. Corpus: `operators.txt`, `pipeline.txt`,
`redirect.txt`, `redirect_dup.txt`, `block.txt`.

**M3 — Expansions.** Add `_variable` and all members: `var_immediate`,
`var_delayed`, `param`, `all_params`, `param_tilde` (+`tilde_mods`,
`tilde_pathsearch`), `for_var_ref`, `substring_op`, `substitute_op`, `%%` literal.
Wire `_variable` into `_argument` and `string`. Corpus: `var_immediate.txt`,
`var_delayed.txt`, `params.txt`, `tilde.txt`, `substring.txt`, `substitute.txt`.

**M4 — Control flow IF/ELSE.** `if`, `if_flag`, `not`, all `cond_*`,
`compare_op`, `_else` (same-line rule), `prec.right(8)`, IF chaining.
Corpus: `if_single.txt`, `if_block.txt`, `if_else.txt`, `if_chain.txt`,
`if_compare.txt`.

**M5 — Control flow FOR.** `for`, `for_option` (`/D /R /L /F`), `for_variable`,
`for_set`, `for_f_options`, `backq_command`. Corpus: `for_plain.txt`,
`for_l.txt`, `for_r.txt`, `for_d.txt`, `for_f.txt`, `for_f_backq.txt`.

**M6 — GOTO/CALL + SET family.** `goto`, `call`, `eof_label`, `label_ref`;
`set`, `set_assign`, `set_prompt`, `set_display`. Corpus: `goto.txt`, `call.txt`,
`set_assign.txt`, `set_prompt.txt`.

**M7 — SET /A arithmetic.** `arith_expr` precedence-climbing sub-grammar,
`arith_var`, `number`, the `[block, set_arith_paren]` conflict. Corpus:
`set_a.txt`, `set_a_modulus.txt`, `set_a_paren.txt`.

**M8 — Edge cases & (optional) external scanner.** Caret-before-separator,
mid-token line continuation, `%VAR:%` non-operator case, leading/positional
redirection, `^^!` idiom, `echo.`/`echo:`/`echo(` variants, polyglot headers.
Introduce `src/scanner.c` *only* for tokens that proved brittle. Add
`__error_recovery`. Corpus: `edge_caret.txt`, `edge_redirect.txt`,
`edge_echo.txt`, `edge_var.txt`.

**M9 — Highlights & bindings.** `queries/highlights.scm` (adapt wharflab),
`queries/injections.scm`, node-types stabilization, README.

---

## 7. Test-corpus plan

Use `tree-sitter test` (`test/corpus/*.txt`) for unit cases and
`tree-sitter parse` against real files for regression. Categories:

1. **Per-feature unit corpus** — one file per node family (see M1–M8 names),
   small focused inputs with expected S-expressions.
2. **Escaping/quoting matrix** — caret arithmetic (`^&`, `^^`, `^^^&`,
   `^^^^^&`), continuation joins, `^^!` under delayed expansion, unbalanced
   quotes (rest-of-line quoted), `%%` literal, `^"`.
3. **Expansion matrix** — every sigil and operator, the `%VAR:%` edge, the
   `%~dpnxg` greedy case (document chosen parse), `%~$PATH:N`, nested
   `%a%%b%` adjacency, `!VAR!` always-accepted.
4. **Redirection matrix** — `2>file`, `echo 2>file`, `hello2>file`, `>&`/`<&`,
   `> file 2>&1` vs `2>&1 > file` (tree only; semantics noted), leading
   `>out echo hi`, `9>nul`.
5. **Control-flow matrix** — IF single/block/else/chain/compare numeric-vs-string;
   FOR all five variants, `tokens=`/`delims=`/`skip=`/`eol=`/`usebackq`,
   tokens-to-variable consecutive assignment; ELSE same-line rule
   (positive + negative).
6. **Comment matrix** — `rem`/`REM=`/`rem;`/bare `rem`/`remxyz` (not a comment);
   `::`, `;::`, `   ::`; `& rem` inline; `::` inside block (parse + note).
7. **Negative/error cases** — empty `()`, `<<`, stray `)`, malformed
   `%~`/`%var:` (note: cmd aborts; we produce an error node).
8. **Real-world known-good corpus** (regression, not assertion):
   - Windows SDK / Visual Studio `vcvarsall.bat`, `vsdevcmd.bat` family
     (heavy SET/IF/FOR/CALL, redirection).
   - `gradlew.bat`, Maven `mvn.cmd`, Apache `*.bat` launchers, Node `npm.cmd`/
     `npx.cmd`, Python `activate.bat`, Conda `activate.bat`.
   - `git-for-windows` `*.bat`, ANGLE/Chromium build `.bat` scripts.
   - The dBenham/DosTips and ss64 example snippets (delayed-expansion,
     substring, FOR /F idioms).
   - Polyglot batch+PowerShell/VBScript headers from real `.cmd` wrappers.
   Run `tree-sitter parse --quiet` over a checked-in fixtures dir and gate CI on
   zero ERROR nodes (allowing a documented allowlist for genuinely cmd-invalid
   files).

---

## 8. Open questions / risks

1. **Phase-order imprecision (fundamental).** A CF grammar parses *unexpanded*
   source; `%VAR%` content that injects/removes operators or quotes is invisible.
   Accepted limitation; cannot be fixed without an evaluator.
2. **`!VAR!` over-acceptance.** We always parse delayed refs; without tracking
   `SETLOCAL ENABLEDELAYEDEXPANSION` we cannot know `!` is literal. Risk: false
   variable nodes in non-delayed scripts. Mitigation: highlighting only; optional
   lint pass could downgrade.
3. **`%~dpnxg` greedy-backtrack.** Resolution depends on in-scope FOR vars —
   statically unknowable. We pick one fixed parse; document it. Risk: occasional
   mis-segmentation of modifier vs trailing literal.
4. **Caret-before-separator.** `^ `/`^,` rebinding a separator into literal text
   is the hardest CF case; the fixed-token approximation may mis-split. Decision
   needed: ship the approximation (M1–M7) and promote to external scanner in M8,
   or write the scanner earlier. Recommendation: defer to M8.
5. **Mid-token line continuation.** `echo first^\nsecond` should yield one token;
   regex `_line_continuation` may create a node boundary. Validate in M2; promote
   to scanner if it produces spurious structure.
6. **ELSE same-line enforcement.** Encoding "`)` ELSE `(` on one physical line"
   purely via rule shape (no `_newline` between `then` and `_else`) needs
   validation that the negative case (ELSE on next line) yields a clean error
   node, not a misparse of the whole file.
7. **Batch vs command-line dialect.** We commit to batch (`%%x`, `%N`, `%%`→`%`).
   If interactive `.cmd` snippets appear in corpora, single-`%` FOR vars may
   misparse. Decide whether to accept both (adds ambiguity) or document
   batch-only.
8. **`SET /A` `%` collisions.** Modulus `%`/`%%` and bitwise `& | ^ < >` inside
   the expression clash with cmd's outer tokenizer; raw source escapes/quotes
   them inconsistently. The arith sub-grammar must accept escaped, quoted, and
   bare forms — risk of ambiguity with the surrounding command grammar; rely on
   the declared conflict + `/a` keyword anchor.
9. **`::` inside blocks.** Real cmd breaks on `::` inside `(…)`; we parse it
   anyway. Whether to emit a warning is a linter concern, but the grammar must at
   least not cascade-fail the rest of the file.
10. **Empty `&` RHS and stray `)`.** ReactOS allows empty `&` RHS and silently
    swallows stray `)` (GOTO-into-block hack). Decide how lenient to be vs
    producing error nodes; recommendation: allow empty `&` RHS, treat stray `)`
    as an error node (it is rare and a real bug magnet).
11. **External-scanner scope creep.** If too many bodies (echo/rem/label) move to
    the scanner, complexity and serialization risk grow. Keep the scanner minimal
    (`_concat`, `_caret_escape`, `__error_recovery`) and prefer grammar regexes.
