use tree_sitter::{Parser, Tree};

fn parse(source: &str) -> Tree {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_cmd::LANGUAGE.into())
        .expect("loading Cmd grammar");
    parser
        .parse(source, None)
        .expect("parser should return a tree")
}

#[test]
fn complete_keywords_preserve_body_and_redirect_ranges() {
    for separator in [" ", "\t", ",", ";", "="] {
        let source = format!("for %%a in{separator}(x) do{separator}echo %%a\n");
        let tree = parse(&source);
        let root = tree.root_node();
        assert!(!root.has_error(), "{source}: {}", root.to_sexp());
        let statement = root
            .named_child(0)
            .expect("fixture should have this named child");
        let body = statement
            .child_by_field_name("body")
            .expect("fixture should have a body field");
        let name = body
            .child_by_field_name("name")
            .expect("fixture should have a name field");
        assert_eq!(&source[name.byte_range()], "echo");
    }
    let source = "for %%a in (x)do>out echo %%a\n";
    let tree = parse(source);
    assert!(!tree.root_node().has_error());
    let statement = tree
        .root_node()
        .named_child(0)
        .expect("fixture should have this named child");
    let body = statement
        .child_by_field_name("body")
        .expect("fixture should have a body field");
    let redirect = body
        .child_by_field_name("redirect")
        .expect("fixture should have a redirect field");
    assert_eq!(&source[redirect.byte_range()], ">out");
    assert_eq!(
        &source[body
            .child_by_field_name("name")
            .expect("fixture should have a name field")
            .byte_range()],
        "echo"
    );
}

#[test]
fn symbol_named_for_variables_keep_declaration_and_reference_ranges() {
    for (variable, reference) in [("%%*", "%%*"), ("%%~", "%%~"), ("%%*", "%%~n*")] {
        for ending in ["\n", "\r\n"] {
            let source = format!("for {variable} in (one) do echo {reference}{ending}");
            let tree = parse(&source);
            let root = tree.root_node();
            assert!(!root.has_error(), "{source}: {}", root.to_sexp());
            let statement = root.named_child(0).expect("FOR statement");
            let declaration = statement
                .child_by_field_name("variable")
                .expect("FOR declaration");
            assert_eq!(declaration.kind(), "loop_variable_declaration");
            assert_eq!(&source[declaration.byte_range()], variable);
            let body = statement.child_by_field_name("body").expect("FOR body");
            let argument = body.child_by_field_name("argument").expect("echo argument");
            let reference_node = argument.named_child(0).expect("FOR reference");
            assert_eq!(reference_node.kind(), "loop_variable");
            assert_eq!(&source[reference_node.byte_range()], reference);
        }
    }
}

#[cfg(windows)]
#[test]
fn cmd_requires_complete_for_keywords() {
    use std::fs;
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after the Unix epoch")
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "tree-sitter-cmd-for-keywords-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir(&directory).expect("creating oracle fixture directory");
    let path = directory.join("case.cmd");
    let comspec = std::env::var_os("COMSPEC").unwrap_or_else(|| "cmd.exe".into());
    for (source, valid) in [
        ("for %%a in(x) do echo reached", false),
        ("for %%a in (x) do(echo reached)", false),
        ("for %%a in (x) do\"echo\" reached", false),
        ("for %%a in (x) do echo reached", true),
        ("for %%a in (x)do>out echo reached", true),
    ] {
        fs::write(&path, format!("@echo off\r\n{source}\r\n")).expect("writing oracle script");
        let output = Command::new(&comspec)
            .current_dir(&directory)
            .args(["/d", "/c"])
            .arg(&path)
            .output()
            .expect("running cmd.exe oracle");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        if valid {
            assert!(output.status.success(), "{source}: {stderr}");
            assert!(stderr.is_empty(), "{source}: {stderr}");
            let result = if source.contains(">out") {
                fs::read_to_string(directory.join("out")).expect("reading redirected oracle output")
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
    fs::remove_dir_all(directory).expect("removing oracle fixture directory");
}
