# tree-sitter-cmd

A [tree-sitter](https://tree-sitter.github.io/tree-sitter/) grammar for the
**Windows `cmd.exe` command interpreter** — i.e. batch scripts (`.bat`, `.cmd`).

The grammar aims to be as conformant as practical with real `cmd.exe` behaviour.
Since cmd has no formal or public grammar, it is grounded in the
[ReactOS](https://github.com/reactos/reactos) reimplementation of `cmd.exe`
(`base/shell/cmd/parser.c`), the [ss64](https://ss64.com/nt/) and Microsoft
Learn references, and the dBenham/jeb batch-line-parser phase model. See
[`GRAMMAR_DESIGN.md`](GRAMMAR_DESIGN.md) for the full design rationale.

## Status

Validated against a suite of well-known real-world batch scripts. **11 of 12
parse with zero error nodes**, including the notoriously tricky `gradlew.bat`,
the 333-line `mvn.cmd`, and a 579-line LLVM release script:

| Script | Lines | Errors |
|--------|------:|-------:|
| `gradlew.bat` (Gradle) | 82 | 0 |
| `mvn.cmd` (Maven) | 333 | 0 |
| `build_llvm_release.bat` (LLVM) | 579 | 0 |
| `build.bat` (CPython) | 234 | 6¹ |
| `razzle.cmd` (Windows Terminal) | 126 | 0 |
| `conda.bat`, `activate.bat` (conda / virtualenv) | — | 0 |
| `npm.cmd`, `build.cmd` (.NET), `bootstrap-vcpkg.bat`, … | — | 0 |

¹ CPython's `build.bat` uses the `echo.message (parenthesised)` idiom — a
documented limitation (literal unquoted parentheses in arguments).

## Features

The grammar models:

- **Commands** — name + argument tail, the `@` echo-suppress prefix (before any
  command form, e.g. `@rem`, `@if`, `@echo off`).
- **Operators** — sequencing `&`, and-`&&`, or-`||`, and pipes `|`, with the
  cmd precedence `&` < `||` < `&&` < `|`.
- **Redirections** — `>`, `>>`, `<`, fd-prefixed (`2>`), and handle duplication
  (`2>&1`, `>&2`), including leading redirections.
- **Parenthesised blocks** — multi-line `( … )` compounds with attached
  redirections; `::`/labels inside blocks.
- **Control flow** — `IF` (`/I`, `NOT`, `==`/`EQU`/`NEQ`/`LSS`/`LEQ`/`GTR`/`GEQ`,
  `EXIST`/`DEFINED`/`ERRORLEVEL`/`CMDEXTVERSION`, single-line & block, `ELSE`,
  nesting, the `if (%1)==()` idiom); `FOR` (`/D`, `/R [path]`, `/L`, `/F` with
  options or a `` `command` ``); `GOTO`/`CALL` and labels.
- **The SET family** — `SET name=value`, `SET /A`, `SET /P`, quoted `SET "x=y"`,
  display forms, and trailing redirections.
- **Expansions** — `%VAR%` and `%VAR:...%` (substring/substitution), delayed
  `!VAR!`, positional `%0`–`%9`, `%*`, `%~`-modifiers (`%~dp0`, `%~$PATH:1`),
  FOR variables `%%i`, and the `%%` literal.
- **Comments** — `REM` (rest-of-line, including special characters) and `::`.
- **Escaping** — the caret `^x` escape and `^`-newline line continuation,
  double-quoted strings (quotes group; cmd does not strip them).

Keyword/command disambiguation uses tree-sitter keyword extraction, so
`set` is the keyword but `setlocal` is a command, `rem` is a comment but
`remote` is a command, etc.

## Installation & usage

### Tree-sitter CLI

```sh
npm install            # installs the CLI and builds the native binding
npx tree-sitter generate
npx tree-sitter parse path/to/script.bat
npx tree-sitter test   # run the corpus test suite
```

### Node

```js
const Parser = require('tree-sitter');
const Cmd = require('tree-sitter-cmd');

const parser = new Parser();
parser.setLanguage(Cmd);

const tree = parser.parse('@echo off\r\nif exist x (echo y) else (echo z)\r\n');
console.log(tree.rootNode.toString());
```

Rust, Python, Go and Swift bindings are also generated under `bindings/`.

## Testing

Two layers of tests:

1. **Unit corpus** — `test/corpus/*.txt`, run with `tree-sitter test`. 72
   focused cases across every construct, each with an expected S-expression.
2. **Real-world regression** — `test/real-world/` fetches a curated set of
   known-good scripts from upstream projects and asserts the error budget:

   ```sh
   bash test/real-world/fetch.sh   # download fixtures (gitignored)
   bash test/real-world/check.sh   # parse and check error counts
   ```

   The fixtures are not committed; they belong to their upstream projects under
   their own licenses (see `test/real-world/sources.tsv`).

## Known limitations

These follow from `cmd.exe` being phased and context-sensitive in ways a
single context-free pass cannot fully reproduce. None cause cascading failures
on valid scripts; they are documented in `GRAMMAR_DESIGN.md §8`.

- **Literal unquoted parentheses in arguments** (`echo (text)`,
  `echo.msg (note)`) are parsed as blocks. Quote or `^`-escape them.
- **Delayed expansion `!VAR!`** is always parsed as a reference, even where
  `SETLOCAL ENABLEDELAYEDEXPANSION` is not active (it is literal at runtime
  there).
- **`SET /A` expressions** are captured as a generic argument tail rather than a
  full arithmetic sub-grammar.
- **String interiors are opaque** — `%VAR%` inside `"…"` expands at runtime but
  is not sub-noded.
- **Line continuations** join adjacent fragments into one argument node across
  the `^`-newline.

## Layout

```
grammar.js              the grammar
src/scanner.c           external scanner (CONCAT word-join, REM keyword)
queries/                highlights.scm, injections.scm
test/corpus/            unit test corpus
test/real-world/        real-world regression harness
bindings/               node / rust / python / go / swift bindings
GRAMMAR_DESIGN.md       design document
```

## License

MIT.
