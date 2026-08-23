# Real-world feature coverage

This matrix highlights grammar-sensitive constructs in the operational and
interactive fixtures added by the corpus-breadth sweep. It is not an inventory
of every command in the full corpus. The Rust integration test requires every
fixture below to parse without `ERROR` or `MISSING` nodes.

| Fixture | Real-world role | Grammar-sensitive coverage | Preserved form |
| --- | --- | --- | --- |
| `microsoft-securecerts.bat` | Certificate ACL hardening | `ICACLS`, `WHOAMI`, `FOR /F`, unquoted `%VAR%` paths | ASCII, LF |
| `microsoft-rdsn-deploy.cmd` | Remote service deployment | UNC paths, `SCHTASKS` create/run/end/delete, dynamic `CALL :%cmd%`, `%~dp0` | ASCII, LF |
| `arkenfox-updater.bat` | Interactive configuration updater | `CHOICE`, `SHIFT`, `SET /A`, delayed expansion, nested `FOR /F` and blocks | ASCII, LF |
| `openvino-install-service.bat` | Windows service installation | `SC CREATE`/`SC CONFIG`, `SET /P`, delayed expansion inside quoted command strings | ASCII, LF |
| `microsoft-printtrace.cmd` | Elevated print diagnostics | `REG QUERY`, redirections, provider GUIDs, long argument lists | ASCII, CRLF |
| `dotnet-watsontcp-testdebug.bat` | Multi-client test orchestration | `FOR /L`, parenthesized loop body, `%1` loop bound | ASCII, CRLF |
| `microsoft-scalar-capture-perfview.bat` | Performance capture | nested `SET /A` arithmetic, modulo, comma expressions, and compound `%%=` assignment | ASCII, LF |
| `tencent-tgfx-codeformat.bat` | Recursive source formatting | `FOR /R`, nested loops, subroutine calls, `%~1` path modifiers | ASCII, LF |
| `reactos-remaster.cmd` | Interactive ISO remastering | `CHOICE`, `SET /P`, newline macro, caret continuations, delayed expansion | UTF-8 non-ASCII, LF |

## Intentional gaps

- No license-clear, reasonably small real-world `NET USE`/domain-logon script
  was selected. UNC paths are covered without checking in credentials or an
  internal enterprise logon script.
- UTF-16, OEM-code-page, BOM-prefixed, and mixed-encoding scripts are not yet
  represented. Upstream bytes are kept verbatim rather than manufacturing an
  encoding variant solely for the test corpus.
- The corpus tests syntactic acceptance, not the runtime semantics of commands
  such as `SC`, `REG`, `SCHTASKS`, or `SET /A`.
