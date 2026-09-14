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

    fn run_script(name: &str, source: &[u8]) -> std::process::Output {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "tree-sitter-cmd-{name}-{}-{nonce}", std::process::id(),
        ));
        fs::create_dir(&directory).unwrap();
        let path = directory.join("case.cmd");
        fs::write(&path, source).unwrap();
        let comspec = std::env::var_os("COMSPEC").unwrap_or_else(|| OsString::from("cmd.exe"));
        let output = run_cmd(&comspec, &path);
        fs::remove_dir_all(&directory).unwrap();
        output
    }

    #[test]
    fn delayed_bangs_do_not_protect_outer_operators() {
        for (mode, expected) in [
            ("EnableDelayedExpansion", vec!["left", "right", "\"left\"", "\"right\""]),
            ("DisableDelayedExpansion", vec!["left!", "right!", "\"!left\"", "\"right!\""]),
        ] {
            let source = format!(
                "@echo off\r\nsetlocal {mode}\r\necho left! & echo right!\r\necho \"!left\" & echo \"right!\"\r\n",
            );
            let output = run_script("delayed-boundaries", source.as_bytes());
            assert!(output.status.success(), "{}", escaped(&output.stderr));
            assert!(output.stderr.is_empty(), "{}", escaped(&output.stderr));
            let stdout = String::from_utf8(output.stdout).unwrap();
            assert_eq!(stdout.lines().map(str::trim_end).collect::<Vec<_>>(), expected);
        }
    }

    #[test]
    fn attached_if_rhs_retains_later_equals() {
        let output = run_script("if-equals", b"@echo off\r\nif not b==b=c echo retained\r\nif b==b=c echo unexpected\r\necho done\r\n");
        assert!(output.status.success(), "{}", escaped(&output.stderr));
        assert!(output.stderr.is_empty(), "{}", escaped(&output.stderr));
        assert_eq!(String::from_utf8(output.stdout).unwrap().lines().collect::<Vec<_>>(), ["retained", "done"]);
    }

    #[test]
    fn internal_set_quotes_protect_metacharacters() {
        let output = run_script("set-quotes", b"@echo off\r\nsetlocal EnableDelayedExpansion\r\nset \"TS_CMD_QUOTE=a\"b\"c&d\"\r\necho !TS_CMD_QUOTE!\r\nset \"TS_CMD_QUOTE=a\"b\"c|d\"\r\necho !TS_CMD_QUOTE!\r\nset \"TS_CMD_QUOTE=a\"b\"c>d\"\r\necho !TS_CMD_QUOTE!\r\n");
        assert!(output.status.success(), "{}", escaped(&output.stderr));
        assert!(output.stderr.is_empty(), "{}", escaped(&output.stderr));
        assert_eq!(String::from_utf8(output.stdout).unwrap().lines().collect::<Vec<_>>(), ["a\"b\"c&d", "a\"b\"c|d", "a\"b\"c>d"]);
    }

    #[test]
    fn caret_escaped_set_quotes_leave_operators_active() {
        let output = run_script("set-caret-quotes", b"@echo off\r\nsetlocal DisableDelayedExpansion\r\nset ^\"TS_CMD_CARET=left&echo operator^\"\r\nset TS_CMD_CARET\r\nset ^\"TS_CMD_CARET=left>caret.out tail^\"\r\nif exist caret.out echo redirected\r\nset TS_CMD_CARET\r\nset ^\"TS_CMD_CARET=left^&right^\"\r\nset TS_CMD_CARET\r\nset TS_CMD_SOURCE=expanded\r\nset ^\"TS_CMD_CARET=%TS_CMD_SOURCE%^\"\r\nset TS_CMD_CARET\r\n");
        assert!(output.status.success(), "{}", escaped(&output.stderr));
        assert!(output.stderr.is_empty(), "{}", escaped(&output.stderr));
        assert_eq!(
            String::from_utf8(output.stdout).unwrap().lines().collect::<Vec<_>>(),
            ["operator\"", "TS_CMD_CARET=left", "redirected", "TS_CMD_CARET=left tail", "TS_CMD_CARET=left&right", "TS_CMD_CARET=expanded"],
        );
    }

    #[test]
    fn quotes_inside_caret_set_delayed_text_protect_later_operators() {
        for mode in ["EnableDelayedExpansion", "DisableDelayedExpansion"] {
            let source = format!(
                "@echo off\r\nsetlocal {mode}\r\nset ^\"TS_CMD_ODD=!TS_CMD_MISSING\"suffix!&echo unexpected^\"\r\nset ^\"TS_CMD_ODD=!TS_CMD_MISSING\"suffix!|echo unexpected^\"\r\nset ^\"TS_CMD_ODD=!TS_CMD_MISSING\"suffix!>unexpected.txt^\"\r\nif exist unexpected.txt echo unexpected\r\n(\r\nset ^\"TS_CMD_ODD=!TS_CMD_MISSING\"suffix!)&echo unexpected^\"\r\n)\r\necho done\r\n",
            );
            let output = run_script("set-delayed-quotes", source.as_bytes());
            assert!(output.status.success(), "{}", escaped(&output.stderr));
            assert!(output.stderr.is_empty(), "{}", escaped(&output.stderr));
            assert_eq!(String::from_utf8(output.stdout).unwrap().trim_end(), "done");
        }
    }

    #[test]
    fn delayed_quotes_in_caret_set_redirect_targets_protect_operators() {
        let output = run_script("set-redirect-delayed-quotes", b"@echo off\r\nsetlocal DisableDelayedExpansion\r\nset ^\"TS_CMD_TARGET=ok>!a\"b!&echo unexpected^\"\r\nset ^\"TS_CMD_TARGET=ok>!a\"b! !c!&echo unexpected^\"\r\nset ^\"TS_CMD_TARGET=ok>!a\"b!\"&echo visible\r\necho done\r\n");
        assert!(output.status.success(), "{}", escaped(&output.stderr));
        assert!(output.stderr.is_empty(), "{}", escaped(&output.stderr));
        assert_eq!(String::from_utf8(output.stdout).unwrap().lines().collect::<Vec<_>>(), ["visible", "done"]);
    }

    #[test]
    fn redirect_filename_separators_follow_quotes_and_continuations() {
        let output = run_script("redirect-separators", b"@echo off\r\necho marker >\"a^\",b\r\nif exist \"a^\" echo quoted\r\nif exist \"a^,b\" echo unexpected\r\necho marker >a^\r\nb,c\r\nif exist ab echo continued\r\nif exist \"ab,c\" echo unexpected\r\n");
        assert!(output.status.success(), "{}", escaped(&output.stderr));
        assert!(output.stderr.is_empty(), "{}", escaped(&output.stderr));
        assert_eq!(String::from_utf8(output.stdout).unwrap().lines().collect::<Vec<_>>(), ["quoted", "continued"]);
    }

    #[test]
    fn continued_set_suffix_does_not_start_a_command() {
        for marker in ["&", "|", ">", "\r\n"] {
            let source = format!("@echo off\r\nset \"TS_CMD_SUFFIX=y\"junk^\r\n{marker}echo unexpected\r\necho done\r\n");
            let output = run_script("set-suffix", source.as_bytes());
            assert!(output.status.success(), "{}", escaped(&output.stderr));
            assert!(output.stderr.is_empty(), "{}", escaped(&output.stderr));
            assert_eq!(String::from_utf8(output.stdout).unwrap().trim_end(), "done");
        }
    }

    #[test]
    fn last_set_quote_can_open_the_ignored_suffix() {
        let output = run_script("set-open-suffix", b"@echo off\r\nsetlocal EnableDelayedExpansion\r\nset \"TS_CMD_QUOTE=a\"b\"c&d\r\necho !TS_CMD_QUOTE!\r\n");
        assert!(output.status.success(), "{}", escaped(&output.stderr));
        assert!(output.stderr.is_empty(), "{}", escaped(&output.stderr));
        assert_eq!(String::from_utf8(output.stdout).unwrap().trim_end(), "a\"b");
    }

    #[test]
    fn else_in_a_command_tail_is_literal() {
        let output = run_script("else-tail", b"@echo off\r\nsetlocal\r\nif 1==1 echo someone else here\r\nif 1==0 echo yes else echo unexpected\r\nif 1==1 set TS_CMD_ELSE=else\r\necho %TS_CMD_ELSE%\r\nif 1==0 (echo unexpected) else echo branch\r\nif 1==0 echo first |(echo last) else echo pipeline-branch\r\n");
        assert!(output.status.success(), "{}", escaped(&output.stderr));
        assert!(output.stderr.is_empty(), "{}", escaped(&output.stderr));
        assert_eq!(String::from_utf8(output.stdout).unwrap().lines().collect::<Vec<_>>(), ["someone else here", "else", "branch", "pipeline-branch"]);
    }

    #[test]
    fn else_does_not_match_a_longer_word() {
        let invalid = run_script("else-prefix", b"@echo off\r\nif 1==1 (echo yes) elsex echo no\r\n");
        assert!(!invalid.status.success());
        assert!(!invalid.stderr.is_empty());
        let valid = run_script("else-token", b"@echo off\r\nif 1==0 (echo yes) else echo no\r\n");
        assert!(valid.status.success(), "{}", escaped(&valid.stderr));
        assert!(valid.stderr.is_empty());
        assert_eq!(String::from_utf8(valid.stdout).unwrap().trim_end(), "no");
    }

    #[test]
    fn for_requires_a_delimiter_after_its_binder() {
        let invalid = run_script("for-binder-prefix", b"@echo off\r\nfor %%ain (x) do echo %%a\r\n");
        assert!(!invalid.status.success());
        assert!(!invalid.stderr.is_empty());
        let valid = run_script("for-binder-token", b"@echo off\r\nfor %%a in (x) do echo %%a\r\n");
        assert!(valid.status.success(), "{}", escaped(&valid.stderr));
        assert!(valid.stderr.is_empty());
        assert_eq!(String::from_utf8(valid.stdout).unwrap().trim_end(), "x");
    }

    #[test]
    fn goto_tail_descriptors_follow_cmd_token_separators() {
        for (suffix, redirects_stderr) in [
            ("+2>nul", false),
            ("+ 2>nul", true),
            (";2>nul", true),
            ("=2>nul", true),
            (",2>nul", true),
            (":2>nul", false),
            ("+ignored;2>nul", true),
        ] {
            let source = format!("@echo off\r\ngoto TS_CMD_MISSING{suffix}\r\n");
            let output = run_script("goto-tail-descriptor", source.as_bytes());
            assert!(!output.status.success(), "{suffix}");
            assert_eq!(output.stderr.is_empty(), redirects_stderr, "{suffix}: {}", escaped(&output.stderr));
        }
    }

    #[test]
    fn goto_accepts_text_after_a_redirection() {
        for target in ["destination>nul tail", "destination 2>nul tail", "destination;ignored>nul more"] {
            let source = format!("@echo off\r\ngoto {target}\r\necho unexpected\r\nexit /b 1\r\n:destination tail\r\n:destination\r\necho reached\r\n");
            let output = run_script("goto-redirect-tail", source.as_bytes());
            assert!(output.status.success(), "{}", escaped(&output.stderr));
            assert!(output.stderr.is_empty(), "{}", escaped(&output.stderr));
            assert_eq!(String::from_utf8(output.stdout).unwrap().trim_end(), "reached");
        }
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
