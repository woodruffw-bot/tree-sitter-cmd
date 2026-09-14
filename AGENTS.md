# AGENTS.md

Instructions for agents working on tree-sitter-cmd, a Tree-sitter grammar for
Windows batch scripts (`.bat` and `.cmd`). Keep all agent development instructions
in this file. Other documentation describes usage, behavior, and limitations.

## Grammar priorities

- Static analysis is the primary use case. Preserve source text, accurate and
  stable CST nodes and fields, and local error recovery.
- Keep syntax errors visible as Tree-sitter `ERROR` or missing nodes. Do not
  replace them with normal nodes or invent syntax to make malformed input parse.
- Model cmd syntax. Keep program flags and option payloads as opaque arguments
  unless they change statement structure or token boundaries. FOR `/R` may
  consume a path. FOR `/F` options remain opaque, including `usebackq`, `tokens=`,
  and `delims=`.
- Keep highlight and injection queries consistent with the CST. Do not add
  grammar states, nodes, aliases, or recovery rules just to simplify a query.
  Do not infer another language from a command name or its flags.
- Check `GRAMMAR_DESIGN.md` before changing a documented limitation or design
  choice. The grammar parses batch syntax without tracking runtime state.
- Do not remove the anonymous recovery markers unless the replacement preserves
  missing-body line boundaries and nested block operator scope. Test full child
  traversal and incremental parsing.

## Writing style

These rules apply to documentation, comments, commit messages, and PR descriptions.

- Use clear, precise English. Prefer short sentences and common words.
- Avoid semicolons and em dashes in prose. Preserve punctuation in code examples.
- State what the code does. Remove marketing, inflated adjectives, filler, and
  repetitive explanations.
- Verify technical claims against the code. Omit test and fixture counts unless
  the reader needs them.
- Cut text that adds no useful information.

## Build and test

Install the CLI from the official Rust crate:

```sh
cargo install --locked --version 0.26.11 tree-sitter-cli
```

Its native JavaScript runtime evaluates `grammar.js`. Node and npm are not
required.

After editing `grammar.js` or `src/scanner.c`, run:

```sh
tree-sitter generate --js-runtime native
tree-sitter test
cargo test
```

Commit the generated files. Do not edit `src/parser.c`, `src/grammar.json`, or
`src/node-types.json` by hand. The CI jobs are in `.github/workflows/ci.yml`.

Additional checks:

```sh
tree-sitter fuzz                  # Mutated inputs and incremental edits
cargo test --test real_world      # Script fixtures
```

On Windows, run the observation report with:

```sh
cargo test --test windows_oracle -- --include-ignored --nocapture
```

Review the report before drawing conclusions about cmd syntax. Command output
and exit status alone do not establish the correct CST. Add focused corpus or
Rust assertions when a grammar change implements confirmed behavior.

## Repository layout

| Path | Purpose |
| --- | --- |
| `grammar.js` | Grammar rules |
| `src/scanner.c` | Context-sensitive tokens |
| `queries/` | Highlight and injection queries |
| `test/corpus/` | Inputs and expected syntax trees |
| `test/real-world/` | Script fixtures and their metadata |
| `tests/` | Rust integration tests |
| `GRAMMAR_DESIGN.md` | Grammar behavior and design decisions |
| `bindings/rust/` | Rust bindings |

## Code conventions

- Follow `.editorconfig`: 2 spaces for JavaScript, Scheme, JSON, and YAML, and
  4 spaces for C. Do not reformat unrelated lines.
- Use descriptive snake_case node names. Reuse existing field names for the same
  role, such as `argument` and `operator`. Do not introduce synonyms such as
  `arg` or `op`.
- Prefix hidden helper rules with `_`. Use supertypes for groups of related
  nodes, such as `_expansion`.
- Alias keywords to `$.keyword`, not the string `'keyword'`, so they appear as
  named nodes and match `(keyword)` queries.
- Use standard highlight captures, such as `@keyword`, `@string`, `@operator`,
  `@variable`, and `@comment`. Add a capture only when it describes the node.
- Add a focused input and expected syntax tree in `test/corpus/` for each new
  construct.

## Script fixtures

Keep existing fixtures intact. To add a fixture:

1. Save the UTF-8 script verbatim under `test/real-world/fixtures/`.
2. Add a `<filename>.LICENSE` sibling recording its origin, SPDX identifier,
   copyright, and a link to the full license.
3. Add a row to `test/real-world/sources.tsv` as `<filename>\t<source-url>`.
   Use a GitHub `blob` URL with the full 40-character commit ID for GitHub sources.
4. Run `cargo test --test real_world`.

When a fixture needs to preserve a particular node kind, add a row to
`test/real-world/contracts.tsv` as `<filename>\t<node-kind>\t<minimum-count>`.
These checks supplement the rejection of `ERROR` and missing nodes without
fixing the entire tree shape.

## Pull requests

Keep changes focused. Keep documentation cleanup separate from grammar changes.
Explain the problem, what changed, and how the change was checked.
