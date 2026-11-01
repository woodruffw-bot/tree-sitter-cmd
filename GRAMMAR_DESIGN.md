# Syntax tree reference

This grammar parses Windows batch source for static analysis. It preserves
source spelling and byte ranges without executing commands or expanding
variables. [src/node-types.json](src/node-types.json) defines the available
nodes, fields, and their cardinality.

## Commands and control flow

`program` contains statements, labels, and comments. Ordinary commands have a
`command_name` and repeated `argument` and `redirect` fields. Adjacent text,
quotes, escapes, and expansions form one word. Whitespace separates words.

Keywords are case-insensitive and use named `keyword` nodes. Longer names such
as `setlocal` remain commands. Internal-command delimiters permit forms such
as `goto:eof` and `set/a`. Explicit paths and dotted names such as `if.exe`
remain command names.

The command operators bind in this order, from lowest to highest:

| Operator | Node | Fields |
| --- | --- | --- |
| `&` | `seq_list` | `left`, optional `right` |
| `\|\|` | `or_list` | `left`, `right` |
| `&&` | `and_list` | `left`, `right` |
| `\|` | `pipeline` | `left`, `right` |

Binary nodes are left-associative. `quiet_statement` wraps the complete
statement after `@`, including its operators. Repeated prefixes nest.

A `(` starts a `block` where a command is expected. In argument text it is
literal. Within a block, an unquoted, unescaped `)` closes the block, even if
an earlier argument contains a literal `(`. Outside a block, `)` can be text.
Blocks can have leading and trailing redirections.

### IF

`if_statement` has a `condition`, a `consequence`, and an optional `alternative`.
The condition is a `comparison` or `unary_condition`. `/I`, `NOT`, unary
condition keywords, and textual comparison operators require complete tokens.

Each branch includes the command-operator expression that follows it. Both
commands in `if 1==1 echo a & echo b` belong to the consequence. ELSE follows a
completed block on the same physical line, including a block at the end of an
operator expression. ELSE inside an ordinary command tail remains text.

An operand attached to `==` retains further equals signs: the right operand
of `if a===b=c echo yes` is `=b=c`. With a separator after `==`, standard token
separators are skipped instead.

### FOR

`for_statement` has a `variable` field containing a `loop_variable_declaration`,
an optional `option`, an optional `set`, and a `body`. The body includes its
following command-operator expression.

The grammar uses batch declarations such as `%%a`. It accepts the structural
switches `/D`, `/R`, `/L`, and `/F`. `/R` can take a root path. `/F` can take an
opaque option argument. Other option contents remain text.

The supported `/D` and `/R` combinations are `/D /R [path]`, `/R /D`, and
`/R path /D`. An explicit path cannot follow `/R /D`. Switches, the declaration,
IN, and DO require their actual token boundaries.

FOR sets allow newlines and standard separators. The first unquoted,
unescaped `)` closes the set. Apostrophes and backticks are ordinary set text.
They do not protect operators or imply a nested command language.

### Labels, GOTO, and CALL

`label` defines a jump target. `goto_statement.target` is a `label_reference`.
`call_statement.target` is an `argument`. Both commands require a target and
can contain redirections before or after it.

Label names retain literal spelling. A definition can have spaces in its name.
GOTO lookup delimiters can end the name before the command ends. The remaining
source is `label_text`. Quotes protect outer operators even when a GOTO lookup
delimiter occurs inside them.

An interleaved redirection splits a GOTO name into repeated `name` fields.
Each `label_name` covers contiguous source. Redirections remain siblings.
The tree does not resolve labels or account for CALL's runtime reparsing.

## SET

`set_statement` distinguishes assignment, prompt, arithmetic, quoted, and
display forms. Names retain expansion children. Values and prompts retain
source fragments in repeated fields when redirections interrupt them.

In `set "name=value"` and `set /p "name=prompt"`, the final surviving quote
closes the wrapper. Earlier quotes remain payload text and still affect outer
operator protection. Quotes in removed redirection targets do not close the
wrapper. Text after the final wrapper quote is `set_ignored_suffix`.

Parentheses in an unquoted name follow the surrounding block context. Balanced
quoted fragments inside an unquoted name protect operators and remain part of
the name, including their quotes. A quote that opens in an unquoted name and
closes in its value is not supported.

A caret-escaped opening quote does not protect later operators. Such SET
payloads use source fragments without inferred binding fields.

An unquoted name split by redirections has repeated `name: (variable_name)`
fields with `redirect` siblings. No name node spans removed redirection text.
SET `/A` requires a payload but does not parse the arithmetic language.

## Redirections

`redirect_file` and `redirect_dup` share the `_redirection` supertype. Both have
`operator` and `target` fields and an optional `source` file descriptor.

A descriptor is one digit immediately before `<` or `>` at a CMD token
boundary. For example, `echo a,2>out` redirects descriptor `2`, while
`echo a2>out` retains `a2` as text and redirects stdout. Closing quotes can also
establish a descriptor boundary. A spaced digit remains an argument.

File targets use standard separators: spaces, tabs, commas, semicolons, and
equals signs. Handle duplication skips these before its target. A duplication
target is a digit or a percent expansion evaluated before redirection parsing.
Delayed references, FOR references, and literal `%%` cannot supply this target.
Missing and invalid targets retain error state.

Redirections can occur before and within command tails. IF and FOR reject
leading redirections. The CST records source order without modeling stream
effects or the result of repeated redirects.

## Quotes, escapes, and expansions

Double quotes group source text and remain in its range. `string` nodes contain
expansion children. Open quotes run to the line ending or end of input.

Outside double quotes, a caret makes the following character literal. A
caret-newline discards the newline and makes the next character literal. An
`escape_sequence` retains the full source spelling, including the newline.
Inside double quotes, a caret is ordinary text and does not protect a closing
quote. LF and CRLF are supported. A standalone carriage return is not a newline.

Percent variables, positional parameters, `%*`, modified parameters, FOR
references, and literal `%%` have distinct nodes under `_expansion`. Modifier
payloads remain inside their expansion node. References can appear outside
the runtime scope where they would be meaningful.

Delayed references cannot hide active outer operators. A reference that would
change an ordinary word's quote state is split into literal and string
fragments. Its closing bang remains literal, so it cannot pair with a later
reference. Some SET and filename contexts conservatively retain the whole
quote-changing span as text.

## Comments

`rem_comment` has an opaque body through the end of the physical line.
Expansion-shaped text and operators in that body are not executable nodes.
REM requires an internal-command delimiter. `rem-tool` remains a command.

A colon at the start of a physical line defines a label when a valid name
follows. Double-colon comments and parser-level colon comments use
`colon_comment`. They can start a line or follow `&` or `@`. They cannot
directly satisfy an IF/FOR body or the right side of `&&`, `||`, or `|`.
Colon comments inside blocks are accepted, although CMD's runtime behavior
there is unreliable. PowerShell comment markers have no special meaning.

## Errors and analysis limits

Malformed input retains Tree-sitter `ERROR` or missing nodes. A missing IF,
ELSE, FOR, or quiet-statement body stays on its line. An empty command block
contains a missing command name.

Full child traversal also exposes anonymous, zero-width `_body_boundary`
terminals during missing-body and empty-block recovery. They are implementation
markers, not CMD syntax. Named traversal omits them. They accompany genuine
error state and should not be interpreted as executable content.

The tree represents unexpanded source. Expansions can change runtime syntax,
and a clean tree does not establish that a command will run successfully.
The grammar does not track environment values, delayed-expansion enablement,
SETLOCAL state, SHIFT numbering, FOR scope, command lookup, or filesystem state.

Other limits:

- Caret-spelled keywords are not decoded into control-flow nodes.
- FOR modifier spellings can depend on runtime variable scope.
- FOR `/F` source roles and option languages are not interpreted.
- Variable names containing literal newlines are unsupported.
- Caret handling does not model the conditional delayed-expansion pass inside
  quotes.

Callers must select or decode the input encoding. Byte ranges refer to the
buffer given to Tree-sitter. Transcoding requires an offset map when original
file offsets matter. The grammar does not detect OEM code pages or mixed
encodings. Tree-sitter handles a leading byte-order mark.
