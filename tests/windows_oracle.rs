fn escaped(bytes: &[u8]) -> String {
    bytes
        .iter()
        .copied()
        .flat_map(std::ascii::escape_default)
        .map(char::from)
        .collect()
}

#[test]
fn escaped_output_preserves_non_utf8_bytes() {
    assert_eq!(escaped(b"OEM: \x80\xff\r\n"), "OEM: \\x80\\xff\\r\\n");
}

#[cfg(windows)]
mod windows {
    use super::escaped;
    use std::ffi::OsString;
    use std::fs;
    use std::path::Path;
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    use tree_sitter::{Node, Parser};

    struct OracleCase {
        name: &'static str,
        source: &'static [u8],
    }

    fn parser() -> Parser {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_cmd::LANGUAGE.into())
            .expect("loading Cmd grammar");
        parser
    }

    fn collect_problems(node: Node<'_>, problems: &mut Vec<String>) {
        if node.is_error() || node.is_missing() {
            let kind = if node.is_missing() {
                format!("MISSING {}", node.kind())
            } else {
                "ERROR".to_owned()
            };
            problems.push(format!(
                "{kind} bytes {}..{} points {:?}..{:?}",
                node.start_byte(),
                node.end_byte(),
                node.start_position(),
                node.end_position(),
            ));
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            collect_problems(child, problems);
        }
    }

    fn run_cmd(comspec: &OsString, path: &Path) -> std::process::Output {
        Command::new(comspec)
            .current_dir(
                path.parent()
                    .expect("oracle script has a parent directory"),
            )
            .arg("/d")
            .arg("/c")
            .arg(path)
            .output()
            .unwrap_or_else(|error| panic!("running {}: {error}", path.display()))
    }

    #[test]
    #[ignore = "manual Windows oracle; output requires human interpretation"]
    fn report_cmd_observations() {
        let cases = [
            OracleCase {
                name: "standard-separators",
                source: b"@echo off\r\nif,1==1,echo separator-ok\r\n",
            },
            OracleCase {
                name: "if-help",
                source: b"if /?\r\n",
            },
            OracleCase {
                name: "for-help",
                source: b"for /?\r\n",
            },
            OracleCase {
                name: "rem-help",
                source: b"rem /?\r\n",
            },
            OracleCase {
                name: "quiet-compound",
                source: b"@echo one & echo two\r\n",
            },
            OracleCase {
                name: "empty-block",
                source: b"()\r\n@echo after\r\n",
            },
            OracleCase {
                name: "standalone-cr",
                source: b"@echo off\r\necho A\rB\r\n",
            },
            OracleCase {
                name: "redirection-spacing",
                source: b"@echo off\r\necho redirected 2>& 1\r\n",
            },
            OracleCase {
                name: "caret-and-continuation",
                source: b"@echo off\r\necho left ^\r\n&& echo right\r\n",
            },
            OracleCase {
                name: "caret-pipe-continuation",
                source: b"@echo off\r\necho left ^\r\n| echo right\r\n",
            },
            OracleCase {
                name: "if-operator-prefix",
                source: b"@echo off\r\nif left equright echo should-not-run\r\n",
            },
            OracleCase {
                name: "if-flag-prefix",
                source: b"@echo off\r\nif /ia==A echo should-not-run\r\n",
            },
            OracleCase {
                name: "if-attached-equals-remainder",
                source: b"@echo off\r\nif b===b echo should-not-run\r\n",
            },
            OracleCase {
                name: "for-flag-prefix",
                source: b"@echo off\r\nfor /ffoo %%a in (x) do echo %%a\r\n",
            },
            OracleCase {
                name: "for-r-path-prefix",
                source: b"@echo off\r\nfor /rC:\\src %%a in (x) do echo %%a\r\n",
            },
            OracleCase {
                name: "for-combined-flag-prefix",
                source: b"@echo off\r\nfor /d/r %%a in (x) do echo %%a\r\n",
            },
            OracleCase {
                name: "for-d-r-root",
                source: b"@echo off\r\nfor /d /r . %%a in (x) do echo %%a\r\n",
            },
            OracleCase {
                name: "for-r-d-root-invalid",
                source: b"@echo off\r\nfor /r /d . %%a in (x) do echo %%a\r\n",
            },
            OracleCase {
                name: "for-r-root-d",
                source: b"@echo off\r\nfor /r . /d %%a in (x) do echo %%a\r\n",
            },
            OracleCase {
                name: "colon-if-body",
                source: b"@echo off\r\nif 1==1 ::note\r\necho after\r\n",
            },
            OracleCase {
                name: "colon-for-body",
                source: b"@echo off\r\nfor %%a in (x) do ::note\r\necho after\r\n",
            },
            OracleCase {
                name: "colon-pipeline",
                source: b"@echo off\r\necho left | ::note\r\necho after\r\n",
            },
            OracleCase {
                name: "colon-and",
                source: b"@echo off\r\necho left && ::note\r\necho after\r\n",
            },
            OracleCase {
                name: "colon-quiet",
                source: b"@echo off\r\n@::note\r\necho after\r\n",
            },
            OracleCase {
                name: "for-f-unmatched-apostrophe",
                source: b"@echo off\r\nfor /f %%a in ('echo unfinished) do echo %%a\r\n",
            },
            OracleCase {
                name: "for-f-inner-apostrophe",
                source: b"@echo off\r\nfor /f %%a in ('echo it's fine') do echo %%a\r\n",
            },
            OracleCase {
                name: "for-f-usebackq-unmatched-backtick",
                source: b"@echo off\r\nfor /f \"usebackq\" %%a in (`echo unfinished) do echo %%a\r\n",
            },
            OracleCase {
                name: "set-a-empty",
                source: b"@echo off\r\nset /a\r\necho after\r\n",
            },
            OracleCase {
                name: "powershell-markers",
                source: b"<# polyglot marker\r\n#>\r\n",
            },
        ];

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before Unix epoch")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "tree-sitter-cmd-oracle-{}-{nonce}",
            std::process::id(),
        ));
        fs::create_dir(&directory)
            .unwrap_or_else(|error| panic!("creating {}: {error}", directory.display()));
        fs::create_dir_all(directory.join("x"))
            .unwrap_or_else(|error| panic!("creating oracle root entry: {error}"));
        fs::create_dir_all(directory.join("child").join("x"))
            .unwrap_or_else(|error| panic!("creating nested oracle root entry: {error}"));

        let comspec = std::env::var_os("COMSPEC").unwrap_or_else(|| OsString::from("cmd.exe"));
        for case in cases {
            let path = directory.join(format!("{}.cmd", case.name));
            fs::write(&path, case.source)
                .unwrap_or_else(|error| panic!("writing {}: {error}", path.display()));

            let tree = parser()
                .parse(case.source, None)
                .expect("parser returned no tree");
            let mut problems = Vec::new();
            collect_problems(tree.root_node(), &mut problems);
            let output = run_cmd(&comspec, &path);

            println!("case: {}", case.name);
            println!("source: {}", escaped(case.source));
            println!("cmd status: {:?}", output.status.code());
            println!("cmd stdout: {}", escaped(&output.stdout));
            println!("cmd stderr: {}", escaped(&output.stderr));
            println!("cst: {}", tree.root_node().to_sexp());
            println!("cst has_error: {}", tree.root_node().has_error());
            println!("cst problems: {problems:?}");
            println!();
        }

        fs::remove_dir_all(&directory)
            .unwrap_or_else(|error| panic!("removing {}: {error}", directory.display()));
    }
}
