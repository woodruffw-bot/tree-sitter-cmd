# AGENTS.md

Guidance for AI agents and human contributors working in this repository.

This is a tree-sitter grammar for the Windows `cmd.exe` batch dialect (`.bat`,
`.cmd`). The grammar lives in `grammar.js` and `src/scanner.c`; the design
rationale is in `GRAMMAR_DESIGN.md`.

## Writing style

This applies to docs, comments, commit messages, and PR descriptions.

- Use simple, clear language. Prefer short sentences and common words.
- No emdashes. Use a period, comma, colon, or parentheses instead.
- No LLM flourishes: no "delve", no "it's worth noting", no inflated adjectives
  ("comprehensive", "robust", "seamless"), no rule-of-three padding.
- State facts, not marketing. Say what something does and what it does not do.
  Skip stress-test counts, repo roll-calls, and superlatives.
- Be accurate. Verify claims against the code before writing them down, and keep
  numbers (test counts, fixture counts) current or leave them out.
- Cut anything that does not help the reader. Shorter is better when the meaning
  is the same.

## Build and test

```sh
cargo install --locked --version 0.26.11 tree-sitter-cli
tree-sitter generate --js-runtime native
tree-sitter test
bash test/real-world/check.sh
```

The CLI comes from the official Rust crate. Its bundled native runtime evaluates
`grammar.js`, so Node and npm are not required.

Always run `tree-sitter generate --js-runtime native` after editing `grammar.js`
or `src/scanner.c`, then run both test layers. CI runs the same steps
(`.github/workflows/ci.yml`).

Do not hand-edit generated files (`src/parser.c`, `src/grammar.json`,
`src/node-types.json`). Change `grammar.js` and regenerate.

## Layout

```
grammar.js          the grammar
src/scanner.c       external scanner (word-join, REM, block parens, caret escape, string end)
queries/            highlights.scm, injections.scm
test/corpus/        unit corpus (input plus expected S-expression)
test/real-world/    whole upstream scripts parsed against ERROR-node budgets
GRAMMAR_DESIGN.md   design document
bindings/           rust crate
```

## Conventions

- Indentation follows `.editorconfig`: 2 spaces for `.js`, `.scm`, JSON, and
  YAML; 4 spaces for C. Do not reformat unrelated lines.
- Follow tree-sitter naming idioms. Named node types are snake_case and
  descriptive. Reuse the field names already in use for the same role (`name`,
  `value`, `left`, `right`, `operator`, `condition`, `consequence`,
  `alternative`, `argument`, `target`, `body`, `option`, `kind`); do not add a
  second name for one role (no `arg` beside `argument`, no `op` beside
  `operator`). Helper rules that should not appear in the tree take a leading
  underscore (`_name`). Expose a `choice` of related nodes as a `supertype` when
  a query would want to match the group (for example `_expansion`). Alias
  keywords to the named `keyword` node (alias to `$.keyword`, the symbol, not the
  string `'keyword'`) so `(keyword)` matches them and they appear in the tree.
- Highlight captures in `queries/highlights.scm` use the standard capture names
  (`@keyword`, `@string`, `@operator`, `@variable`, `@comment`, `@number`,
  `@punctuation.bracket`, and so on). A new visible node should get a capture.
- When adding a construct, add a focused case to the matching `test/corpus/`
  file with its expected S-expression.
- When adding a real-world fixture, follow `test/real-world/README.md`: drop the
  verbatim script in `fixtures/`, add a `<name>.LICENSE` sibling, and add a row
  to `sources.tsv`.
- The grammar targets the batch dialect and over-accepts a few runtime-gated
  constructs on purpose. Before "fixing" an apparent quirk, check
  `GRAMMAR_DESIGN.md` to see whether it is a documented decision.

## Pull requests

- Keep changes focused. Documentation-only changes should not touch the grammar,
  and vice versa.
- State plainly in the description what changed and what did not.
