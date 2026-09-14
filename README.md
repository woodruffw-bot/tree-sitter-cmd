# tree-sitter-cmd

A [Tree-sitter](https://tree-sitter.github.io/tree-sitter/) grammar for Windows
`cmd.exe` batch scripts (`.bat` and `.cmd`), intended for static analysis.

`cmd.exe` has no public formal grammar. This grammar draws on the
[ReactOS parser](https://github.com/reactos/reactos/tree/master/base/shell/cmd),
[ss64](https://ss64.com/nt/), Microsoft Learn, and the dBenham/jeb model of batch
parsing phases. [GRAMMAR_DESIGN.md](GRAMMAR_DESIGN.md) describes the behavior and
limitations in detail.

## Supported syntax

- Commands and arguments, with `@` echo suppression over full statements.
- Command operators, from lowest to highest precedence: `&`, `||`, `&&`, `|`.
- File and handle redirections before and after commands or within their arguments.
- Parenthesized blocks with attached redirections.
- `IF` conditions, comparisons, `/I`, `NOT`, `ELSE`, and nested statements.
- `FOR` loops, including `/D`, `/R`, `/L`, and `/F`.
- `GOTO`, `CALL`, labels, `goto:eof`, and `call:label`.
- `SET`, `SET /A`, `SET /P`, quoted assignments, and display forms.
- Percent, delayed, positional, modified, and `FOR` variable expansions.
- `REM` and `::` comments.
- Caret escapes, line continuations, and double-quoted strings.

A `(` starts a block where a command is expected. In an argument it is literal,
so `echo (text)` is one command. Inside a block, an unquoted, unescaped `)` closes
the block. A literal closing parenthesis in that context needs a caret escape,
`^)`.

Keywords appear as named `keyword` nodes. A longer command name such as
`setlocal` or `remote` does not become a `set` or `rem` keyword. Expansions remain
named nodes inside strings and share the `_expansion` supertype.

## Usage

From the repository root:

```sh
cargo install --locked --version 0.26.11 tree-sitter-cli
tree-sitter parse path/to/script.bat
```

The repository includes the generated parser. Only Rust bindings are provided.

From Rust:

```rust
use tree_sitter::Parser;

let mut parser = Parser::new();
parser
    .set_language(&tree_sitter_cmd::LANGUAGE.into())
    .expect("loading Cmd grammar");

let source = "@echo off\r\nif exist x (echo y) else (echo z)\r\n";
let tree = parser.parse(source, None).unwrap();
println!("{}", tree.root_node().to_sexp());
```

The crate also exports `NODE_TYPES`, `HIGHLIGHTS_QUERY`, and `INJECTIONS_QUERY`.

### Input encoding

The caller selects or decodes the script's encoding before parsing:

- The default Tree-sitter parse API accepts UTF-8. Tree-sitter handles a leading
  byte-order mark.
- Bindings with UTF-16LE, UTF-16BE, or custom-decoder APIs can preserve offsets in
  the original encoded input.
- OEM code pages need external context. The grammar cannot infer a code page from
  the script.
- Mixed encodings need to be rejected or normalized before parsing. One
  Tree-sitter input has one encoding.

Node byte ranges refer to the buffer passed to Tree-sitter. After transcoding to
UTF-8, they refer to the UTF-8 bytes. Callers that need the original file offsets
must retain an offset map or use a suitable Tree-sitter input API.

## Known limitations

`cmd.exe` expands and parses input in several phases. This grammar parses the
unexpanded source, so it cannot reproduce every runtime interpretation.

- Delayed references such as `!VAR!` are recognized even when delayed expansion
  is disabled at runtime.
- `SET /A` expressions remain argument text without an arithmetic syntax tree.
- A `FOR` set ends at the first unquoted, unescaped `)`. A filename containing
  parentheses needs quotes, as in `for %%a in ("file (1).txt") do echo %%a`.
- `FOR /F` apostrophes and backticks remain ordinary argument text. The grammar
  does not infer command sources or inject another language. These delimiters do
  not protect outer operators or parentheses from cmd parsing.
- Caret-spelled control-flow keywords are not decoded into keyword nodes. They
  may remain command text or produce an error.
- Variable names containing a literal newline are not supported.
- `FOR` reference scope and ambiguous modifier spellings cannot be resolved from
  syntax alone.

The [script fixtures](test/real-world/README.md) and
[Windows tests](test/windows/README.md) describe how behavior is checked.
Development instructions are in [AGENTS.md](AGENTS.md).

## License

[MIT](LICENSE).
