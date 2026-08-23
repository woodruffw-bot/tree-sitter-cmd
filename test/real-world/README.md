# Real-world parse-regression corpus

Known-good, real-world Windows batch/cmd scripts, checked in and parsed by CI to
guard against regressions. Unlike the unit corpus in `../corpus/` (small inputs
with expected S-expressions), these are whole upstream files. The Rust
integration test reads each file as raw bytes and requires a parse tree without
`ERROR` or `MISSING` nodes.

```
fixtures/      the committed scripts (plus a .LICENSE per file)
sources.tsv    filename and source URL for each fixture
```

Run it:

```sh
cargo test --test real_world
```

## Third-party content and licensing

Most files under `fixtures/` are third-party, included verbatim solely as test
input. They are not part of the grammar and are not covered by this repository's
MIT license; each remains under its upstream project's license. Every fixture
has a sibling `<filename>.LICENSE` recording its origin, SPDX identifier,
copyright, and a link to the full license text. Licenses currently represented:
Apache-2.0 (incl. the LLVM exception), MIT, BSD-3-Clause, Artistic-2.0, PSF-2.0,
and GPL-2.0-only. They are aggregated here for testing only.

The `mre-*.bat` files are different: they are original Minimal Reproducible
Examples authored for this repo (MIT, like the grammar). They distill idioms from
scripts whose licenses forbid vendoring the real file verbatim (for example
Elasticsearch, Elastic-2.0 / SSPL) without copying any third-party text.

## Adding a case

Add cases as new constructs or bugs surface; do not rewrite existing ones.

1. Drop the script in `fixtures/`, kept verbatim.
2. Add a `fixtures/<name>.LICENSE` sibling with its provenance and license.
3. Add a row to `sources.tsv`: `<name>\t<source-url>`.
4. Confirm `cargo test --test real_world` passes.
