# Windows cmd oracle

This opt-in harness runs short byte-exact scripts through both `cmd.exe` and
the grammar. It reports command output, exit status, the CST, and every
`ERROR` or `MISSING` node.

Run it on Windows:

```sh
cargo test --test windows_oracle -- --include-ignored --nocapture
```

The output is evidence, not an automatic conformance result. `cmd.exe`
diagnostics are localized, an exit status can come from the command rather than
the parser, and successful execution does not prove a specific CST shape.
Output is escaped byte-for-byte, including bytes from the active OEM code page,
so the report never replaces undecodable output with Unicode replacement text.
Review each observation before changing the grammar. Keep focused corpus and
Rust assertions in the PR that implements a confirmed behavior.

The initial cases cover standard separators, help forms, quiet scope, empty
blocks, standalone carriage returns, redirection spacing, caret continuations,
complete IF/FOR token boundaries, colon-comment command positions, FOR /F
source delimiters, empty SET /A expressions, and PowerShell polyglot markers.
