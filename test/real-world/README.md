# Real-world parse-regression corpus

A growing collection of **known-good, real-world** Windows batch/cmd scripts,
checked in and parsed by CI to guard against regressions. Unlike the unit
corpus in `../corpus/` (small inputs with expected S-expressions), these are
whole upstream files; the only assertion is that each parses within its
**ERROR-node budget** (0 for everything currently).

```
fixtures/                       the committed scripts (+ a .LICENSE per file)
sources.tsv                     budget + filename + source URL for each fixture
check.sh                        parse every fixture, fail if over budget
```

Run it:

```sh
bash test/real-world/check.sh
```

## Third-party content & licensing

Most files under `fixtures/` are **third-party, included verbatim solely as
test input**. They are *not* part of the grammar and are *not* covered by this
repository's MIT license — each remains under its upstream project's license.
Every fixture has a sibling `<filename>.LICENSE` recording its origin, SPDX
license identifier, copyright, and a link to the full license text.

Licenses currently represented: Apache-2.0 (incl. LLVM exception), MIT (incl.
the `npocmaka/batch.scripts` cmd torture-test collection), BSD-3-Clause,
Artistic-2.0, PSF-2.0, and GPL-2.0-only (`gfw-vcpkg-copy-dlls.bat`, from Git for
Windows). They are aggregated here for testing only ("mere aggregation"); see
each `.LICENSE` for terms.

The `mre-*.bat` files are different: they are **original** Minimal Reproducible
Examples authored for this repo (MIT, like the rest of the grammar). They
distill idioms from scripts whose licenses forbid vendoring the real file
verbatim (e.g. Elasticsearch, Elastic-2.0 / SSPL) without copying any
third-party text. Their `.LICENSE` sidecars note this provenance.

## Adding a case

The corpus is meant to **accrete over time** — add cases as new constructs or
bugs surface; don't rewrite existing ones.

1. Drop the script in `fixtures/` (keep it verbatim; a descriptive name is fine).
2. Add a `fixtures/<name>.LICENSE` sibling with its provenance and license.
3. Add a row to `sources.tsv`: `<budget>\t<name>\t<source-url>` (budget `0`
   unless it exercises a documented limitation).
4. Confirm `bash test/real-world/check.sh` passes.
