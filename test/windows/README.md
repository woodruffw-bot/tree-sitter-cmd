# Windows cmd tests

[`tests/windows_oracle.rs`](../../tests/windows_oracle.rs) runs short scripts
through `cmd.exe`. It contains regression assertions and an optional observation
report that also parses each script with the grammar.

The report includes command output, exit status, the CST, and every `ERROR` or
missing node. Output is escaped byte for byte, including bytes from the active
OEM code page. Undecodable bytes are preserved in the report.

The observations need interpretation. Diagnostics depend on the system language,
an exit status may come from an executed command, and successful execution does
not establish a particular CST shape.

Cases include separators, help forms, echo suppression, empty blocks, carriage
returns, redirection spacing, caret continuations, IF and FOR token boundaries,
colon comments, FOR /F source delimiters, SET /A, and PowerShell comment markers.

Run instructions and guidance for grammar changes are in
[AGENTS.md](../../AGENTS.md).
