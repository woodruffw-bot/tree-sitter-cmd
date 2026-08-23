# tree-sitter-cmd

A [tree-sitter](https://tree-sitter.github.io/tree-sitter/) grammar for the
Windows `cmd.exe` command interpreter, i.e. batch scripts (`.bat`, `.cmd`).

`cmd.exe` has no formal or public grammar, so this one is grounded in the
[ReactOS](https://github.com/reactos/reactos) reimplementation
(`base/shell/cmd/parser.c`), the [ss64](https://ss64.com/nt/) and Microsoft
Learn references, and the dBenham/jeb batch-line-parser phase model. See
[`GRAMMAR_DESIGN.md`](GRAMMAR_DESIGN.md) for the design rationale.

## What it parses

- **Commands**: command name plus argument tail, and the `@` echo-suppress
  prefix (`@echo off`, `@rem`, `@if`).
- **Operators**: sequencing `&`, and `&&`, or `||`, and pipes `|`, with cmd
  precedence (`&` < `||` < `&&` < `|`).
- **Redirections**: `>`, `>>`, `<`, fd-prefixed (`2>`), and handle duplication
  (`2>&1`, `>&2`), including leading redirections.
- **Blocks**: multi-line `( ... )` compounds with attached redirections.
- **Control flow**: `IF` (`/I`, `NOT`, the comparison and `EXIST`/`DEFINED`/
  `ERRORLEVEL`/`CMDEXTVERSION` tests, `ELSE`, nesting); `FOR` (`/D`, `/R`, `/L`,
  `/F` with options and a `` `command` ``, `'command'`, or file source); `GOTO`,
  `CALL` (including the colon-glued `goto:eof` / `call:label` forms), and labels.
- **The SET family**: `SET name=value`, `SET /A`, `SET /P`, quoted `SET "x=y"`,
  and display forms.
- **Expansions**: `%VAR%` and `%VAR:...%`, delayed `!VAR!`, positional `%0`-`%9`,
  `%*`, `%~` modifiers (`%~dp0`, `%~$PATH:1`), FOR variables `%%i`, and `%%`.
- **Comments**: `REM` and `::`.
- **Escaping**: the caret `^x` escape, caret line continuation, and
  double-quoted strings (quotes group; cmd does not strip them). Expansions
  inside a string are sub-noded, so a quoted `%PATH%` is a real `variable` node.

Parentheses are context-sensitive: `(` opens a block only where a command is
expected. In an argument it is a literal character that does not nest, so
`echo (text)` parses as one command. While a block is open, the first unescaped
`)` closes it, which is why cmd needs `^)` to echo a close-paren inside a block.
This is tracked by the external scanner as a block-depth counter.

Keyword extraction handles command disambiguation, so `set` is a keyword but
`setlocal` is a command, and `rem` is a comment but `remote` is a command.
Keywords surface as named `(keyword)` nodes, and the `%…%`/`!…!` expansion forms
share a `_expansion` supertype so queries can target them as a group.

## Usage

```sh
cargo install --locked --version 0.26.11 tree-sitter-cli
tree-sitter generate --js-runtime native
tree-sitter parse path/to/script.bat
```

The CLI comes from the official Rust crate. Its bundled native runtime evaluates
`grammar.js`, so Node and npm are not required.

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

Rust is the only binding; the generated parser sources live in `src/`.

## Testing

```sh
tree-sitter test # unit corpus (test/corpus/)
tree-sitter fuzz # mutated corpus inputs and incremental edits
cargo test       # Rust, incremental, and real-world regression tests
```

The unit corpus holds focused cases with expected S-expressions, one file per
construct. The real-world corpus parses whole upstream scripts (gradlew.bat,
mvn.cmd, catalina.bat, Node's vcbuild.bat, and others) from raw bytes. Every
fixture must parse without `ERROR` or `MISSING` nodes. Those fixtures are
third-party test input under their own licenses; see `test/real-world/README.md`.

CI runs the CLI fuzzer on each change. A separate libFuzzer job runs briefly on
parser pull requests and for a longer period each week. Both build the committed
C parser and scanner directly. They do not require Node or npm.

## Known limitations

`cmd.exe` is phased and context-sensitive in ways a single context-free pass
cannot fully reproduce. None of these cascade on valid scripts. See
`GRAMMAR_DESIGN.md` for detail.

- **`!VAR!` is always parsed as a delayed reference**, even where
  `SETLOCAL ENABLEDELAYEDEXPANSION` is not active and it is literal at runtime.
- **`SET /A` expressions** are captured as a generic argument tail, not a full
  arithmetic sub-grammar.
- **An unquoted `(` in a FOR set** ends the set at the first `)`, like any block
  paren (and like cmd). A set item that contains parentheses must be quoted, e.g.
  `for %%a in ("file (1).txt")`.
- **Line continuation** joins a mid-word caret before an indented next line into
  one word, where cmd would keep two arguments. The common `arg ^` form (space
  before the caret) is unaffected. A *dangling* caret continuation at the end of
  the file, with no following line to splice onto, produces an error node.
  Continuation onto a following line, blank or not, is fine.
- **Variables whose name is a literal newline** (`%LF%` macros) are not
  supported.

## Layout

```
grammar.js          the grammar
src/scanner.c       external scanner (word-join, REM, block parens, caret escape, string end)
queries/            highlights.scm, injections.scm
test/corpus/        unit test corpus
test/real-world/    real-world regression harness
tests/              Rust integration tests
GRAMMAR_DESIGN.md   design document
bindings/           rust crate
```

## License

MIT.
