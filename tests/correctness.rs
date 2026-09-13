use tree_sitter::{Node, Parser};

fn parser() -> Parser {
    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_cmd::LANGUAGE.into()).unwrap();
    parser
}

fn node_sources<'a>(node: Node<'_>, kind: &str, source: &'a str) -> Vec<&'a str> {
    let mut result = Vec::new();
    if node.kind() == kind {
        result.push(&source[node.byte_range()]);
    }
    for i in 0..node.named_child_count() {
        result.extend(node_sources(node.named_child(i as u32).unwrap(), kind, source));
    }
    result
}

#[test]
fn delayed_references_preserve_outer_command_boundaries() {
    for (source, kind, expected) in [
        ("echo !x & echo y!\n", "seq_list", "echo !x & echo y!"),
        ("echo !x | echo y!\n", "pipeline", "echo !x | echo y!"),
        ("echo !x >out!\n", "redirect_file", ">out!"),
        ("echo \"!x\" & echo \"y!\"\n", "seq_list", "echo \"!x\" & echo \"y!\""),
        ("(echo !x) & echo y!\n", "block", "(echo !x)"),
    ] {
        let tree = parser().parse(source, None).unwrap();
        let root = tree.root_node();
        assert!(!root.has_error(), "{source}: {}", root.to_sexp());
        assert_eq!(node_sources(root, kind, source), [expected]);
        assert!(node_sources(root, "delayed_variable", source).is_empty());
    }
}

#[test]
fn protected_metacharacters_remain_in_delayed_references() {
    for (source, expected) in [
        ("echo \"!a&b!\"\n", "!a&b!"),
        ("echo !a^&b!\n", "!a^&b!"),
        ("echo !ProgramFiles(x86)!\n", "!ProgramFiles(x86)!"),
        ("(echo \"!ProgramFiles(x86)!\")\n", "!ProgramFiles(x86)!"),
        ("set \"x=!a&b!\"\n", "!a&b!"),
        ("set \"_path=!_path:\"Q=!\"\n", "!_path:\"Q=!"),
        ("echo !x\"&y\"!\n", "!x\"&y\"!"),
    ] {
        let tree = parser().parse(source, None).unwrap();
        assert!(!tree.root_node().has_error());
        assert_eq!(node_sources(tree.root_node(), "delayed_variable", source), [expected]);
    }
    let tree = parser().parse("cmd 2>&!A&B!\n", None).unwrap();
    assert!(tree.root_node().has_error(), "a missing duplication target must stay invalid");
}

#[test]
fn attached_if_operands_retain_all_equals_signs() {
    for (operand, expected_right, expected_command) in [
        ("b=c", "b=c", "echo yes"),
        ("\"b\"=c", "\"b\"=c", "echo yes"),
        ("%B%=c", "%B%=c", "echo yes"),
        ("=b=c", "=b=c", "echo yes"),
        ("b^=c", "b^=c", "echo yes"),
        (" b=c", "b", "c echo yes"),
        (",b=c", "b", "c echo yes"),
    ] {
        let source = format!("if a=={operand} echo yes\n");
        let tree = parser().parse(&source, None).unwrap();
        let statement = tree.root_node().named_child(0).unwrap();
        assert!(!statement.has_error(), "{source}: {}", statement.to_sexp());
        let comparison = statement.child_by_field_name("condition").unwrap();
        let right = comparison.child_by_field_name("right").unwrap();
        let command = statement.child_by_field_name("consequence").unwrap();
        assert_eq!(&source[right.byte_range()], expected_right);
        assert_eq!(&source[command.byte_range()], expected_command);
    }
    for ending in ["", "\n", "\r\n"] {
        let source = format!("if a==b=c{ending}");
        let tree = parser().parse(&source, None).unwrap();
        assert!(tree.root_node().has_error(), "missing command: {source}");
        assert!(node_sources(tree.root_node(), "command_name", &source).is_empty());
    }
}

#[test]
fn internal_set_quotes_preserve_outer_operator_protection() {
    for (source, field, value) in [
        ("set \"x=a\"b\"c&d\"\n", "value", "a\"b\"c&d"),
        ("set \"x=a\"b\"c|d\"\n", "value", "a\"b\"c|d"),
        ("set \"x=a\"b\"c>d\"\n", "value", "a\"b\"c>d"),
        ("(set \"x=a\"b\"c)d\")\n", "value", "a\"b\"c)d"),
        ("set /p \"x=a\"b\"c&d\"\n", "prompt", "a\"b\"c&d"),
    ] {
        let tree = parser().parse(source, None).unwrap();
        let root = tree.root_node();
        assert!(!root.has_error(), "{source}: {}", root.to_sexp());
        let mut cursor = root;
        while cursor.kind() != "set_quoted" && cursor.kind() != "set_prompt" {
            cursor = if cursor.kind() == "set_statement" {
                cursor.named_child(1).unwrap()
            } else {
                cursor.named_child(0).unwrap()
            };
        }
        assert_eq!(&source[cursor.child_by_field_name(field).unwrap().byte_range()], value);
        for kind in ["command_name", "seq_list", "pipeline", "redirect_file"] {
            assert!(node_sources(root, kind, source).is_empty(), "{source}: {kind}");
        }
    }
    let source = "set \"x=a\"b&echo \"after\"\n";
    let tree = parser().parse(source, None).unwrap();
    assert!(!tree.root_node().has_error());
    assert_eq!(node_sources(tree.root_node(), "command_name", source), ["echo"]);
}

#[test]
fn redirect_filenames_stop_at_unprotected_separators() {
    for (source, redirect) in [
        ("echo hi >\"a^\",b tail\n", ">\"a^\""),
        ("echo hi >%1foo,bar echo %X%\n", ">%1foo"),
        ("echo hi >%*foo,bar echo %X%\n", ">%*foo"),
        ("echo hi >%%Afoo,bar echo %X%\n", ">%%Afoo"),
        ("echo hi >%~dp0foo,bar echo %X%\n", ">%~dp0foo"),
        ("echo hi >a^\nb,c tail\n", ">a^\nb"),
        ("echo hi >a^\r\nb,c tail\n", ">a^\r\nb"),
        ("echo hi >\"a,b\" tail\n", ">\"a,b\""),
        ("echo hi >a^,b tail\n", ">a^,b"),
        ("(echo hi >a(b,c tail)\n", ">a(b"),
    ] {
        let tree = parser().parse(source, None).unwrap();
        assert!(!tree.root_node().has_error(), "{source}: {}", tree.root_node().to_sexp());
        assert_eq!(node_sources(tree.root_node(), "redirect_file", source), [redirect]);
    }
    let source = "echo >\"out\"2>err\n";
    let tree = parser().parse(source, None).unwrap();
    assert!(!tree.root_node().has_error());
    assert_eq!(node_sources(tree.root_node(), "redirect_file", source), [">\"out\"", "2>err"]);
}

#[test]
fn continued_set_suffix_includes_the_forced_literal() {
    for newline in ["\n", "\r\n"] {
        let suffix = format!("junk^{newline}{newline}echo after");
        let source = format!("set \"x=y\"{suffix}\n");
        let tree = parser().parse(&source, None).unwrap();
        assert!(!tree.root_node().has_error());
        assert_eq!(node_sources(tree.root_node(), "set_ignored_suffix", &source), [suffix.as_str()]);
        assert!(node_sources(tree.root_node(), "command_name", &source).is_empty());
        for marker in ["&", "|", ">", "<"] {
            let suffix = format!("junk^{newline}{marker}echo after");
            let source = format!("set \"x=y\"{suffix}\n");
            let tree = parser().parse(&source, None).unwrap();
            assert!(!tree.root_node().has_error());
            assert_eq!(node_sources(tree.root_node(), "set_ignored_suffix", &source), [suffix.as_str()]);
            assert!(node_sources(tree.root_node(), "command_name", &source).is_empty());
            assert!(node_sources(tree.root_node(), "redirect_file", &source).is_empty());
        }
        let source = format!("(set \"x=y\"junk^{newline}))\n");
        let tree = parser().parse(&source, None).unwrap();
        assert!(!tree.root_node().has_error());
        assert_eq!(node_sources(tree.root_node(), "set_ignored_suffix", &source), [format!("junk^{newline})")]);
    }
    let source = "set \"x=y\"junk^\n &echo after\n";
    let tree = parser().parse(source, None).unwrap();
    assert!(!tree.root_node().has_error());
    assert_eq!(node_sources(tree.root_node(), "command_name", source), ["echo"]);
}

#[test]
fn last_set_quote_can_leave_the_ignored_suffix_quoted() {
    for suffix in ["c&d", "c|d", "c>d", "c)d", "c^&d"] {
        let source = format!("set \"x=a\"b\"{suffix}\n");
        let tree = parser().parse(&source, None).unwrap();
        let root = tree.root_node();
        assert!(!root.has_error(), "{source}: {}", root.to_sexp());
        assert_eq!(node_sources(root, "set_ignored_suffix", &source), [suffix]);
        assert!(node_sources(root, "command_name", &source).is_empty());
        assert!(node_sources(root, "redirect_file", &source).is_empty());
    }
}

#[test]
fn else_requires_a_complete_token() {
    for suffix in ["x echo no", "where", "\"x\" echo no", "(echo no)", "%X% echo no", "^ echo no", ""] {
        let source = format!("if 1==1 (echo yes) else{suffix}\n");
        let tree = parser().parse(&source, None).unwrap();
        assert!(tree.root_node().has_error(), "{source}");
    }
    for tail in [" echo no", "\techo no", ",echo no", ";echo no", "=echo no", ">out echo no"] {
        let source = format!("if 1==0 (echo yes) else{tail}\n");
        let tree = parser().parse(&source, None).unwrap();
        assert!(!tree.root_node().has_error(), "{source}: {}", tree.root_node().to_sexp());
        let statement = tree.root_node().named_child(0).unwrap();
        assert!(statement.child_by_field_name("alternative").is_some());
    }
    let source = "if 1==1 (echo yes) elsex echo no\necho tail\n";
    let tree = parser().parse(source, None).unwrap();
    assert!(tree.root_node().has_error());
    assert_eq!(node_sources(tree.root_node(), "command_name", source), ["echo", "echo"]);
}

#[test]
fn for_binder_requires_a_source_separator_before_in() {
    for separator in [" ", "\t", ",", ";", "=", " ,;= "] {
        let source = format!("for %%a{separator}in (x) do echo %%a\n");
        let tree = parser().parse(&source, None).unwrap();
        assert!(!tree.root_node().has_error(), "{source}");
        assert_eq!(node_sources(tree.root_node(), "loop_variable_declaration", &source), ["%%a"]);
    }
    for spelling in ["%%ain", "%%aIN", "%%a^ in", "%%a\"in\""] {
        let source = format!("for {spelling} (x) do echo %%a\n");
        let tree = parser().parse(&source, None).unwrap();
        assert!(tree.root_node().has_error(), "{source}");
    }
}

#[test]
fn goto_target_segments_do_not_include_removed_redirections() {
    for (source, names, redirects) in [
        ("goto foo>nul bar\n", vec!["foo", "bar"], vec![">nul"]),
        ("goto foo 2>nul bar\n", vec!["foo", "bar"], vec!["2>nul"]),
        ("goto foo>nul bar>err baz\n", vec!["foo", "bar", "baz"], vec![">nul", ">err"]),
        ("goto foo;ignored>nul more\n", vec!["foo"], vec![">nul"]),
        ("goto foo;ignored 2>nul more\n", vec!["foo"], vec!["2>nul"]),
        ("goto foo;ignored>nul more 2>err end\n", vec!["foo"], vec![">nul", "2>err"]),
        ("goto foo>nul ;ignored>err more\n", vec!["foo"], vec![">nul", ">err"]),
        ("goto ::skip>nul later\n", vec![], vec![">nul"]),
    ] {
        let tree = parser().parse(source, None).unwrap();
        let root = tree.root_node();
        assert!(!root.has_error(), "{source}: {}", root.to_sexp());
        assert_eq!(node_sources(root, "label_name", source), names);
        assert_eq!(node_sources(root, "redirect_file", source), redirects);
        assert_eq!(node_sources(root, "label_reference", source).len(), 1);
    }
    for (suffix, text, redirect) in [
        ("+2>err", "+2", ">err"),
        ("+ 2>err", "+", "2>err"),
        (";2>err", ";", "2>err"),
        ("=2>err", "=", "2>err"),
        (",2>err", ",", "2>err"),
        (":2>err", ":2", ">err"),
        ("+ignored;2>err", "+ignored;", "2>err"),
    ] {
        let source = format!("goto foo{suffix}\n");
        let tree = parser().parse(&source, None).unwrap();
        let root = tree.root_node();
        assert!(!root.has_error());
        assert_eq!(node_sources(root, "label_text", &source), [text]);
        assert_eq!(node_sources(root, "redirect_file", &source), [redirect]);
    }
    let source = "goto foo;ignored>nul more\n";
    let tree = parser().parse(source, None).unwrap();
    assert_eq!(node_sources(tree.root_node(), "label_text", source), [";ignored", "more"]);
    let source = "goto foo>nul\n";
    let tree = parser().parse(source, None).unwrap();
    let statement = tree.root_node().named_child(0).unwrap();
    assert!(!statement.has_error());
    assert!(statement.child_by_field_name("redirect").is_some());
    assert_eq!(node_sources(statement, "label_reference", source), ["foo"]);
    for suffix in ["^&echo after", "\"a&b\"", "^\n&echo after"] {
        let source = format!("goto foo;ignored>nul {suffix}\n");
        let tree = parser().parse(&source, None).unwrap();
        let root = tree.root_node();
        assert!(!root.has_error());
        assert_eq!(node_sources(root, "label_text", &source), [";ignored", suffix]);
        assert!(node_sources(root, "command_name", &source).is_empty());
    }
}
