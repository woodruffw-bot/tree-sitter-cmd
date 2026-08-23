# tree-sitter-cmd

A [tree-sitter](https://tree-sitter.github.io/tree-sitter/) grammar for Windows
`cmd.exe` batch scripts (`.bat` and `.cmd`).

`cmd.exe` has no public grammar. This grammar is based on the
[ReactOS](https://github.com/reactos/reactos) parser
(`base/shell/cmd/parser.c`), the [ss64](https://ss64.com/nt/) and Microsoft
Learn references, and the dBenham/jeb batch-line-parser phase model. See
[`GRAMMAR_DESIGN.md`](GRAMMAR_DESIGN.md) for design decisions.

## What it parses

- Commands, argument tails, and the `@` echo-suppression prefix.
- The `&`, `&&`, `||`, and `|` operators. The grammar uses cmd precedence:
  `&` < `||` < `&&` < `|`.
- Input, output, and handle redirections, including leading redirections.
- Multi-line `( ... )` blocks with attached redirections.
- `IF` forms, including `/I`, `NOT`, comparisons, condition tests, `ELSE`,
  and nested statements.
- `FOR` forms, including `/D`, `/R`, `/L`, and `/F` sources and options.
- `GOTO`, `CALL`, labels, and the colon-glued `goto:eof` and `call:label`
  forms.
- `SET`, `SET /A`, `SET /P`, quoted assignments, and display forms.
- Percent, delayed, positional, modified, and `FOR` variable expansions.
- `REM` and `::` comments.
- Caret escapes, caret line continuations, and double-quoted strings. Expansions
  inside strings remain named nodes.

Parentheses depend on context. `(` starts a block only where a command is
expected. Inside an argument it is a literal character, so `echo (text)` is one
command. In a block, the first unescaped `)` closes the block. Use `^)` to
include a closing parenthesis in a command inside the block. The external
scanner tracks the block depth.

Keyword extraction distinguishes complete keywords from longer command names.
For example, `set` is a keyword, but `setlocal` is a command. Likewise, `rem`
is a comment, but `remote` is a command. Keywords appear as named `(keyword)`
nodes. Percent and delayed expansions share the `_expansion` supertype.

## Usage

```sh
cargo install --locked --version 0.26.11 tree-sitter-cli
tree-sitter generate --js-runtime native
tree-sitter parse path/to/script.bat
```

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

The crate exports `HIGHLIGHTS_QUERY` and `INJECTIONS_QUERY`. Only Rust bindings
are provided.

## Testing

```sh
tree-sitter test # unit corpus
tree-sitter fuzz # mutated inputs and incremental edits
cargo test       # Rust and real-world regression tests
```

The unit corpus contains focused inputs and expected syntax trees. Rust
integration tests parse upstream scripts from raw bytes and reject `ERROR` or
`MISSING` nodes. Each fixture retains its third-party license. See
[`test/real-world/README.md`](test/real-world/README.md).

## Known limitations

`cmd.exe` processes input in several context-dependent phases. A single
context-free parse cannot match every case. See `GRAMMAR_DESIGN.md` for details.

- `!VAR!` is always parsed as a delayed reference, even when delayed expansion
  is not active and the text is literal at runtime.
- `SET /A` expressions are an argument tail, not an arithmetic syntax tree.
- An unquoted `(` in a `FOR` set ends the set at the first `)`. Quote a set
  item that contains parentheses, such as
  `for %%a in ("file (1).txt")`.
- A caret that continues a word onto an indented line is joined into one word.
  `cmd.exe` treats it as two arguments. The common `arg ^` form is not
  affected. A final caret with no following line produces an error node.
- Variable names that contain a literal newline, as used by `%LF%` macros, are
  not supported.

## Layout

```
grammar.js          the grammar
src/scanner.c       external scanner (word-join, REM, block parens, caret escape, string end)
queries/            highlights.scm, injections.scm
test/corpus/        unit test corpus
test/real-world/    real-world regression harness
tests/              Rust integration tests
GRAMMAR_DESIGN.md   design document
bindings/           Rust crate
```

## License

MIT.
