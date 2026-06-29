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

Stress-tested against 160+ well-known real-world batch scripts from across the
ecosystem. The committed regression corpus (`test/real-world/`) holds **37 of
them — all parsing with zero error nodes**, including the notoriously tricky
`gradlew.bat`, the 333-line `mvn.cmd`, a 579-line LLVM release script, Node's
989-line `vcbuild.bat`, classics from Apache Tomcat / Kafka / Spark / Ant, Go,
Flutter, CPython, pyenv-win and .NET, and a curated slice of the
[`npocmaka/batch.scripts`](https://github.com/npocmaka/batch.scripts) cmd
torture-test collection:

| Script | Lines | Errors |
|--------|------:|-------:|
| `vcbuild.bat` (Node.js) | 989 | 0 |
| `build_llvm_release.bat` (LLVM) | 579 | 0 |
| `kafka-run-class.bat` (Apache Kafka) | — | 0 |
| `catalina.bat` (Apache Tomcat) | 330 | 0 |
| `mvn.cmd` (Maven), `ant.bat` (Apache Ant) | — | 0 |
| `make.bat` (Go), `flutter.bat` (Flutter) | — | 0 |
| `gradlew.bat`, `build.bat` (CPython), `pyenv.bat`, … | — | 0 |

The broader sweep surfaced (and fixed) several real edge cases — SET names
containing expansions (`set err%%i=`), the `set /p=` print-without-newline
idiom, redirections on `call`, quoted SET values with embedded quotes
(`set "x="y" !z!"`), caret-escaped unquoted `FOR /F` options
(`for /f tokens^=2-5^ delims^=.-_ %%j in (...)`), stacked `@@` quiet prefixes,
non-comma `FOR /L` separators (`(1;1=5)`), single-character `FOR` variables of
any kind (`%%#`, `%%1`), and — most importantly — the cmd-accurate block-vs-
literal parenthesis model that makes the `(echo()` robust blank-line idiom and
`echo (parenthesised)` arguments parse the way cmd actually runs them.

Where vendoring a script's license forbids checking it in (e.g. Elasticsearch
is Elastic-2.0 / SSPL), the corpus instead carries an independent, hand-authored
**MRE** (`fixtures/mre-*.bat`) that distills the same idioms without copying any
third-party text.

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
- **Context-sensitive parentheses** — `(` opens a block only where a command is
  expected; in an argument it is a literal character that does **not** nest
  (`echo (text)`). While a block is open, the first unescaped `)` closes it — a
  literal `(` in an argument never protects a later `)`, exactly as in cmd
  (which is why cmd needs `^)` to echo a close-paren inside a block, and why the
  `(echo()` blank-line idiom is a block whose body is the command `echo(`).
  Tracked by the external scanner as a block-depth counter.

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

1. **Unit corpus** — `test/corpus/*.txt`, run with `tree-sitter test`. 83
   focused cases across every construct, each with an expected S-expression.
2. **Real-world regression** — `test/real-world/` is a committed corpus of
   known-good upstream scripts, parsed against per-file ERROR-node budgets:

   ```sh
   bash test/real-world/check.sh   # parse fixtures, fail if over budget
   ```

   The fixtures are third-party, included verbatim as test input only; each has
   a sibling `<file>.LICENSE` recording its origin and license (the corpus
   spans Apache-2.0, MIT, BSD-3, Artistic-2.0, PSF and GPL-2.0). Scripts whose
   licenses forbid vendoring are represented by original `mre-*.bat` MREs
   instead. See `test/real-world/README.md`.

## Known limitations

These follow from `cmd.exe` being phased and context-sensitive in ways a
single context-free pass cannot fully reproduce. None cause cascading failures
on valid scripts; they are documented in `GRAMMAR_DESIGN.md §8`.

- **Delayed expansion `!VAR!`** is always parsed as a reference, even where
  `SETLOCAL ENABLEDELAYEDEXPANSION` is not active (it is literal at runtime
  there).
- **`SET /A` expressions** are captured as a generic argument tail rather than a
  full arithmetic sub-grammar.
- **String interiors are opaque** — `%VAR%` inside `"…"` expands at runtime but
  is not sub-noded.
- **Line continuations** join adjacent fragments into one argument node across
  the `^`-newline.
- **`SET name=value` with spaces in the name** (`set sim salabim=x`, a valid but
  rare cmd form) is not modelled; the name token stops at whitespace.
- **Caret-escaped `%VAR%` inside `FOR /F` options** (`for /f eol^=^%LF%%LF%^ …`)
  and variables whose name is a literal newline (`%\n%` macros) are not
  supported — both are torture-test tricks rather than mainstream batch.
- **An escaped `^)` inside a multi-line block** can still mis-close the block in
  some positions (the literal-paren-in-block edge of the model above).

## Layout

```
grammar.js              the grammar
src/scanner.c           external scanner (CONCAT word-join, REM, block parens)
queries/                highlights.scm, injections.scm
test/corpus/            unit test corpus
test/real-world/        real-world regression harness
bindings/               node / rust / python / go / swift bindings
GRAMMAR_DESIGN.md       design document
```

## License

MIT.
