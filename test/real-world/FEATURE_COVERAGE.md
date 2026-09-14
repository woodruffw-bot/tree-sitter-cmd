# Script fixture coverage

These fixtures cover selected batch syntax in complete scripts. The integration
test rejects `ERROR` and missing nodes. It does not check the runtime behavior of
the commands or parse option languages such as `SET /A` arithmetic.

| Fixture | Purpose | Syntax examples | Encoding and line endings |
| --- | --- | --- | --- |
| `microsoft-securecerts.bat` | Certificate permissions | `FOR /F`, unquoted `%VAR%` paths | ASCII, LF |
| `microsoft-rdsn-deploy.cmd` | Service deployment | UNC paths, dynamic `CALL :%cmd%`, `%~dp0` | ASCII, LF |
| `arkenfox-updater.bat` | Interactive configuration updates | `SET /A`, delayed expansion, nested `FOR /F` and blocks | ASCII, LF |
| `openvino-install-service.bat` | Service installation | `SET /P`, delayed expansion inside quoted strings | ASCII, LF |
| `microsoft-printtrace.cmd` | Print diagnostics | Redirections and long argument lists | ASCII, CRLF |
| `dotnet-watsontcp-testdebug.bat` | Client tests | `FOR /L`, a loop body in parentheses, `%1` as the loop bound | ASCII, CRLF |
| `microsoft-scalar-capture-perfview.bat` | Performance capture | `SET /A` arguments with parentheses, modulo, commas, and `%%=` | ASCII, LF |
| `tencent-tgfx-codeformat.bat` | Source formatting | `FOR /R`, nested loops, subroutine calls, `%~1` modifiers | ASCII, LF |
| `reactos-remaster.cmd` | ISO remastering | `SET /P`, newline macro, caret continuations, delayed expansion | UTF-8 with non-ASCII text, LF |

## Gaps

The corpus has no `NET USE` or domain logon fixture. Other fixtures cover UNC
paths. UTF-16, OEM code pages, byte-order marks, and mixed encodings are not
represented in this corpus.
