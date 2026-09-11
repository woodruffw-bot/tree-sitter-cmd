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
