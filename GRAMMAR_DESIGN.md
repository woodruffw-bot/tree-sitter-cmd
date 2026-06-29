# tree-sitter-cmd grammar design

This document explains how the grammar is built and why. It is grounded in the
ReactOS `cmd.exe` reimplementation (`base/shell/cmd/parser.c`, `cmd.c`,
`batch.c`, `for.c`, `if.c`), the dBenham/jeb batch-line-parser phase model
(DosTips t=3587), the ss64 and Microsoft Learn references, and prior tree-sitter
work (wharflab/tree-sitter-batch, tree-sitter-bash).

The grammar targets the Windows `cmd.exe` batch dialect. It is a recognizer and
a highlightable concrete syntax tree, not an executable AST. The target scope is
`source.dosbatch`, file types `.bat` and `.cmd`. License: MIT.

## 1. Scope

In scope:

- Simple commands (name plus argument tail) and the `@` quiet prefix.
- Sequencing and boolean operators `&`, `&&`, `||`, and pipes `|`.
- Redirections `>`, `>>`, `<`, `N>`, `N>>`, `N<`, and handle duplication `N>&M`.
- Parenthesized command blocks, multi-line, with attached redirections.
- Control flow: `IF`/`ELSE`, `FOR` (plain, `/D`, `/R`, `/L`, `/F`), `GOTO`,
  `CALL`, and labels.
- Comments: `REM` and `::`.
- Expansions: `%VAR%`, `%N`, `%*`, `%~mods` (immediate), `!VAR!` (delayed),
  `%%x` (FOR variables), and the `:~off,len` substring and `:search=replace`
  substitution operators.
- `SET`, `SET /A`, and `SET /P`.
- Caret escaping `^x`, caret line continuation `^\n`, and double-quoted strings
  (grouping only, since cmd does not strip quotes).

Out of scope (runtime behavior, not syntax):

- Variable values, environment state, and filesystem globbing results.
- The receiving program's `argv` parsing (`CommandLineToArgvW`), which is a
  separate grammar downstream of cmd.
- Runtime semantics of `&&`/`||` gating, `GOTO` target resolution, `SHIFT`
  renumbering, and `SETLOCAL` scoping.

Two deliberate conformance choices:

- **Batch dialect, not interactive.** `%%` collapses to `%`, positional
  parameters are `%1`, and FOR variables are `%%x`. The interactive single-`%`
  FOR-variable dialect is not a goal.
- **Over-accept runtime-gated constructs.** `!VAR!` is always parsed as a
  delayed reference even though it is literal text unless
  `SETLOCAL ENABLEDELAYEDEXPANSION` is active, because that state cannot be
  tracked statically.

The grammar keeps the Windows label-vs-comment distinction: `:name` is a jump
target and `::...` is a comment label. ReactOS collapses both into "comment";
this grammar does not.

## 2. Why cmd is hard to parse

`cmd.exe` has no whole-file grammar. Each logical line is read, expanded,
parsed, and executed before the next line is read. The grammar still models the
whole file as one tree because a static tool wants a tree for the entire buffer.

The deeper problem is that cmd runs each line through an ordered pipeline, and
the same byte means different things in different phases:

| Phase | Action |
|------:|--------|
| 1 | Percent (`%`) expansion: `%%`→`%`, `%N`, `%*`, `%~`, `%VAR%` |
| 1.5 | Strip `\r` (CRs never separate or store) |
| 2 | Caret/quote tokenization: `^` removed, `^\n` continuation, recognize operators, detect REM/IF/FOR |
| 4 | FOR `%%x` loop-variable substitution |
| 5 | Delayed (`!`) expansion, a second caret-removal pass that ignores quote state |

The consequences that drive the grammar design: percent is resolved before
caret, delayed `!` is resolved after caret in a separate pass, and `\r` is
invisible.

A tree-sitter grammar is a single context-free pass, so it cannot reproduce this
phase ordering. It approximates the surface syntax and accepts the resulting
imprecision (see Limitations). The specific difficulties:

1. **Context-sensitive tokenization.** The same characters lex differently
   depending on quote state, block depth, whether a digit sits at a token
   boundary (redirection vs text), and which FOR variables are in scope.
2. **Separators are not universal whitespace.** `, ; =` act like spaces in most
   contexts but not in IF's left operand (where `=` must survive for `==`) or in
   a command's argument tail. They cannot go in `extras`.
3. **Caret rebinds the next character**, including separators and newlines, and
   it does so before tokenization. This is the main reason an external scanner
   is needed.
4. **Expansion happens before parsing.** `%VAR%` content can introduce or
   remove operators and quotes invisibly. The grammar parses the unexpanded
   source.

## 3. Lexing: grammar.js vs the external scanner

Most of the grammar lives in `grammar.js` with `[ \t]`-only extras, significant
newlines, `token.immediate` for tight expansion lexing, case-insensitive keyword
helpers, and precedence to resolve command/operator/control-flow ambiguity.
Keyword extraction (`word: $._cmd_text`) makes `IF`/`FOR`/`SET` match robustly
against barewords, so a file named `if` still works as an argument. Keywords are
aliased to the named `keyword` node so they appear in the tree and highlight via
`(keyword)`. `_cmd_text` ends a bareword at `:` — cmd ends an internal-command
name there (and at `.\,/;=[]`), so `goto:eof`/`call:label` are `goto`/`call`
plus a `:label` argument — with a leading-drive-letter exception (`C:`,
`C:\tools\foo.exe`) so command paths still parse.

`extras` is `[/[ \t]/, token(/\^\r?\n/)]`. The line continuation is an anonymous,
invisible extra, the same approach tree-sitter-bash uses for `\\\n`: it produces
no node and is transparent to word adjacency, so `echo a^\nb` joins into one word
while `echo a ^\nb` stays two arguments. Newline is a statement terminator and
stays structural. `\r` is swallowed by matching every newline as `/\r?\n/`.

The external scanner (`src/scanner.c`) owns the genuinely context-sensitive
tokens. Its only state is a single block-depth counter:

| Token | Role |
|-------|------|
| `CONCAT` | zero-width join of adjacent word fragments into one argument |
| `REM` | the `rem` keyword as a whole word (tree-sitter keyword extraction declines `rem`) |
| `BLOCK_OPEN` / `BLOCK_CLOSE` | `(`/`)` that open and close a structural block |
| `LPAREN` / `RPAREN` | a literal `(`/`)` that does not affect block nesting |
| `CARET_ESCAPE` | a lone `^` that escapes a following `%`/`!` expansion |

`=` is a word boundary in the scanner so `CONCAT` cannot starve `==` or
`name=value`.

## 4. The parenthesis model

This is the most distinctive part of the design. cmd does not balance
parentheses with a stack of kinds; it tracks a single block-nesting depth.

- `(` begins a block only at the start of a command or SET, where the grammar
  offers `BLOCK_OPEN` through `valid_symbols`. In an argument, `(` is a literal
  `LPAREN` that does not increase the depth.
- While a block is open (`depth > 0`), the first unescaped `)` is `BLOCK_CLOSE`
  and ends the block. A literal `(` in an argument never protects a later `)`.

This mirrors ReactOS `parser.c`, where `(` only begins a block at a token start
and an in-block `)` always ends the token. It is why cmd needs `^)` to echo a
close-paren inside a block, and why `(echo()` is a block whose body is the
command `echo(`. Concretely, this makes `echo (text)`, `echo.version(s)`, and
the `(echo()` blank-line idiom all parse the way cmd runs them.

## 5. Tricky areas

### Caret escaping and line continuation

Two distinct mechanisms:

- **Line continuation `^\n`** splices the next physical line. cmd resolves this
  before tokenization, so it is purely lexical and must be transparent to word
  boundaries. It is modeled as the anonymous extra described in section 3.
  Residual imprecision: a mid-word caret before an indented next line
  (`echo a^\n   b`) joins to `ab`, where cmd would produce `a   b`. The common
  `arg ^\n   arg` form (space before the caret) is unaffected. A *dangling*
  continuation — a caret at the end of the file, with no following line to splice
  onto — is an error node, since the invisible extra has nothing to splice onto.
  Continuation onto a following line, blank or not, is fine.
- **Mid-line escape `^x`** makes the following metacharacter literal. The common
  cases are a fixed token. A caret before a `%`/`!` expansion is the scanner's
  `CARET_ESCAPE`: in cmd `^%VAR%` expands `%VAR%` first and the caret escapes the
  result, so the caret must not swallow the `%`/`!`.

Caret inside quotes is a known imprecision: `^` is literal inside `"..."` in
phase 2, but phase 5 removes carets even inside quotes when the line contains a
`!` (the `^^!` idiom). A context-free grammar cannot model that conditional pass.

### Expansions

- **`%VAR%`** name runs from after `%` up to the next `%`, or to a `:` that is
  not immediately followed by the closing `%`. So `%VAR:%` is the variable `VAR`
  (the `:` ends the name), not a modifier.
- **Ordering** of the percent forms is `%N` → `%*` → `%~` → `%%x` (FOR var) →
  `%name%` → literal `%%`, so each maximal-munch form wins cleanly.
- **`%~mods[$ENV:]N`** lexes the modifier letters `dpnxfsatz` greedily, then
  takes the final character as the parameter or FOR variable. The
  greedy-then-backtrack ambiguity (`%~dpnxg` depends on the in-scope FOR vars) is
  statically unknowable, so the grammar picks one fixed parse.
- **Substring `:~start[,len]`** and **substitution `:[*]search=replace`** are
  modeled directly; `=` is the hard delimiter for substitution.

### Strings

Double quotes group only; cmd does not strip them, and `%VAR%`/`!VAR!` still
expand inside them. The `string` node sub-nodes its interior: literal text and a
lone `%`/`!` are hidden tokens that the `string` node covers, while a real
`%VAR%`/`!VAR!` is the same expansion node used elsewhere. So `"%PATH%"` is a
`string` containing a `variable`, and a query or highlighter sees the expansion
through the quotes.

The terminator is the external `_string_end` token, not an optional `"`. The
scanner consumes a closing `"`, or matches zero-width at end of line / end of
input so an unterminated quote still closes (cmd runs an open quote to end of
line). An optional `"` would make a lone `"` a valid empty string, and since a
quote is not a word boundary the `CONCAT` adjacency join would then parse
`"%PATH%"` as two empty strings around a bare `%PATH%` rather than one string
with an interior expansion. The explicit terminator removes that ambiguity: a
quote can only end a string through `_string_end`, so the interior is always
taken into the string. A quote inside a `%VAR:"=%` style substitution stays part
of the single `variable` token (the lexer matches the whole `%...%` first), so
those do not affect string termination.

### IF / ELSE

Single-line (`IF cond cmd [ELSE cmd]`) and block (`IF cond ( ... ) ELSE ( ... )`)
forms are both supported. cmd requires the IF body's closing `)`, the `ELSE`
keyword, and ELSE's opening `(` to be on the same physical line; a `)` alone on a
line followed by `ELSE` on the next is an error. This is enforced by not allowing
a newline between the `then` body and the `else` branch. `prec.right` resolves
the dangling-else to the nearest IF and lets `IF c1 IF c2 cmd` chain. IF does not
take leading redirections.

### FOR

All variants share `FOR [opt] %%v IN (set) DO body`. The IN list is read inside a
block so `)` ends it and inner newlines are skipped. A FOR variable is `%%` plus
any single non-separator character (`%%#`, `%%1` are valid). `FOR /L` accepts the
non-comma numeric separators `;`, `=`, and space, e.g. `(1;1=5)`. `/R` and `/F`
share one optional-argument path because of a lexer-state constraint, so they are
unified behind a single `for_flag` rule (this is the one declared conflict). The
`/F` command source can be `` `backquoted` `` or `'single-quoted'`; both are one
token (`backquote_string` / `single_quote_string`) so inner `)`/operators stay
literal, and both are cmd injection points.

### Redirection

A leading digit is a redirection fd only at a token boundary and immediately
followed by `<`/`>`. So `2>file` redirects, `echo 2>file` splits into `echo` then
`2>`, and `abc2>file` keeps `2` as text. The fd digit is `token.immediate`
against the operator, and a digit preceded by a bareword is consumed as text
first. Redirection ordering is preserved positionally; last-wins and stream-merge
semantics are runtime.

### REM and ::

`REM` is a whole-word keyword followed by a delimiter and a free body to
end-of-line. The body surfaces `%VAR%`/`!VAR!` for highlighting but is otherwise
opaque, and does not honor line continuation (cmd's `ParseRem` does not splice).
`::` is a degenerate label used as a comment. It is a statement, so it is
accepted both at the start of a line and after an operator, matching the common
`dir &:: note` inline-comment idiom (the same position the `& rem` form already
worked in). Like `REM` it runs to end of line, so any later `&`/`|` is part of
the comment. `::` inside a block is unsafe in real cmd, but the grammar parses it
without cascading; a linter layer could warn.

### SET /A

`SET /A`'s right-hand side is arithmetic, where `%` is the modulus operator
(written `%%` in a batch file) and bitwise `& | ^ < >` collide with cmd's outer
tokenizer (so source often caret-escapes or quotes them). The grammar captures
the expression as a generic argument tail rather than a full arithmetic
sub-grammar; see Limitations.

## 6. Node taxonomy

The named nodes group into these families (see `src/node-types.json` for the
full list):

- **Top level**: `program`, `command`, `quiet`, `command_name`.
- **Operators**: `pipeline`, `and_list`, `or_list`, `seq_list`.
- **Redirection**: `redirection`, `redirect_file`, `redirect_dup`,
  `file_descriptor`.
- **Blocks**: `block`.
- **Control flow**: `if` (with `if_flag`, `not`, the `cond_*` tests,
  `compare_op`), `for` (with `for_option`, `for_variable`, `for_set`,
  `backq_command`), `goto`, `call`, `label`.
- **SET**: `set`, `set_assign`, `set_prompt`, `set_display`.
- **Expansions**: `var_immediate`, `var_delayed`, `param`, `all_params`,
  `param_tilde`, `for_var_ref`, with `substring_op` and `substitute_op`.
- **Comments**: `rem_comment`, `colon_comment`.

Operators use conventional left-associative tree-sitter precedence (cmd binding,
loosest to tightest: `&` < `||` < `&&` < `|`). This diverges from ReactOS's
right-leaning operator tree, which would add complexity with no benefit to a
static tool; observable left-to-right reading order is preserved either way.

## 7. Prior art

- **wharflab/tree-sitter-batch** (MIT): the primary architectural reference. A
  pure-`grammar.js` cmd grammar with no external scanner, proving most of cmd is
  doable that way. This grammar borrows its config skeleton and helper patterns.
- **tree-sitter/tree-sitter-bash** (MIT): the scanner-discipline reference. The
  `CONCAT` technique, gating every token on `valid_symbols`, calling `eof()` in
  every loop, and keeping `serialize()` small all come from here. Bash string and
  quoting semantics are deliberately not copied, since cmd quotes are grouping
  only and `%`/`!` expand inside them.

## 8. Limitations

These follow from cmd being phased and context-sensitive. None cascade on valid
scripts.

- **Phase-order imprecision** is fundamental: a context-free grammar parses
  unexpanded source, so `%VAR%` content that injects or removes operators or
  quotes is invisible.
- **`!VAR!` over-acceptance**: delayed references are always parsed, even where
  delayed expansion is not enabled and `!` is literal at runtime.
- **`%~dpnxg` greediness**: the modifier-vs-literal split depends on in-scope FOR
  variables, which is statically unknowable, so one fixed parse is chosen.
- **`SET /A` expressions** are a generic argument tail, not an arithmetic
  sub-grammar.
- **Linefeed-named variables** (`%LF%` macros built by a caret/`%LF%` dance to
  fold multi-line code onto one logical line) are not supported. This is a
  torture-test trick rather than mainstream batch.
- **Dangling caret continuation** (a caret at the end of the file, with no
  following line to splice onto) is an error node; continuation onto a following
  line, blank or not, is fine.
- **Tree-shape imprecisions that still parse cleanly**: `%%` outside a FOR is
  noded as a `loop_variable` (the documented batch-`%%x` vs `%%`-literal
  ambiguity), and the `if (%1)==()` idiom emits the parens as sibling `argument`
  nodes rather than one wrapped operand. Neither produces an error.
