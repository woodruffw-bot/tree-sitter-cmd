use tree_sitter::Parser;

fn parse(source: &str) -> tree_sitter::Tree {
    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_cmd::LANGUAGE.into()).unwrap();
    parser.parse(source, None).unwrap()
}

#[test]
fn in_and_do_require_complete_tokens() {
    for source in [
        "for %%a in(x) do echo %%a\n",
        "for %%a in (x) do(echo %%a)\n",
        "for %%a in (x) do\"echo\" %%a\n",
    ] {
        let tree = parse(source);
        assert!(tree.root_node().has_error(), "{source}: {}", tree.root_node().to_sexp());
    }
}

#[test]
fn complete_keywords_preserve_body_and_redirect_ranges() {
    for separator in [" ", "\t", ",", ";", "="] {
        let source = format!("for %%a in{separator}(x) do{separator}echo %%a\n");
        let tree = parse(&source);
        let root = tree.root_node();
        assert!(!root.has_error(), "{source}: {}", root.to_sexp());
        let statement = root.named_child(0).unwrap();
        let body = statement.child_by_field_name("body").unwrap();
        let name = body.child_by_field_name("name").unwrap();
        assert_eq!(&source[name.byte_range()], "echo");
    }
    let source = "for %%a in (x)do>out echo %%a\n";
    let tree = parse(source);
    assert!(!tree.root_node().has_error());
    let statement = tree.root_node().named_child(0).unwrap();
    let body = statement.child_by_field_name("body").unwrap();
    let redirect = body.child_by_field_name("redirect").unwrap();
    assert_eq!(&source[redirect.byte_range()], ">out");
    assert_eq!(&source[body.child_by_field_name("name").unwrap().byte_range()], "echo");
}

#[cfg(windows)]
#[test]
fn cmd_requires_complete_for_keywords() {
    use std::fs;
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let directory = std::env::temp_dir().join(format!("tree-sitter-cmd-for-keywords-{}-{nonce}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    let path = directory.join("case.cmd");
    let comspec = std::env::var_os("COMSPEC").unwrap_or_else(|| "cmd.exe".into());
    for (source, valid) in [
        ("for %%a in(x) do echo reached", false),
        ("for %%a in (x) do(echo reached)", false),
        ("for %%a in (x) do\"echo\" reached", false),
        ("for %%a in (x) do echo reached", true),
        ("for %%a in (x)do>out echo reached", true),
    ] {
        fs::write(&path, format!("@echo off\r\n{source}\r\n")).unwrap();
        let output = Command::new(&comspec).current_dir(&directory)
            .args(["/d", "/c"]).arg(&path).output().unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        if valid {
            assert!(output.status.success(), "{source}: {stderr}");
            assert!(stderr.is_empty(), "{source}: {stderr}");
            let result = if source.contains(">out") {
                fs::read_to_string(directory.join("out")).unwrap()
            } else {
                stdout.into_owned()
            };
            assert_eq!(result.trim_end(), "reached", "{source}");
        } else {
            assert!(!output.status.success(), "{source}: {stdout}");
            assert!(!stderr.is_empty(), "{source}");
            assert!(!stdout.contains("reached"), "{source}: {stdout}");
        }
    }
    fs::remove_dir_all(directory).unwrap();
}
