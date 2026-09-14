# tree-sitter-cmd grammar design

This document describes the grammar's concrete syntax tree (CST), its parsing
rules, and its limitations. The grammar targets Windows `cmd.exe` batch scripts
for static analysis. It parses source text without executing commands or
expanding variables.

References include the ReactOS cmd implementation (`parser.c`, `cmd.c`,
`batch.c`, `for.c`, and `if.c`), the dBenham/jeb model of batch parsing phases
(DosTips t=3587), ss64, Microsoft Learn, and the Tree-sitter grammars listed under
[Prior art](#7-prior-art). Development instructions are in [AGENTS.md](AGENTS.md).

## 1. Scope

The grammar covers commands and arguments, `@` echo suppression, command
operators, redirections, blocks, IF/ELSE, FOR, GOTO, CALL, labels, SET, comments,
expansions, caret escapes, and double-quoted strings. Its scope name is
`source.dosbatch`, with file types `.bat` and `.cmd`.

The CST does not resolve variable values, filesystem globs, jump targets,
command success, SHIFT parameter numbering, or SETLOCAL state. Argument parsing
by an invoked program is also outside its scope.

Program flags and option payloads remain opaque unless they change cmd's outer
syntax. FOR `/R` can consume a path before the loop variable. FOR `/F` options,
including `usebackq`, `tokens=`, `delims=`, `skip=`, and `eol=`, remain text in one
argument node.

The grammar uses batch spellings such as `%%x` for FOR variables. It does not
target interactive single-percent FOR syntax. Delayed references such as
`!VAR!` are recognized regardless of whether delayed expansion is enabled at
runtime.

Labels and colon comments have distinct nodes. `:name` defines a jump target,
while `::text` is a comment label.

### Input encoding

The caller selects an encoding or decodes the file before parsing. The grammar
does not detect encodings or choose an OEM code page. Tree-sitter's default API
accepts UTF-8. Bindings may also provide UTF-16 and custom-decoder APIs.

CST byte ranges refer to the input buffer. Transcoding changes those offsets,
so callers that need original file offsets must retain a mapping or use an input
API for the original encoding. Tree-sitter handles a leading byte-order mark.
Mixed encodings require rejection or normalization before parsing. The script
fixture harness accepts UTF-8 only.

## 2. Why cmd is hard to parse

Cmd expands, parses, and executes logical lines in stages. The grammar represents
the whole source file as one tree, before expansion. Relevant stages in the
batch parsing phase model include:

| Phase | Action |
| --- | --- |
| 1 | Percent expansion, including `%%`, `%N`, `%*`, `%~`, and `%VAR%` |
| 1.5 | Carriage-return removal |
| 2 | Caret and quote handling, operator recognition, and REM/IF/FOR parsing |
| 4 | FOR variable substitution |
| 5 | Delayed expansion and a further caret-removal pass |

Percent expansion happens before caret handling. Delayed expansion happens
after it. Expansion can introduce or remove syntax that is absent from the
source tree.

Token boundaries also depend on context. Quotes protect some characters,
parentheses depend on block depth, and a digit can be text or a redirection
source. Commas, semicolons, and equals signs act as separators in some parser
positions but remain text in ordinary argument tails. Caret continuation makes
the first character of the next physical line literal.

These rules cannot all be reproduced by one context-free parse of unexpanded
source. The grammar's approximations are described under
[Limitations](#8-limitations).

## 3. Lexing: grammar.js vs the external scanner

Most rules are in [grammar.js](grammar.js). Only spaces and tabs are `extras`.
Newlines terminate statements, and immediate tokens require source adjacency.
Keywords are case-insensitive and appear as named `keyword` nodes.

Keyword extraction uses `word: $._cmd_text`. Internal-command delimiters
(`:.\,/;=[]`) allow spellings such as `goto:eof`, `call:label`, and `set/a`.
The `_cmd_path` rule preserves dotted executable names and explicit paths, so
`if.exe` remains a command name. Drive-letter paths also stay intact. Longer
names such as `setlocal` do not become keywords.

Hidden separator rules consume punctuation only where cmd treats it as a
separator. IF modifiers, unary condition keywords, and FOR switches require a
source delimiter. Textual IF comparison operators must end at whitespace, a
comma, or a semicolon. Subsequent separators may include equals signs. Ordinary
command arguments and SET value tails retain this punctuation as text.

The [external scanner](src/scanner.c) handles tokens that depend on parsing
context. Its serialized state contains block depth, a missing-body boundary
counter, and quote state for SET bindings.

| Token | Purpose |
| --- | --- |
| `CONCAT` | Joins adjacent word fragments without consuming bytes |
| `STANDARD_CONCAT` | Joins fragments until a standard separator |
| `REDIRECT_CONCAT` | Joins filename fragments, preserving attached literal parentheses |
| `IF_ATTACHED_OPERAND` | Selects an operand attached directly to `==` |
| `IF_FLAG` / `IF_NOT` / `IF_CONDITION_KEYWORD` | Matches IF control words before a standard separator |
| `KEYWORD_BOUNDARY` | Checks the token boundary after `ELSE`, `IN`, or `DO` |
| `REM` / `REM_TEXT` | Recognizes the REM keyword and its opaque body |
| `REDIRECT_SOURCE` | Recognizes a file descriptor digit immediately before `<` or `>` |
| `ESCAPED_REDIRECT_DIGIT` | Keeps an escaped word's final digit from becoming a redirection source |
| `EMPTY_BLOCK_OPEN` | Opens a command block whose next non-whitespace character is `)` |
| `BLOCK_OPEN` / `BLOCK_CLOSE` | Opens or closes a structural block |
| `LPAREN` / `RPAREN` | Recognizes literal parentheses |
| `CARET_ESCAPE` | Preserves a caret or caret continuation before an expansion |
| `DELAYED_VARIABLE` | Recognizes a delayed reference without consuming active outer syntax |
| `DELAYED_QUOTE_TEXT` | Keeps a delayed reference and its quoted suffix in one text node |
| `STRING_END` | Closes a string at a quote, line ending, or end of input |
| `SET_STRING_START` / `SET_INNER_QUOTE` / `SET_STRING_END` | Tracks wrapper and internal quotes in SET bindings |
| `SET_NAME_TEXT` / `SET_NAME_TAIL_TEXT` | Scans SET name text under the current quote state |
| `SET_VALUE_TEXT` | Scans SET value text up to an active outer operator |
| `SET_IGNORED_SUFFIX` | Preserves text after the final SET wrapper quote |
| `LABEL_LEADING_SPACE` | Consumes space after a definition colon only when a valid name follows |
| `SET_BINDING_END` | Confirms an assignment delimiter after a redirected SET name |
| `BODY_BOUNDARY` / `BODY_BOUNDARY_AGAIN` | Marks a line boundary before recovery of a missing IF/ELSE/FOR body |
| `BLOCK_BODY_BOUNDARY` | Keeps empty-block recovery inside the block |
| `COMMAND_START` | Remains unavailable so a missing body records a parser error |
| `ERROR_SENTINEL` | Detects Tree-sitter's state in which all external tokens are valid during recovery |

The scanner treats `=` as a word boundary so concatenation cannot consume the
delimiters in `==` or `name=value`. An immediate token in ordinary arguments
allows an attached `=` to continue the word, as in `%VAR%=suffix`.
`STANDARD_CONCAT` also stops at commas and semicolons.

During recovery, the scanner declines zero-width tokens but still emits
`BLOCK_CLOSE`. This prevents an error inside a block from consuming later
commands.

A missing controller body uses two anonymous `_body_boundary` terminals followed
by `COMMAND_START`, which the scanner never emits. This records MISSING
`"command"` on the same line and leaves the next line separate. Full traversal
shows the boundary terminals as ordinary zero-width nodes. Named traversal omits
them. They guide recovery and do not represent cmd syntax. Removing them can
make a missing body consume the next line or detach an operator from a nested
empty block. Tests cover full and incremental trees, including anonymous nodes.

## 4. The parenthesis model

The scanner tracks the depth of structural blocks. It does not balance every
literal parenthesis in the source.

- `(` starts a block where a command or FOR set is expected.
- In an argument, `(` is literal and does not increase block depth. When
  attached to an existing fragment, it stays in that argument.
- Inside a block, an unquoted, unescaped `)` closes the block. A literal `(`
  earlier in an argument does not protect it.

This matches the distinction in ReactOS `parser.c` between block starts and
parentheses inside arguments. It accounts for `echo (text)` outside a block,
`^)` inside a block, and the `(echo()` blank-line ECHO idiom.

A whitespace-only command block is invalid. `EMPTY_BLOCK_OPEN` selects this
case, and `BLOCK_BODY_BOUNDARY` keeps recovery inside the block if an outer
operator follows. The CST contains a direct MISSING `command_name`. It does not
contain a normal `command` with no source text.

## 5. Tricky areas

### Quiet statements

`@` applies to the complete statement that follows. In
`@echo one & echo two`, it suppresses echo for the full sequence. The CST uses a
`quiet_statement` with `quiet` and `body` fields. Repeated prefixes nest, so
`@@echo off` retains both operators.

A missing body records an error without consuming the next physical line.
Labels have their own `quiet` field, so `@:label` remains a `label` node.

### Caret escaping and line continuation

A mid-line `^x` escape makes the next character literal. A continuation `^\nX`
discards the newline and makes `X` literal. The grammar preserves the full
spelling in an `escape_sequence`. It accepts LF and CRLF line endings.

In `^\n&&`, the first `&` is literal and the second is a command separator.
In `^\n|`, the pipe is literal. A continuation with no following character
remains an error. The forced character can itself be a newline, as in
`SET LF=^\n\n`.

Adjacency still determines argument boundaries. `echo a^\nb` has one argument,
while the space in `echo a ^\nb` separates two arguments. Ordinary newlines
remain statement terminators. A carriage return is recognized as part of CRLF,
not as a separate newline.

A caret before a percent or delayed expansion uses `CARET_ESCAPE`. The token
covers the caret and any continuation newline, leaving the opening `%` or `!`
in the expansion node. This preserves both parts despite their different
runtime parsing phases.

Caret behavior inside quotes is approximate. In phase 2, a caret inside double
quotes is literal. The later delayed-expansion pass can remove it even inside
quotes, as in the `^^!` idiom. The grammar does not model that conditional pass.

### Expansions

Expansions are single tokens. `%VAR%` becomes a `variable`, while positional
parameters, `%*`, modified parameters, FOR references, and literal `%%` have
separate node types. The variable rule excludes leading digits, `~`, and `*` so
it does not consume those parameter forms.

Substring and substitution text, such as `:~start,len` and `:search=replace`,
stays inside the `variable` or `delayed_variable` node. The CST does not split
out the name or modifier payload.

Modified parameters and FOR references recognize the letters `dpnxfsatz` and an
optional `$ENV:` search clause. The split between modifiers and a FOR variable
can depend on runtime scope. The grammar chooses one lexical interpretation.

A delayed reference cannot hide an active outer operator or block close.
The scanner tracks quote protection while recognizing it. Quotes and protected
metacharacters in a substitution payload stay inside the reference.

### Strings

Cmd uses double quotes for grouping and retains them in the command text.
Expansions still occur inside quotes. A `string` covers its literal text and
contains named expansion children. For example, `"%PATH%"` contains a
`variable`.

The external `_string_end` token consumes a closing quote or matches without
consuming bytes at a line ending or end of input. This represents cmd's handling
of open quotes. It also avoids the ambiguity of an optional closing quote,
which could split `"%PATH%"` into two empty strings and an expansion. A quote
inside a percent substitution such as `%VAR:"=%` stays in the variable token.

Quoted SET forms use separate scanner state. In `SET "name=value"` and
`SET /P "name=prompt"`, the last quote closes the wrapper. Earlier quotes remain
in the value or prompt. A quoted redirect target does not close the wrapper.
Active redirections split a value or prompt into fields in source order. Names
retain expansion children, including `%~1r` and `_nt!nt!`.

The same wrapper accepts a display prefix such as `SET "PATH"`. Text after the
last quote becomes `set_ignored_suffix`, outside the name, value, or prompt.
Redirections in that suffix keep their source positions. A terminal redirection
remains on `set_statement`.

A caret-escaped opening quote, as in `set ^"macro=call helper^"`, starts a
`set_quoted` payload of text, escapes, strings, and expansions. The escaped quote
is literal, so it does not group operators or redirections. This form has no
inferred binding fields or required closing quote.

If a delayed reference opens a quote, as in `!a"b!`, one `text` node extends
through the next quote or line end. This prevents later bangs from becoming
invented expansions. Redirection filenames use the same representation.

### IF / ELSE

ELSE starts a branch only after a completed block, including a block at the end
of an operator expression. Within a simple command tail it remains text, so
`IF 1==1 ECHO someone else here` has no alternative branch.

ELSE must begin on the same physical line as the end of the consequence and end
at a token boundary. Right precedence attaches it to the nearest IF. Each branch
accepts a full command-operator expression. Thus both commands in
`IF 1==1 ECHO a & ECHO b` belong to the consequence. IF rejects leading
redirections.

`/I` must also be a complete token. In `IF /ileft==right ...`, `/ileft` remains
the left operand. A missing consequence or alternative records MISSING
`"command"` at the controller's line boundary without adopting the next line.

Immediately before a binary comparison operator, commas and semicolons can be
separators. Equals signs remain available for `==`. A textual comparison
operator must end at whitespace, a comma, or a semicolon. Thus
`IF left equright ...` and `IF left equ=right ...` do not become EQU comparisons.
After a delimiter ends the operator, standard separators can include `=`.

An operand attached to `==` preserves subsequent equals signs. In
`IF b===b ...`, the right argument is `=b`. In `IF a===b=c ...`, it is `=b=c`.
A spaced form such as `IF b== =b ...` skips the third equals sign as a standard
separator.

### FOR

FOR variants share `FOR [options] %%v IN (set) DO body`. The body consumes a full
command-operator expression, so both commands in `DO ECHO %%v & ECHO done`
belong to it. A missing body uses the same local MISSING `"command"` recovery
as IF. The set uses block delimiters and permits newlines between items.

A `loop_variable_declaration` is `%%` followed by one permitted character.
Examples include `%%#`, `%%0`, and `%%@`. Modifiers such as `~f` occur only in
references. The plain `%%x` token is shared by references and declarations, so
an outer-loop reference can begin a `/R` path such as `%%a\sub` without
consuming the following declaration. A delimiter must follow the declaration.

FOR `/L` accepts commas, semicolons, equals signs, or spaces between its set
items. For example, `(1;1=5)` produces three arguments.

The accepted `/D` and `/R` combinations are `/D /R [path]`, pathless `/R /D`, and
`/R path /D`. An explicit root cannot follow `/R /D`. Other mixed switches are
syntax errors. Switches must be complete tokens, so `/ffoo`, `/rC:\src`, and
`/d/r` do not split into recognized switches and attached arguments.

`/R` and `/F` each accept one optional argument separated from the switch.
An unquoted slash-leading word is not an ordinary argument in that position,
which prevents an invalid second switch from becoming an option payload or path.

FOR `/F` command quoting depends on the full option payload and the delimiters
around the reconstructed set. The grammar does not infer that runtime role.
Apostrophes and backticks remain ordinary `for_set` argument text, whether
matched or unmatched. The injection query assigns them no nested language.
These characters do not protect raw outer operators or parentheses. A caret is
needed where those characters would otherwise be structural.

### GOTO and CALL

GOTO requires a `label_reference` target. CALL requires an `argument` target.
Redirections can precede the target but cannot replace it. Bare and
redirection-only forms retain `ERROR` or missing state. Recovery stops at the
physical line boundary without consuming the next command as a target.

A redirection can split a GOTO target. The `label_reference` retains the text in
source order as contiguous `label_name` segments in `name` fields, with
`redirect` siblings between them. Text after a lookup delimiter remains
`label_text` across redirections. Terminal redirections stay on `goto_statement`.

### Redirection

A source file descriptor is a digit at a token boundary immediately followed by
`<` or `>`. In `echo 2>file`, the `2` is the redirection source. In `abc2>file`,
it remains part of the command name. A spaced digit is an ordinary argument, as
in `echo 2 >file`. The scanner checks descriptors before joining fragments, so
`echo "text"2>file` also uses `2` as the source.

File targets use standard separators. Punctuation before a target is skipped,
and punctuation after it ends the target. Handle duplication skips spaces,
tabs, commas, semicolons, and equals signs before its target. Thus `2>& ,;=1`
and `2>&1` have the same source, operator, and target. Expansion targets remain
expansion nodes. A missing or malformed duplication target records `ERROR` or
missing state.

The `_redirection` supertype groups `redirect_file` and `redirect_dup`. A
`file_descriptor` appears in the `source` field, separate from the punctuation
in `operator`. Commands, GOTO, CALL, and SET preserve redirections before and
within their argument tails. IF and FOR reject leading redirections.

Cmd removes redirections before interpreting an unquoted SET assignment or
prompt name. The CST retains each contiguous part of the name as a
`name: (variable_name)` field, with `redirect` siblings in source order. Tools
can reconstruct the logical name without treating separated source ranges as
one variable node. Redirections inside other SET payloads belong to the matching
`set_*` node. Terminal redirections belong to `set_statement`. Stream effects
and the result of repeated redirections remain runtime behavior.

In `set x>out=value`, `out` is the redirect target and `=` is the assignment
delimiter. The assignment contains the name, redirection, and value in source
order.

### REM and ::

REM is a complete keyword followed by a delimiter and an opaque body through the
end of the line. The `comment_text` includes text resembling expansions and
does not splice caret continuations. The scanner consumes the body so a lone
`&`, `|`, or `)` in it cannot become an operator or block close. Leading
redirections belong to the comment. A hyphen continues a command name, so
`>nul rem text` is a comment and `rem-tool` is a command.

A single colon at the start of a physical line defines a label when a valid name
follows. Leading whitespace after the colon is ignored, and names can contain
spaces. A double colon is a comment label.

Parser-level colon comments are accepted at the lowest `&` precedence. They can
start a physical line or follow `&`, as in `dir &:note`, `dir &:: note`, and
`call :init &:# note`. `@:: note` is a quiet statement containing a colon
comment.

A direct colon cannot satisfy an IF or FOR body or the right operand of `||`,
`&&`, or `|`. Those forms retain a syntax error. Colon comments inside blocks
are accepted to limit cascading parse errors, although their runtime behavior
in cmd is unreliable.

PowerShell's `<#` and `#>` delimiters have no special comment role in this
grammar. The `<` and `>` retain their cmd redirection roles, so the surrounding
text produces ordinary cmd nodes or parse errors.

### SET /A

SET `/A` accepts an arithmetic expression. Its `%` modulus operator is written
`%%` in batch source. Bitwise operators can conflict with cmd tokenization and
are often quoted or caret-escaped. The grammar retains the expression as an
argument tail without parsing the arithmetic.

## 6. Node taxonomy

[src/node-types.json](src/node-types.json) lists the node types and fields.
The main groups are:

- Top level: `program`, `quiet_statement`, `command`, `command_name`, `quiet`.
- Operators: `seq_list`, `or_list`, `and_list`, `pipeline`.
- Redirections: `_redirection`, `redirect_file`, `redirect_dup`,
  `redirect_operator`, `redirect_dup_operator`, `file_descriptor`.
- Blocks: `block`.
- IF: `if_statement`, `if_flag`, `not`, `comparison`, `comparison_operator`,
  `unary_condition`, `condition_keyword`.
- FOR: `for_statement`, `for_option`, `for_flag`, `loop_variable_declaration`,
  `for_set`.
- GOTO, CALL, and labels: `goto_statement`, `call_statement`, `label`,
  `label_reference`, `label_name`, `label_text`.
- SET: `set_statement`, `set_assignment`, `set_prompt`, `set_arith`, `set_quoted`,
  `set_display`, `variable_name`, `set_ignored_suffix`.
- Expansions, grouped by `_expansion`: `variable`, `delayed_variable`, `parameter`,
  `all_arguments`, `parameter_tilde`, `loop_variable`, `percent_literal`.
- Words and literals: `argument`, `text`, `string`, `escape_sequence`.
- Comments: `rem_comment`, `colon_comment`, `comment_text`.
- Keywords: `keyword`.

The unary `@` operator binds below the binary command operators. Binary
precedence, from lowest to highest, is `&`, `||`, `&&`, `|`. The grammar uses
left-associative binary nodes. ReactOS uses a right-leaning operator tree.
Both preserve source order.

## 7. Prior art

- `wharflab/tree-sitter-batch` (MIT) informed the configuration and grammar helper
  patterns. It implements cmd parsing in `grammar.js` without an external scanner.
- `tree-sitter/tree-sitter-bash` (MIT) informed the scanner's concatenation,
  token gating, and state serialization. Cmd quoting and expansion rules are
  handled separately.

## 8. Limitations

- Expansions can introduce or remove operators and quotes. The grammar parses
  unexpanded source and cannot represent the resulting runtime syntax.
- Delayed references are recognized even when delayed expansion is disabled.
- The split between FOR modifiers and variable names can depend on runtime
  scope. The grammar chooses one interpretation.
- SET `/A` arithmetic remains an argument tail.
- FOR `/F` command-source roles are not inferred from apostrophes, backticks, or
  the option payload. Those delimiters remain argument text.
- An unquoted, unescaped `)` ends a FOR set. A filename containing parentheses
  needs quotes, as in `for %%a in ("file (1).txt") do echo %%a`.
- Variable names containing literal newlines, including names used in newline
  macros, are not supported.
- Caret-spelled keywords are not decoded. Cmd can recognize some keywords after
  removing carets or continuations, while this grammar may produce a generic
  command or an error.
- FOR reference scope is not tracked. A `%%x` reference can appear outside a
  FOR body, but `loop_variable_declaration` appears only in the declaration slot.
