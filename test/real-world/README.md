# Script regression tests

The Rust integration test parses complete batch scripts from `fixtures/`. It
validates UTF-8, parses the original bytes, and rejects trees containing `ERROR`
or missing nodes. The smaller inputs in [`../corpus/`](../corpus/) also specify
expected syntax trees.

| Path | Contents |
| --- | --- |
| `fixtures/` | Scripts and a `.LICENSE` file for each script |
| `sources.tsv` | Fixture filenames and source URLs |
| `contracts.tsv` | Minimum counts of selected node kinds |
| [FEATURE_COVERAGE.md](FEATURE_COVERAGE.md) | Syntax examples and coverage gaps |

Each `sources.tsv` row has the form `<filename>\t<source-url>`. Each
`contracts.tsv` row has the form `<filename>\t<node-kind>\t<minimum-count>`.
The node counts check selected parts of a tree without fixing its full shape.

## Sources and licenses

Most fixtures are verbatim copies of third-party scripts used as test input.
Each remains under its upstream license. Its `<filename>.LICENSE` file records
the origin, SPDX identifier, copyright, and a link to the full license.

The `mre-*.bat` fixtures are original examples written for this repository and
covered by its MIT license. They reproduce batch idioms without copying
third-party scripts.

Instructions for running the tests and adding fixtures are in
[AGENTS.md](../../AGENTS.md).
