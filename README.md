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
  `/F` with options or a `` `command` ``); `GOTO`, `CALL`, and labels.
- **The SET family**: `SET name=value`, `SET /A`, `SET /P`, quoted `SET "x=y"`,
  and display forms.
- **Expansions**: `%VAR%` and `%VAR:...%`, delayed `!VAR!`, positional `%0`-`%9`,
  `%*`, `%~` modifiers (`%~dp0`, `%~$PATH:1`), FOR variables `%%i`, and `%%`.
- **Comments**: `REM` and `::`.
- **Escaping**: the caret `^x` escape, caret line continuation, and
  double-quoted strings (quotes group; cmd does not strip them).

Parentheses are context-sensitive: `(` opens a block only where a command is
expected. In an argument it is a literal character that does not nest, so
`echo (text)` parses as one command. While a block is open, the first unescaped
`)` closes it, which is why cmd needs `^)` to echo a close-paren inside a block.
This is tracked by the external scanner as a block-depth counter.

Keyword extraction handles command disambiguation, so `set` is a keyword but
`setlocal` is a command, and `rem` is a comment but `remote` is a command.

## Usage

```sh
npm install
npx tree-sitter generate
npx tree-sitter parse path/to/script.bat
```

From Node:

```js
const Parser = require('tree-sitter');
const Cmd = require('tree-sitter-cmd');

const parser = new Parser();
parser.setLanguage(Cmd);

const tree = parser.parse('@echo off\r\nif exist x (echo y) else (echo z)\r\n');
console.log(tree.rootNode.toString());
```

Rust, Python, Go, and Swift bindings are generated under `bindings/`.

## Testing

```sh
npx tree-sitter test          # unit corpus (test/corpus/)
bash test/real-world/check.sh # real-world regression (test/real-world/)
```

The unit corpus holds focused cases with expected S-expressions, one file per
construct. The real-world corpus parses whole upstream scripts (gradlew.bat,
mvn.cmd, catalina.bat, Node's vcbuild.bat, and others) against per-file
ERROR-node budgets. Those fixtures are third-party test input under their own
licenses; see `test/real-world/README.md`.

## Known limitations

`cmd.exe` is phased and context-sensitive in ways a single context-free pass
cannot fully reproduce. None of these cascade on valid scripts. See
`GRAMMAR_DESIGN.md` for detail.

- **`!VAR!` is always parsed as a delayed reference**, even where
  `SETLOCAL ENABLEDELAYEDEXPANSION` is not active and it is literal at runtime.
- **`SET /A` expressions** are captured as a generic argument tail, not a full
  arithmetic sub-grammar.
- **String interiors are opaque**: `%VAR%` inside `"..."` is not sub-noded.
- **Line continuation** joins a mid-word caret before an indented next line into
  one word, where cmd would keep two arguments. The common `arg ^` form (space
  before the caret) is unaffected.
- **Variables whose name is a literal newline** (`%LF%` macros) are not
  supported.

## Layout

```
grammar.js          the grammar
src/scanner.c       external scanner (word-join, REM, block parens)
queries/            highlights.scm, injections.scm
test/corpus/        unit test corpus
test/real-world/    real-world regression harness
GRAMMAR_DESIGN.md   design document
bindings/           node, rust, python, go, swift bindings
```

## License

MIT.
