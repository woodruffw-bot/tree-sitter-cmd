# tree-sitter-cmd grammar design

This document explains how the grammar is built and why. It is grounded in the
ReactOS `cmd.exe` reimplementation (`base/shell/cmd/parser.c`, `cmd.c`,
`batch.c`, `for.c`, `if.c`), the dBenham/jeb batch-line-parser phase model
(DosTips t=3587), the ss64 and Microsoft Learn references, and prior tree-sitter
work (wharflab/tree-sitter-batch, tree-sitter-bash).

The grammar targets the Windows `cmd.exe` batch dialect. It is a recognizer and
a concrete syntax tree for static analysis, not an executable AST. Stable CST
shape, source fidelity, and useful error recovery take priority over syntax
highlighting. Highlight and injection queries are secondary consumers and must
not drive grammar structure. The target scope is `source.dosbatch`, file types
`.bat` and `.cmd`. License: MIT.

## 1. Scope

In scope:

- Simple commands (name plus argument tail) and the low-precedence `@` quiet
  operator.
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

The grammar does not parse option languages belonging to invoked programs. It
also keeps cmd built-in option payloads opaque unless an option materially
changes cmd's own syntax. FOR `/R` is grammar-relevant because it may consume a
path before the loop variable. FOR `/F usebackq` is grammar-relevant because it
changes which quote delimiter marks a command source. Other `/F` parsing
keywords, including `tokens=`, `delims=`, `skip=`, and `eol=`, remain text in a
single argument node.

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

### Input encoding

The grammar consumes decoded characters. It does not detect a batch file's
encoding or choose an OEM code page. Those decisions belong to the caller
because they depend on file metadata, the active console code page, or other
external context.

The default Tree-sitter input API expects UTF-8. Tree-sitter also provides
UTF-16LE, UTF-16BE, and custom-decoder input APIs. Callers may use those APIs to
keep offsets in the original encoded buffer, or transcode to UTF-8 before
parsing. After transcoding, CST byte ranges refer to the UTF-8 buffer, not the
original file. A caller that needs both must retain an offset map.

A leading Unicode byte-order mark is handled by Tree-sitter's lexer. Mixed
encodings have no implicit recovery policy: callers must reject them or
normalize them before parsing. The Rust real-world fixture harness deliberately
accepts UTF-8 files only so it does not silently test an unknown decoder.


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
against barewords. Keywords are aliased to the named `keyword` node so they
appear in the tree and highlight via `(keyword)`. `_cmd_text` ends a bareword at
cmd's internal-command delimiters (`:.\,/;=[]`), so `goto:eof`, `call:label`,
and `set/a` recognize the command without requiring a space. `_cmd_path` keeps
dotted executable names and explicit paths such as `if.exe` from being mistaken
for internal commands. A leading drive letter also stays intact. A hidden
`_standard_separator` consumes `,`, `;`, and `=` in parser slots where cmd has
already established a token boundary. Horizontal space remains an extra. IF
modifiers, unary condition keywords, and FOR switches instead use
`_required_standard_separator`, an immediate hidden token that proves a source
delimiter was present. Textual IF comparison operators use
`_required_if_operator_separator`: whitespace, comma, or semicolon must end the
operator token, after which the remaining standard separators may include
equals. Ordinary command and SET value tails keep the same punctuation as text.

`extras` contains only `/[ \t]/`. A caret-newline is not transparent. During
`ParseTokenEx`, cmd discards the newline and treats the first character of the
next physical line as a forced literal. The grammar therefore represents
`^\nX` as one `escape_sequence` node. `echo a^\nb` remains one argument, while
the space in `echo a ^\nb` ends the first argument. In both forms, the node
retains the caret, newline, and forced character. A normal unescaped newline is
a statement terminator and stays structural. The forced character may itself
be a newline, as in the common `SET LF=^\n\n` macro. `\r` is included only as
part of the grammar's `/\r?\n/` newline spelling.

The external scanner (`src/scanner.c`) owns the genuinely context-sensitive
tokens. Its state is a block-depth counter plus a two-step missing-body boundary
counter:

| Token | Role |
|-------|------|
| `CONCAT` | zero-width join of adjacent word fragments into one argument |
| `STANDARD_CONCAT` | the same join, but stops at cmd's `,`, `;`, and `=` separators |
| `REDIRECT_TARGET_SEPARATOR_AHEAD` | selects a separator-aware filename without consuming source bytes |
| `REM` | the `rem` keyword as a whole word (tree-sitter keyword extraction declines `rem`) |
| `REM_TEXT` | the opaque body of a `REM` comment through end of line |
| `REDIRECT_SOURCE` | a file descriptor digit immediately followed by `<` or `>` |
| `BLOCK_OPEN` / `BLOCK_CLOSE` | `(`/`)` that open and close a structural block |
| `LPAREN` / `RPAREN` | a literal `(`/`)` that does not affect block nesting |
| `CARET_ESCAPE` | `^` or `^\n` before a `%`/`!` expansion; the sigil stays in the following expansion node |
| `STRING_END` | the terminator of a double-quoted string: a closing `"`, or zero-width at end of line / input |
| `SET_BINDING_END` | zero-width confirmation that a redirected unquoted SET name is followed by its real `=` delimiter |
| `BODY_BOUNDARY` / `BODY_BOUNDARY_AGAIN` | two zero-width line-boundary markers used only when an IF/ELSE/FOR body is absent |
| `COMMAND_START` | deliberately unavailable after those markers, producing a genuine anonymous MISSING `"command"` error |
| `ERROR_SENTINEL` | an unused final token that detects Tree-sitter's all-symbol error-recovery state |

`=` is a word boundary in the scanner so `CONCAT` cannot starve `==` or
`name=value`. `STANDARD_CONCAT` also stops at `,` and `;`. It is used by the
separator-aware word rules, while ordinary arguments continue across that
punctuation. An argument-specific immediate token joins an adjacent `=` after
another fragment, as in `%VAR%=suffix`, without changing that global boundary.
During error recovery, the scanner declines zero-width tokens but still emits a
real `BLOCK_CLOSE`. This keeps an error inside a block from consuming later
commands. Missing controller bodies are the exception before recovery begins:
the two boundary markers are aliased to anonymous implementation terminals, and
the required `COMMAND_START` is never emitted. The named CST therefore contains
no normal placeholder node: the body is a genuine MISSING `"command"`, and the
next physical line remains a separate statement. Two markers make that local
recovery cheaper than skipping the boundary even when a file has several
missing bodies.

## 4. The parenthesis model

This is the most distinctive part of the design. cmd does not balance
parentheses with a stack of kinds; it tracks a single block-nesting depth.

- `(` begins a block only at the start of a command or SET, where the grammar
  offers `BLOCK_OPEN` through `valid_symbols`. In an argument, `(` is a literal
  `LPAREN` that does not increase the depth.
- While a block is open (`depth > 0`), the first unescaped `)` is `BLOCK_CLOSE`
  and ends the block. A literal `(` in an argument never protects a later `)`.
- An unescaped `(` adjacent to an existing argument fragment stays in that
  same argument, including inside a block. This joining is deliberately scoped
  to arguments: a command-position `(` still begins a block, and `(echo()`
  remains the blank-line ECHO idiom rather than a command named `echo(`.

This mirrors ReactOS `parser.c`, where `(` only begins a block at a token start
and an in-block `)` always ends the token. It is why cmd needs `^)` to echo a
close-paren inside a block, and why `(echo()` is a block whose body is the
command `echo(`. Concretely, this makes `echo (text)`, `echo.version(s)`, and
the `(echo()` blank-line idiom all parse the way cmd runs them.

## 5. Tricky areas

### Quiet statements

`@` is a low-precedence unary operator over the complete statement that
follows. For example, `@echo one & echo two` suppresses echo for the full
sequence, not only for its left command. The CST represents this with a
source-backed `quiet_statement` containing `quiet` and `body` fields. Stacked
prefixes remain nested wrappers, so `@@echo off` retains both source operators.
An absent body is genuine parser recovery and does not adopt the next physical
line. Label definitions are not executable statements, so `@:label` keeps its
existing `quiet` field directly on the `label` node.

### Caret escaping and line continuation

Two distinct mechanisms:

- **Line continuation `^\nX`** discards the physical newline and forces `X` to
  be literal. The grammar keeps all three source parts in one `escape_sequence`
  node. This distinction matters for operators. In `^\n&&`, the first `&` is
  literal and the second is a single command separator. In `^\n|`, the pipe is
  literal and does not begin a pipeline. A continuation without a following
  character remains an error.
- **Mid-line escape `^x`** makes the following character literal. A caret before
  a `%`/`!` expansion is the scanner's `CARET_ESCAPE`. Percent expansion runs
  before caret handling, while delayed expansion runs later, so the caret must
  not swallow the expansion's opening sigil. The scanner applies the same split
  to a continued caret directly before `%`/`!`.

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
- **Substring `:~start[,len]`** and **substitution `:[*]search=replace`** sit
  inside the single `%…%` / `!…!` token, so they parse without error but are not
  broken into sub-nodes.

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

Quoted `SET "name=value"` and `SET /P "name=prompt"` bindings use a separate
last-quote terminator. Quotes before the last one are retained as value text,
while the last quote closes the wrapper. This keeps `name`, `value`, and
`prompt` fields stable without hiding expansions. Unquoted and quoted binding
names also retain expansion children, which represents computed names such as
`%~1r` and `_nt!nt!` without reparsing a leaf token.

The same wrapper is accepted for a no-`=` display prefix (`SET "PATH"`). Text
after its final quote is preserved as an opaque `set_ignored_suffix` field on
quoted assignments, prompts, and displays: cmd truncates the parameter at the
last quote, so that suffix is source text but is not part of the binding value.

`SET` also accepts caret-escaped wrapper quotes, as in
`set ^"macro=call helper^"`. These are represented as one
`caret_quoted_string`. They do not use normal string grouping because the carets
make the quote characters literal during cmd tokenization. A caret-escaped
closing wrapper ends the node immediately, so later operators and commands stay
outside the assignment.

Redirections inside a quoted binding's ignored suffix remain positional
`redirect` fields, while a terminal redirect stays on `set_statement`.

### IF / ELSE

Single-line (`IF cond cmd [ELSE cmd]`) and block (`IF cond ( ... ) ELSE ( ... )`)
forms are both supported. cmd requires the IF body's closing `)`, the `ELSE`
keyword, and ELSE's opening `(` to be on the same physical line; a `)` alone on a
line followed by `ELSE` on the next is an error. This is enforced by not allowing
a newline between the `then` body and the `else` branch. `prec.right` resolves
the dangling-else to the nearest IF and lets `IF c1 IF c2 cmd` chain. IF does not
take leading redirections. The consequence and alternative each consume a full
command-operator expression. For example, both commands in
`IF 1==1 ECHO a & ECHO b` belong to the consequence. An `ELSE` after the
consequence starts the alternative instead of becoming part of that expression.
`/I` is recognized only as a complete standard token. In
`IF /ileft==right ...`, the longer `/ileft` word remains the comparison's left
operand rather than becoming an `/I` flag plus `left`.
An absent consequence or alternative records a MISSING `"command"` at the
controller's own line boundary; it never becomes an empty `command_name` or
adopts the following physical line.
The separator slot immediately before a binary comparison operator accepts only
`,` and `;`, not `=`. This matches cmd's special token fetch: `=` must stay
available for the two-byte `==` operator. A textual operator must also end at a
real whitespace, comma, or semicolon delimiter. Once that delimiter ends the
operator token, standard separators, including `=`, are skipped before the
right operand. Thus `IF left equright ...` and `IF left equ=right ...` do not
become `EQU` comparisons. Compact `==` comparisons remain a separate path.
Their attached right operand does not yet preserve an additional `=` byte, so
sources such as `IF b===b ...` are a known CST limitation.

### FOR

All variants share `FOR [opt] %%v IN (set) DO body`. The body consumes a full
command-operator expression, so every command in `DO ECHO %%v & ECHO done`
runs for each iteration. A missing body uses the same line-local MISSING
`"command"` recovery as IF. The IN list is read inside a block so `)` ends it and
inner newlines are skipped. A `loop_variable_declaration` is exactly `%%` plus
one permitted binder (`%%#`, `%%0`, and `%%@` are valid). Modifiers such as
`~f` are accepted only on `loop_variable` references. The plain `%%x` terminal
is shared between those CST roles so an outer-loop reference can begin a `/R`
path (for example, `%%a\sub`) without stealing the following binder. `FOR /L`
accepts the non-comma numeric separators `;`, `=`, and space, e.g. `(1;1=5)`.
These separators split the set into three `argument` nodes just as commas do.
`/D` and `/R` may be combined as `/D /R [path]`, pathless `/R /D`, or
`/R path /D`. Windows rejects `/R /D path`: in that order an explicit root
cannot follow `/D`. Other mixed switch sets remain syntax errors. Every switch
is a complete standard token, so `/ffoo`, `/rC:\src`, and `/d/r` do not become
`/F`, `/R path`, or `/D /R` forms. `/R` and `/F` each accept one optional,
separator-delimited argument. A slash-leading word is not an ordinary argument,
so an illegal second switch is not hidden as an option or path.

`/F` command quoting depends on its options. By default, `'single-quoted'` is a
command and backquotes are literal. With `usebackq`, `` `backquoted` `` is a
command and single quotes are literal. The `backquote_string` and
`single_quote_string` nodes keep inner `)` and operators literal. Their content
nodes are delimiter-free but neutral. The injection query checks the `/F` option
text and captures only the command form for the active quote mode. This avoids
assigning command semantics to the inactive delimiter and avoids range
adjustment directives that the Rust highlighter does not apply. A
single-quoted source may span lines. Apostrophes inside a double-quoted span are
part of the source. An error-recovery pattern keeps an unterminated `usebackq`
command injectable while it is being edited.

### GOTO and CALL

`GOTO` requires a `label_reference` target and `CALL` requires an `argument`
target. Redirections may occur before those targets, but never replace them.
Bare or redirection-only forms therefore retain genuine Tree-sitter `ERROR` or
MISSING state; they are not accepted as targetless statement nodes. Recovery
also stops at the physical line boundary so a following command is not adopted
as the missing target.

### Redirection

A leading digit is a redirection fd only at a token boundary and immediately
followed by `<`/`>`. So `2>file` redirects, `echo 2>file` splits into `echo` then
`2>`, and `abc2>file` keeps `2` as text. Redirection filenames use cmd's standard
separators, so punctuation before a filename is skipped and punctuation after
it ends the target. The operator after an fd is
`token.immediate`, and the external scanner only emits the fd when the next
byte is `<` or `>`. It checks this before joining adjacent fragments, so
`echo "text"2>file` keeps `2` as the redirection source. A spaced digit remains
an ordinary argument, as in `echo 2 >file`. A duplication operator skips cmd's
horizontal and standard separators (`space`, tab, `,`, `;`, `=`) before its
target, so `2>& ,;=1` has the same `source`, `operator`, and `target` fields as
`2>&1`. Complete expansion targets remain structured expansion nodes. A missing
or malformed duplication target has no accepting recovery production, so it
remains a genuine Tree-sitter `ERROR` or missing node.

In the CST, `_redirection` is a transparent supertype over `redirect_file` and
`redirect_dup`. A leading fd is a `file_descriptor` in the `source` field,
separate from the punctuation-only `operator` field. Ordinary commands, GOTO,
CALL, and SET preserve redirections before and within their argument tails. IF
and FOR still reject leading redirections, matching cmd. Redirections are
removed before SET interprets an unquoted assignment or `/P` name. Each
surviving, source-contiguous name segment is a separate
`name: (variable_name)` field, with positional `redirect` siblings between the
segments. This keeps every variable range contiguous while letting tools
reconstruct the logical name. A redirect inside other SET payload text is a
`redirect` field on the matching `set_*` node. Terminal SET redirects remain
fields on `set_statement`. Redirection ordering is preserved positionally;
last-wins and stream-merge semantics are runtime.

Because file targets stop at contextual standard separators, the adjacent
spelling `set x>out=value` keeps `out` as the redirect target and reconnects
`x` to the assignment delimiter. The assignment contains `name`, `redirect`,
then the following `value`, all with source-contiguous ranges.

### REM and ::

`REM` is a whole-word keyword followed by a delimiter and a free body to
end-of-line. The body is one opaque `comment_text` node, including text that
looks like `%VAR%` or `!VAR!`, and does not honor line continuation (cmd's
`ParseRem` does not splice). The scanner consumes the body so a one-character
body such as `&`, `|`, or `)` cannot be mistaken for an operator or block close.
Leading redirections belong to the comment, while a hyphen continues a command
name, so `>nul rem text` is a comment and `rem-tool` is a command.
`::` is a degenerate label used as a comment. A single colon at the start of a
physical line defines a label when a valid name follows it. Leading whitespace
after the colon is ignored, and spaces may occur inside the name. Parser-level
colon comments are recognized only at the lowest `&` precedence. They may begin
a physical line or follow `&`, which covers `dir &:note`, `dir &:: note`, and
`call :init &:# note`. ReactOS handles `@` by recursively calling
`ParseCommandOp(C_OP_LOWEST)`, so `@:: note` is a quiet statement whose body is
a colon comment. A direct colon cannot satisfy an IF or FOR body or the required
right operand of `||`, `&&`, or `|`; those spellings retain a syntax error.
Colon-comment lines inside blocks enter the same grammar slot, but remain unsafe
in real cmd. This avoids cascading CST damage without claiming that the spelling
is portable batch syntax; a linter layer could warn.

`<#` and `#>` are PowerShell comment delimiters, not CMD comments. Under CMD,
their `<` and `>` characters keep their redirection roles. The surrounding
bytes therefore parse as normal CMD tokens or as recovery errors. The grammar
does not assign a special comment node.

### SET /A

`SET /A`'s right-hand side is arithmetic, where `%` is the modulus operator
(written `%%` in a batch file) and bitwise `& | ^ < >` collide with cmd's outer
tokenizer (so source often caret-escapes or quotes them). The grammar captures
the expression as a generic argument tail rather than a full arithmetic
sub-grammar; see Limitations.

## 6. Node taxonomy

The named nodes group into these families (see `src/node-types.json` for the
full list):

- **Top level**: `program`, `quiet_statement`, `command`, `command_name`,
  `quiet`.
- **Operators**: `seq_list`, `or_list`, `and_list`, `pipeline`.
- **Redirection**: `_redirection`, `redirect_file`, `redirect_dup`,
  `redirect_operator`, `redirect_dup_operator`, and `file_descriptor`.
- **Blocks**: `block`.
- **Control flow**: `if_statement` (with `if_flag`, `not`, `comparison` /
  `comparison_operator`, and `unary_condition` / `condition_keyword`),
  `for_statement` (with `for_option` / `for_flag`,
  `loop_variable_declaration`, `for_set`, and the `backquote_string` /
  `single_quote_string` quoted items, whose interiors are neutral
  `backquote_content` / `single_quote_content` nodes),
  `goto_statement` (with `label_reference`, `label_name`, and `label_text`),
  `call_statement`, and `label` (with `label_name` and `label_text`).
- **SET**: `set_statement`, with the `set_assignment`, `set_prompt`, `set_arith`,
  `set_quoted`, and `set_display` branches, `variable_name`, and the opaque
  `set_ignored_suffix`.
- **Expansions** (the `_expansion` supertype): `variable`, `delayed_variable`,
  `parameter`, `all_arguments`, `parameter_tilde`, `loop_variable`, and
  `percent_literal`. The `:~off,len` substring and `:search=replace`
  substitution syntax stays inside the `variable` token, not separate nodes.
- **Words and literals**: `argument`, `text`, `string`, `escape_sequence`.
- **Comments**: `rem_comment`, `colon_comment`, `comment_text`; keywords
  surface as the aliased `keyword` node.

The unary `@` operator binds more loosely than the binary command operators.
Those operators use conventional left-associative tree-sitter precedence (cmd
binding, loosest to tightest: `&` < `||` < `&&` < `|`). This diverges from
ReactOS's right-leaning operator tree, which would add complexity with no
benefit to a static tool; observable left-to-right reading order is preserved
either way.

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
- **Unquoted parentheses in a FOR set**: the set runs from the opening `(` to the
  first `)`, the same block-paren rule used everywhere (and what cmd does), so an
  inner unquoted `(...)` closes the set early and the rest errors. A set item that
  contains parentheses must be quoted (`for %%a in ("file (1).txt")`), which parses
  cleanly. The error stays on that line; the block-depth counter does not desync.
- **Linefeed-named variables** (`%LF%` macros built by a caret/`%LF%` dance to
  fold multi-line code onto one logical line) are not supported. This is a
  torture-test trick rather than mainstream batch.
- **Caret-spelled grammar keywords** are not decoded. Cmd can recognize a
  keyword after removing carets, including a caret-newline within the word.
  The grammar may retain such a spelling as a generic command or an error.
- **FOR reference scope**: the grammar cannot resolve which one-character FOR
  variables are in scope. A lexical `%%x` form can therefore be a
  `loop_variable` reference outside a FOR body. It is never a
  `loop_variable_declaration` outside the declaration slot after `FOR`.
