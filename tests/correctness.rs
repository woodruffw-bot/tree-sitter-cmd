use tree_sitter::{Node, Parser};

fn parser() -> Parser {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_cmd::LANGUAGE.into())
        .expect("loading Cmd grammar");
    parser
}

#[test]
fn unquoted_set_names_keep_post_redirect_whitespace() {
    for (prefix, field) in [("set ", "value"), ("set /p ", "prompt")] {
        for ending in ["\n", "\r\n"] {
            for gap in [" ", "\t", " \t "] {
                for tail in ["name", "", "%V%tail"] {
                    let source = format!("{prefix}head>nul{gap}{tail}=value{ending}");
                    let tree = parser().parse(&source, None).expect("SET tree");
                    let root = tree.root_node();
                    assert!(!root.has_error(), "{source}: {}", root.to_sexp());
                    let binding = root
                        .named_child(0)
                        .expect("SET statement")
                        .named_child(1)
                        .expect("SET binding");
                    let mut cursor = binding.walk();
                    let names: Vec<_> = binding
                        .children_by_field_name("name", &mut cursor)
                        .map(|node| &source[node.byte_range()])
                        .collect();
                    assert_eq!(names, ["head", &format!("{gap}{tail}")], "{source}");
                    let redirect = binding
                        .child_by_field_name("redirect")
                        .expect("name redirect");
                    assert_eq!(&source[redirect.byte_range()], ">nul");
                    let value = binding.child_by_field_name(field).expect("value or prompt");
                    assert_eq!(&source[value.byte_range()], "value");
                }
            }
        }
    }
    for gap in [" ", "\t", " \t "] {
        let source = format!("set head>nul{gap}tail\n");
        let tree = parser().parse(&source, None).expect("display tree");
        assert!(!tree.root_node().has_error());
        assert_eq!(
            node_sources(tree.root_node(), "variable_name", &source),
            ["head", &format!("{gap}tail")]
        );
    }
}

#[test]
fn quoted_set_names_trim_initial_horizontal_space() {
    for (prefix, kind, field) in [
        ("set ", "set_quoted", "value"),
        ("set /p ", "set_prompt", "prompt"),
    ] {
        for gap in [" ", "\t", " \t "] {
            for ending in ["\n", "\r\n"] {
                let source = format!("{prefix}\"{gap}name=value\"{ending}");
                let tree = parser().parse(&source, None).expect("quoted SET tree");
                let root = tree.root_node();
                assert!(!root.has_error(), "{source}: {}", root.to_sexp());
                let binding = root
                    .named_child(0)
                    .expect("SET statement")
                    .named_child(1)
                    .expect("SET binding");
                assert_eq!(binding.kind(), kind);
                assert_eq!(binding.start_byte(), "set ".len());
                let name = binding.child_by_field_name("name").expect("trimmed name");
                assert_eq!(&source[name.byte_range()], "name");
                assert_eq!(name.start_byte(), prefix.len() + 1 + gap.len());
                let value = binding.child_by_field_name(field).expect("value or prompt");
                assert_eq!(&source[value.byte_range()], "value");
            }
        }
    }
}

#[test]
fn active_escaped_equals_delimit_set_bindings() {
    for (prefix, kind, field) in [
        ("set ", "set_assignment", "value"),
        ("set /p ", "set_prompt", "prompt"),
    ] {
        for operator in ["^=", "^\n=", "^\r\n="] {
            for ending in ["\n", "\r\n"] {
                let source = format!("{prefix}name{operator}value=tail{ending}");
                let tree = parser().parse(&source, None).expect("SET tree");
                let root = tree.root_node();
                assert!(!root.has_error(), "{source}: {}", root.to_sexp());
                let binding = root
                    .named_child(0)
                    .expect("SET statement")
                    .named_child(1)
                    .expect("SET binding");
                assert_eq!(binding.kind(), kind);
                assert_eq!(
                    &source[binding
                        .child_by_field_name("name")
                        .expect("binding name")
                        .byte_range()],
                    "name"
                );
                let values: Vec<_> = binding
                    .children_by_field_name(field, &mut binding.walk())
                    .map(|node| &source[node.byte_range()])
                    .collect();
                assert_eq!(values.concat(), "value=tail");
                assert_eq!(
                    node_sources(binding, "escape_sequence", &source),
                    [operator]
                );
            }
        }
    }
    for (source, name) in [
        ("set \"a^=x\"\n", "a^"),
        ("set \"a\"b^=x\"\n", "a\"b"),
        ("set name^^=x\n", "name^^"),
    ] {
        let tree = parser().parse(source, None).expect("quoted SET tree");
        let root = tree.root_node();
        assert!(!root.has_error(), "{source}: {}", root.to_sexp());
        assert_eq!(node_sources(root, "variable_name", source), [name]);
        let binding = root
            .named_child(0)
            .expect("SET statement")
            .named_child(1)
            .expect("quoted assignment");
        assert_eq!(
            &source[binding
                .child_by_field_name("value")
                .expect("quoted value")
                .byte_range()],
            "x"
        );
    }
    for (source, gap, field) in [
        ("set head>nul ^=value\n", " ", "value"),
        ("set /p head>nul\t^=value\n", "\t", "prompt"),
    ] {
        let tree = parser().parse(source, None).expect("redirected SET tree");
        let root = tree.root_node();
        assert!(!root.has_error(), "{source}: {}", root.to_sexp());
        let binding = root
            .named_child(0)
            .expect("SET statement")
            .named_child(1)
            .expect("SET binding");
        let names: Vec<_> = binding
            .children_by_field_name("name", &mut binding.walk())
            .map(|node| &source[node.byte_range()])
            .collect();
        assert_eq!(names, ["head", gap]);
        assert_eq!(
            &source[binding
                .child_by_field_name(field)
                .expect("value or prompt")
                .byte_range()],
            "value"
        );
    }
}

fn node_sources<'a>(node: Node<'_>, kind: &str, source: &'a str) -> Vec<&'a str> {
    let mut result = Vec::new();
    if node.kind() == kind {
        result.push(&source[node.byte_range()]);
    }
    for i in 0..node.named_child_count() {
        result.extend(node_sources(
            node.named_child(i as u32)
                .expect("fixture should have this named child"),
            kind,
            source,
        ));
    }
    result
}

#[test]
fn set_names_preserve_literal_parentheses_and_quoted_fragments() {
    for name in [
        "x(y)",
        "x\"y\"",
        "x\"a&b\"",
        "x\"%V%\"",
        "x(1) tail",
        "x\"a^&b\"",
    ] {
        for (prefix, payload, field) in
            [("set ", "value", "value"), ("set /p ", "prompt", "prompt")]
        {
            let source = format!("{prefix}{name}={payload}\n");
            let tree = parser()
                .parse(&source, None)
                .expect("parser should return a tree");
            let root = tree.root_node();
            assert!(!root.has_error(), "{source}: {}", root.to_sexp());
            assert_eq!(node_sources(root, "variable_name", &source), [name]);
            assert!(node_sources(root, "command_name", &source).is_empty());
            let binding = root
                .named_child(0)
                .expect("fixture should have this named child")
                .named_child(1)
                .expect("fixture should have this named child");
            assert_eq!(
                &source[binding
                    .child_by_field_name(field)
                    .expect("fixture should have the requested field")
                    .byte_range()],
                payload
            );
        }
        let source = format!("set {name}\n");
        let tree = parser()
            .parse(&source, None)
            .expect("parser should return a tree");
        assert!(!tree.root_node().has_error(), "{source}");
        assert_eq!(
            node_sources(tree.root_node(), "variable_name", &source),
            [name]
        );
    }
    let source = "(set x(y)\necho after\n";
    let tree = parser()
        .parse(source, None)
        .expect("parser should return a tree");
    assert!(!tree.root_node().has_error());
    assert_eq!(
        node_sources(tree.root_node(), "block", source),
        ["(set x(y)"]
    );
    assert_eq!(
        node_sources(tree.root_node(), "variable_name", source),
        ["x(y"]
    );
    assert_eq!(
        node_sources(tree.root_node(), "command_name", source),
        ["echo"]
    );
}

#[test]
fn set_name_strings_preserve_protected_delayed_references() {
    for reference in ["!a&b!", "!a|b!", "!a<b!", "!a>b!", "!a)b!"] {
        for (prefix, suffix) in [("set ", "=value"), ("set /p ", "=prompt"), ("set ", "")] {
            let name = format!("x\"{reference}\"");
            let source = format!("{prefix}{name}{suffix}\n");
            let tree = parser()
                .parse(&source, None)
                .expect("parser should return a tree");
            let root = tree.root_node();
            assert!(!root.has_error(), "{source}: {}", root.to_sexp());
            assert_eq!(
                node_sources(root, "variable_name", &source),
                [name.as_str()]
            );
            assert_eq!(node_sources(root, "delayed_variable", &source), [reference]);
            assert!(node_sources(root, "command_name", &source).is_empty());
        }
    }
}

#[test]
fn argument_separators_expose_redirection_descriptors() {
    for prefix in ["a,", "a;", "a=", "%V%=", "\"a\"=", "a^b="] {
        for operator in [">", ">>", "<", ">&"] {
            let target = if operator == ">&" { "1" } else { "out" };
            let source = format!("echo {prefix}2{operator}{target}\n");
            let tree = parser()
                .parse(&source, None)
                .expect("parser should return a tree");
            let root = tree.root_node();
            assert!(!root.has_error(), "{source}: {}", root.to_sexp());
            let command = root
                .named_child(0)
                .expect("fixture should have this named child");
            let argument = command
                .child_by_field_name("argument")
                .expect("fixture should have a argument field");
            assert_eq!(&source[argument.byte_range()], prefix);
            let redirect = command
                .child_by_field_name("redirect")
                .expect("fixture should have a redirect field");
            assert_eq!(
                &source[redirect.byte_range()],
                format!("2{operator}{target}")
            );
            assert_eq!(
                &source[redirect
                    .child_by_field_name("source")
                    .expect("fixture should have a source field")
                    .byte_range()],
                "2"
            );
        }
    }
    for argument in ["a2", "a,22", "\"a,2\"", "a,^2", "a,2x", "%V% =2"] {
        let source = format!("echo {argument}>out\n");
        let tree = parser()
            .parse(&source, None)
            .expect("parser should return a tree");
        assert!(!tree.root_node().has_error(), "{source}");
        if argument != "%V% =2" {
            assert!(
                node_sources(tree.root_node(), "file_descriptor", &source).is_empty(),
                "{source}"
            );
        } else {
            assert_eq!(
                node_sources(tree.root_node(), "argument", &source),
                ["%V%", "=", "out"]
            );
        }
    }
}

#[test]
fn delayed_references_do_not_change_ordinary_quote_state() {
    for (source, commands) in [
        ("echo !a\"b!x!c!&tail\"\n", vec!["echo"]),
        ("echo \"!a\"b!x!c!&echo visible\n", vec!["echo", "echo"]),
    ] {
        let tree = parser()
            .parse(source, None)
            .expect("parser should return a tree");
        let root = tree.root_node();
        assert!(!root.has_error(), "{source}: {}", root.to_sexp());
        assert_eq!(node_sources(root, "delayed_variable", source), ["!c!"]);
        assert_eq!(node_sources(root, "command_name", source), commands);
        let command = if root
            .named_child(0)
            .expect("fixture should have this named child")
            .kind()
            == "seq_list"
        {
            root.named_child(0)
                .expect("fixture should have this named child")
                .child_by_field_name("left")
                .expect("fixture should have a left field")
        } else {
            root.named_child(0)
                .expect("fixture should have this named child")
        };
        assert_eq!(
            command.named_child_count(),
            2,
            "one argument must retain all adjacent fragments"
        );
    }
    for (source, commands) in [
        ("echo !a\"b!&echo hidden\"\n", vec!["echo"]),
        ("echo !a\"b!\"&echo visible\n", vec!["echo", "echo"]),
        ("echo \"!a\"b!&echo visible\n", vec!["echo", "echo"]),
        ("call !a\"b!&echo hidden\"\n", vec![]),
        ("set x=!a\"b!&echo hidden\"\n", vec![]),
        ("(echo !a\"b!)&echo hidden\")\n", vec!["echo"]),
        ("echo !a\"b!>hidden\"\n", vec!["echo"]),
    ] {
        let tree = parser()
            .parse(source, None)
            .expect("parser should return a tree");
        let root = tree.root_node();
        assert!(!root.has_error(), "{source}: {}", root.to_sexp());
        assert_eq!(node_sources(root, "command_name", source), commands);
        assert!(node_sources(root, "delayed_variable", source).is_empty());
        assert!(node_sources(root, "redirect_file", source).is_empty());
    }
    let source = "echo !a\"b!&echo hidden\"\n";
    let tree = parser()
        .parse(source, None)
        .expect("parser should return a tree");
    assert_eq!(
        node_sources(tree.root_node(), "argument", source),
        ["!a\"b!&echo hidden\""]
    );
    assert_eq!(
        node_sources(tree.root_node(), "string", source),
        ["\"b!&echo hidden\""]
    );
}

#[test]
fn delayed_references_preserve_outer_command_boundaries() {
    for (source, kind, expected) in [
        ("echo !x & echo y!\n", "seq_list", "echo !x & echo y!"),
        ("echo !x | echo y!\n", "pipeline", "echo !x | echo y!"),
        ("echo !x >out!\n", "redirect_file", ">out!"),
        (
            "echo \"!x\" & echo \"y!\"\n",
            "seq_list",
            "echo \"!x\" & echo \"y!\"",
        ),
        ("(echo !x) & echo y!\n", "block", "(echo !x)"),
    ] {
        let tree = parser()
            .parse(source, None)
            .expect("parser should return a tree");
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
        let tree = parser()
            .parse(source, None)
            .expect("parser should return a tree");
        assert!(!tree.root_node().has_error());
        assert_eq!(
            node_sources(tree.root_node(), "delayed_variable", source),
            [expected]
        );
    }
    let tree = parser()
        .parse("cmd 2>&!A&B!\n", None)
        .expect("parser should return a tree");
    assert!(
        tree.root_node().has_error(),
        "a missing duplication target must stay invalid"
    );
}

#[test]
fn caret_set_delayed_fragments_preserve_operator_and_bang_boundaries() {
    for operator in ["&", "|", "<", ">"] {
        let source = format!("set ^\"x=!a\"b!{operator}echo hidden^\"\n");
        let tree = parser()
            .parse(&source, None)
            .expect("parser should return a tree");
        let root = tree.root_node();
        assert!(!root.has_error(), "{source}: {}", root.to_sexp());
        assert_eq!(root.named_child_count(), 1);
        assert_eq!(
            root.named_child(0)
                .expect("fixture should have this named child")
                .kind(),
            "set_statement"
        );
        assert!(node_sources(root, "delayed_variable", &source).is_empty());
        assert_eq!(
            node_sources(root, "text", &source),
            ["x=", &format!("!a\"b!{operator}echo hidden^\"")]
        );
    }

    for suffix in ["&echo hidden^\"", " !c!&echo hidden^\"", "&echo hidden"] {
        let source = format!("set ^\"x=!a\"b!{suffix}\n");
        let tree = parser()
            .parse(&source, None)
            .expect("parser should return a tree");
        let root = tree.root_node();
        assert!(!root.has_error(), "{source}: {}", root.to_sexp());
        assert_eq!(
            root.named_child(0)
                .expect("fixture should have this named child")
                .kind(),
            "set_statement"
        );
        assert!(node_sources(root, "delayed_variable", &source).is_empty());
        assert!(node_sources(root, "command_name", &source).is_empty());
        assert_eq!(
            node_sources(root, "text", &source),
            ["x=", &format!("!a\"b!{suffix}")]
        );
    }

    let source = "set ^\"x=!a\"b!\"&echo visible\n";
    let tree = parser()
        .parse(source, None)
        .expect("parser should return a tree");
    let root = tree.root_node();
    assert!(!root.has_error(), "{}", root.to_sexp());
    assert_eq!(node_sources(root, "seq_list", source), [source.trim_end()]);
    assert_eq!(node_sources(root, "command_name", source), ["echo"]);

    let source = "(set ^\"x=!a\"b!)&echo hidden^\")\n";
    let tree = parser()
        .parse(source, None)
        .expect("parser should return a tree");
    let root = tree.root_node();
    assert!(!root.has_error(), "{}", root.to_sexp());
    assert_eq!(node_sources(root, "block", source), [source.trim_end()]);
    assert!(node_sources(root, "seq_list", source).is_empty());

    let source = "set ^\"x=!a\"b\"c!&echo visible^\"\n";
    let tree = parser()
        .parse(source, None)
        .expect("parser should return a tree");
    let root = tree.root_node();
    assert!(!root.has_error(), "{}", root.to_sexp());
    assert_eq!(
        node_sources(root, "delayed_variable", source),
        ["!a\"b\"c!"]
    );
    assert_eq!(node_sources(root, "command_name", source), ["echo"]);
    assert_eq!(node_sources(root, "seq_list", source), [source.trim_end()]);
}

#[test]
fn delayed_quotes_in_redirect_filenames_preserve_source_boundaries() {
    for prefix in ["set ^\"x=foo", "echo hi "] {
        for (operator, target) in [
            (">", "!a\"b!&echo hidden^\""),
            (">>", "pre!a\"b! !c!&echo hidden^\""),
            ("<", "!a\"b!|echo hidden"),
            (">", "!a\"b!\"suffix"),
        ] {
            let source = format!("{prefix}{operator}{target}\n");
            let tree = parser()
                .parse(&source, None)
                .expect("parser should return a tree");
            let root = tree.root_node();
            assert!(!root.has_error(), "{source}: {}", root.to_sexp());
            assert_eq!(root.named_child_count(), 1);
            let statement = root
                .named_child(0)
                .expect("fixture should have this named child");
            let redirect = statement
                .child_by_field_name("redirect")
                .expect("fixture should have a redirect field");
            assert_eq!(
                &source[redirect.byte_range()],
                format!("{operator}{target}")
            );
            assert_eq!(
                &source[redirect
                    .child_by_field_name("operator")
                    .expect("fixture should have a operator field")
                    .byte_range()],
                operator
            );
            assert_eq!(
                &source[redirect
                    .child_by_field_name("target")
                    .expect("fixture should have a target field")
                    .byte_range()],
                target
            );
            assert!(node_sources(root, "delayed_variable", &source).is_empty());
            assert!(node_sources(root, "seq_list", &source).is_empty());
        }
    }

    let source = "set ^\"x=foo>!a\"b!\"&echo visible\n";
    let tree = parser()
        .parse(source, None)
        .expect("parser should return a tree");
    let root = tree.root_node();
    assert!(!root.has_error(), "{}", root.to_sexp());
    assert_eq!(node_sources(root, "redirect_file", source), [">!a\"b!\""]);
    assert_eq!(node_sources(root, "command", source), ["echo visible"]);
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
        let tree = parser()
            .parse(&source, None)
            .expect("parser should return a tree");
        let statement = tree
            .root_node()
            .named_child(0)
            .expect("fixture should have this named child");
        assert!(!statement.has_error(), "{source}: {}", statement.to_sexp());
        let comparison = statement
            .child_by_field_name("condition")
            .expect("fixture should have a condition field");
        let right = comparison
            .child_by_field_name("right")
            .expect("fixture should have a right field");
        let command = statement
            .child_by_field_name("consequence")
            .expect("fixture should have a consequence field");
        assert_eq!(&source[right.byte_range()], expected_right);
        assert_eq!(&source[command.byte_range()], expected_command);
    }
    for ending in ["", "\n", "\r\n"] {
        let source = format!("if a==b=c{ending}");
        let tree = parser()
            .parse(&source, None)
            .expect("parser should return a tree");
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
        let tree = parser()
            .parse(source, None)
            .expect("parser should return a tree");
        let root = tree.root_node();
        assert!(!root.has_error(), "{source}: {}", root.to_sexp());
        let mut cursor = root;
        while cursor.kind() != "set_quoted" && cursor.kind() != "set_prompt" {
            cursor = if cursor.kind() == "set_statement" {
                cursor
                    .named_child(1)
                    .expect("fixture should have this named child")
            } else {
                cursor
                    .named_child(0)
                    .expect("fixture should have this named child")
            };
        }
        assert_eq!(
            &source[cursor
                .child_by_field_name(field)
                .expect("fixture should have the requested field")
                .byte_range()],
            value
        );
        for kind in ["command_name", "seq_list", "pipeline", "redirect_file"] {
            assert!(
                node_sources(root, kind, source).is_empty(),
                "{source}: {kind}"
            );
        }
    }
    let source = "set \"x=a\"b&echo \"after\"\n";
    let tree = parser()
        .parse(source, None)
        .expect("parser should return a tree");
    assert!(!tree.root_node().has_error());
    assert_eq!(
        node_sources(tree.root_node(), "command_name", source),
        ["echo"]
    );
}

#[test]
fn caret_quoted_set_preserves_outer_syntax_and_source_fragments() {
    for (source, kind, expected) in [
        ("set ^\"x=foo&echo second^\"\n", "command", "echo second^\""),
        (
            "set ^\"x=foo&&echo second^\"\n",
            "and_list",
            "set ^\"x=foo&&echo second^\"",
        ),
        (
            "set ^\"x=foo||echo second^\"\n",
            "or_list",
            "set ^\"x=foo||echo second^\"",
        ),
        (
            "set ^\"x=foo|echo second^\"\n",
            "pipeline",
            "set ^\"x=foo|echo second^\"",
        ),
        ("set ^\"x=foo>out tail^\"\n", "redirect_file", ">out"),
        ("set ^\"x=foo>out\n", "redirect_file", ">out"),
        (
            "(set ^\"x=foo) & echo second^\"\n",
            "block",
            "(set ^\"x=foo)",
        ),
    ] {
        let tree = parser()
            .parse(source, None)
            .expect("parser should return a tree");
        let root = tree.root_node();
        assert!(!root.has_error(), "{source}: {}", root.to_sexp());
        assert_eq!(node_sources(root, kind, source), [expected]);
    }

    let source = "set ^\"x=%PATH%!SUFFIX!^&done^\"\n";
    let tree = parser()
        .parse(source, None)
        .expect("parser should return a tree");
    let root = tree.root_node();
    assert!(!root.has_error(), "{}", root.to_sexp());
    assert_eq!(node_sources(root, "variable", source), ["%PATH%"]);
    assert_eq!(node_sources(root, "delayed_variable", source), ["!SUFFIX!"]);
    assert_eq!(
        node_sources(root, "escape_sequence", source),
        ["^\"", "^&", "^\""]
    );
    assert!(node_sources(root, "seq_list", source).is_empty());
    assert!(node_sources(root, "string", source).is_empty());
}

#[test]
fn quoted_set_names_preserve_metacharacters_and_expansions() {
    for name in [
        "x(1)", "x&y", "x|y", "x<y", "x>y", "x^y", "x%N%(1)", "x!N!&y ",
    ] {
        for payload in [
            format!("\"{name}=value\""),
            format!("/p \"{name}=prompt\""),
            format!("\"{name}\""),
        ] {
            for source in [format!("set {payload}\n"), format!("(set {payload})\n")] {
                let tree = parser()
                    .parse(&source, None)
                    .expect("parser should return a tree");
                let root = tree.root_node();
                assert!(!root.has_error(), "{source}: {}", root.to_sexp());
                assert_eq!(node_sources(root, "variable_name", &source), [name]);
                for kind in ["seq_list", "pipeline", "redirect_file", "escape_sequence"] {
                    assert!(
                        node_sources(root, kind, &source).is_empty(),
                        "{source}: {kind}"
                    );
                }
                if name.contains("%N%") {
                    assert_eq!(node_sources(root, "variable", &source), ["%N%"]);
                }
                if name.contains("!N!") {
                    assert_eq!(node_sources(root, "delayed_variable", &source), ["!N!"]);
                }
            }
        }
    }
    let source = "set x&echo after\nset x>out=y\nset x^&y=value\n";
    let tree = parser()
        .parse(source, None)
        .expect("parser should return a tree");
    let root = tree.root_node();
    assert!(!root.has_error(), "{}", root.to_sexp());
    assert_eq!(node_sources(root, "command_name", source), ["echo"]);
    assert_eq!(node_sources(root, "redirect_file", source), [">out"]);
    assert_eq!(node_sources(root, "escape_sequence", source), ["^&"]);
}

#[test]
fn quoted_set_names_stop_when_delayed_text_closes_the_outer_quote() {
    for (operator, kind) in [("&", "seq_list"), ("|", "pipeline")] {
        let source = format!("set \"x!a\"!{operator}echo y=z\"\n");
        let tree = parser()
            .parse(&source, None)
            .expect("parser should return a tree");
        let root = tree.root_node();
        assert!(!root.has_error(), "{source}: {}", root.to_sexp());
        assert_eq!(
            root.named_child(0)
                .expect("fixture should have this named child")
                .kind(),
            kind
        );
        assert_eq!(node_sources(root, "variable_name", &source), ["x!a\"!"]);
        assert_eq!(node_sources(root, "delayed_variable", &source), ["!a\"!"]);
        assert_eq!(node_sources(root, "command", &source), ["echo y=z\""]);
    }
    for (source, name, redirect) in [
        ("set \"x!a\"!>out&echo y=z\"\n", "x!a\"!", vec![">out"]),
        ("set \"x!a\"!<in&echo y=z\"\n", "x!a\"!", vec!["<in"]),
        ("(set \"x!a\"!)&echo y=z\"\n", "x!a\"!", vec![]),
        ("set \"x!a\"!)&echo y=z\"\n", "x!a\"!)", vec![]),
        ("set \"x!a\"!^&tail&echo y=z\"\n", "x!a\"!^&tail", vec![]),
        ("set \"x!a\"! !b!&echo y=z\"\n", "x!a\"! !b!", vec![]),
    ] {
        let tree = parser()
            .parse(source, None)
            .expect("parser should return a tree");
        let root = tree.root_node();
        assert!(!root.has_error(), "{source}: {}", root.to_sexp());
        assert_eq!(node_sources(root, "variable_name", source), [name]);
        assert_eq!(node_sources(root, "redirect_file", source), redirect);
        assert_eq!(node_sources(root, "command", source), ["echo y=z\""]);
    }
    for source in ["set \"x!a\"!=v\"\n", "set /p \"x!a\"!=v\"\n"] {
        let tree = parser()
            .parse(source, None)
            .expect("parser should return a tree");
        let root = tree.root_node();
        assert!(!root.has_error(), "{source}: {}", root.to_sexp());
        assert_eq!(node_sources(root, "variable_name", source), ["x!a\"!"]);
        assert_eq!(node_sources(root, "argument", source), ["v"]);
    }
    let source = "set /p \"x!a\"!&echo after\n";
    let tree = parser()
        .parse(source, None)
        .expect("parser should return a tree");
    let root = tree.root_node();
    assert!(root.has_error(), "SET /P still requires an equals sign");
    assert_eq!(node_sources(root, "command", source), ["echo after"]);
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
        let tree = parser()
            .parse(source, None)
            .expect("parser should return a tree");
        assert!(
            !tree.root_node().has_error(),
            "{source}: {}",
            tree.root_node().to_sexp()
        );
        assert_eq!(
            node_sources(tree.root_node(), "redirect_file", source),
            [redirect]
        );
    }
    let source = "echo >\"out\"2>err\n";
    let tree = parser()
        .parse(source, None)
        .expect("parser should return a tree");
    assert!(!tree.root_node().has_error());
    assert_eq!(
        node_sources(tree.root_node(), "redirect_file", source),
        [">\"out\"", "2>err"]
    );
}

#[test]
fn continued_set_suffix_includes_the_forced_literal() {
    for newline in ["\n", "\r\n"] {
        let suffix = format!("junk^{newline}{newline}echo after");
        let source = format!("set \"x=y\"{suffix}\n");
        let tree = parser()
            .parse(&source, None)
            .expect("parser should return a tree");
        assert!(!tree.root_node().has_error());
        assert_eq!(
            node_sources(tree.root_node(), "set_ignored_suffix", &source),
            [suffix.as_str()]
        );
        assert!(node_sources(tree.root_node(), "command_name", &source).is_empty());
        for marker in ["&", "|", ">", "<"] {
            let suffix = format!("junk^{newline}{marker}echo after");
            let source = format!("set \"x=y\"{suffix}\n");
            let tree = parser()
                .parse(&source, None)
                .expect("parser should return a tree");
            assert!(!tree.root_node().has_error());
            assert_eq!(
                node_sources(tree.root_node(), "set_ignored_suffix", &source),
                [suffix.as_str()]
            );
            assert!(node_sources(tree.root_node(), "command_name", &source).is_empty());
            assert!(node_sources(tree.root_node(), "redirect_file", &source).is_empty());
        }
        let source = format!("(set \"x=y\"junk^{newline}))\n");
        let tree = parser()
            .parse(&source, None)
            .expect("parser should return a tree");
        assert!(!tree.root_node().has_error());
        assert_eq!(
            node_sources(tree.root_node(), "set_ignored_suffix", &source),
            [format!("junk^{newline})")]
        );
    }
    let source = "set \"x=y\"junk^\n &echo after\n";
    let tree = parser()
        .parse(source, None)
        .expect("parser should return a tree");
    assert!(!tree.root_node().has_error());
    assert_eq!(
        node_sources(tree.root_node(), "command_name", source),
        ["echo"]
    );
}

#[test]
fn last_set_quote_can_leave_the_ignored_suffix_quoted() {
    for suffix in ["c&d", "c|d", "c>d", "c)d", "c^&d"] {
        let source = format!("set \"x=a\"b\"{suffix}\n");
        let tree = parser()
            .parse(&source, None)
            .expect("parser should return a tree");
        let root = tree.root_node();
        assert!(!root.has_error(), "{source}: {}", root.to_sexp());
        assert_eq!(node_sources(root, "set_ignored_suffix", &source), [suffix]);
        assert!(node_sources(root, "command_name", &source).is_empty());
        assert!(node_sources(root, "redirect_file", &source).is_empty());
    }
}

#[test]
fn else_remains_in_command_tails() {
    for (tail, kind, text) in [
        ("echo someone else here", "argument", "else"),
        ("echo else", "argument", "else"),
        ("echo elsewhere", "text", "elsewhere"),
        ("echo someone ELSE here", "argument", "ELSE"),
        ("set x=else", "argument", "else"),
        ("set /p x=else", "argument", "else"),
        ("set /a else", "argument", "else"),
        ("goto someone else", "label_name", "someone else"),
        ("call :helper else", "argument", "else"),
        ("echo first >nul else", "argument", "else"),
        ("echo first & echo someone else here", "argument", "else"),
    ] {
        let source = format!("if x==x {tail}\n");
        let tree = parser()
            .parse(&source, None)
            .expect("parser should return a tree");
        let root = tree.root_node();
        assert!(!root.has_error(), "{source}: {}", root.to_sexp());
        let statement = root
            .named_child(0)
            .expect("fixture should have this named child");
        let consequence = statement
            .child_by_field_name("consequence")
            .expect("fixture should have a consequence field");
        assert_eq!(&source[consequence.byte_range()], tail);
        assert!(statement.child_by_field_name("alternative").is_none());
        assert!(
            node_sources(consequence, kind, &source).contains(&text),
            "{source}"
        );
    }
}

#[test]
fn else_branches_follow_completed_blocks() {
    for consequence in [
        "(echo yes)",
        "(echo yes)>nul",
        "echo first |(echo last)",
        "echo first & (echo last)",
        "if y==y (echo inner) else (echo other)",
    ] {
        let source = format!("if x==x {consequence} else (echo no)\n");
        let tree = parser()
            .parse(&source, None)
            .expect("parser should return a tree");
        let root = tree.root_node();
        assert!(!root.has_error(), "{source}: {}", root.to_sexp());
        let statement = root
            .named_child(0)
            .expect("fixture should have this named child");
        let body = statement
            .child_by_field_name("consequence")
            .expect("fixture should have a consequence field");
        let alternative = statement
            .child_by_field_name("alternative")
            .expect("fixture should have a alternative field");
        assert_eq!(&source[body.byte_range()], consequence);
        assert_eq!(&source[alternative.byte_range()], "(echo no)");
    }
}

#[test]
fn else_requires_a_complete_token() {
    for suffix in [
        "x echo no",
        "where",
        "\"x\" echo no",
        "(echo no)",
        "%X% echo no",
        "^ echo no",
        "",
    ] {
        let source = format!("if 1==1 (echo yes) else{suffix}\n");
        let tree = parser()
            .parse(&source, None)
            .expect("parser should return a tree");
        assert!(tree.root_node().has_error(), "{source}");
    }
    for tail in [
        " echo no",
        "\techo no",
        ",echo no",
        ";echo no",
        "=echo no",
        ">out echo no",
    ] {
        let source = format!("if 1==0 (echo yes) else{tail}\n");
        let tree = parser()
            .parse(&source, None)
            .expect("parser should return a tree");
        assert!(
            !tree.root_node().has_error(),
            "{source}: {}",
            tree.root_node().to_sexp()
        );
        let statement = tree
            .root_node()
            .named_child(0)
            .expect("fixture should have this named child");
        assert!(statement.child_by_field_name("alternative").is_some());
    }
    let source = "if 1==1 (echo yes) elsex echo no\necho tail\n";
    let tree = parser()
        .parse(source, None)
        .expect("parser should return a tree");
    assert!(tree.root_node().has_error());
    assert_eq!(
        node_sources(tree.root_node(), "command_name", source),
        ["echo", "echo"]
    );
}

#[test]
fn if_control_words_do_not_split_compound_operands() {
    for prefix in [
        "/i",
        "not",
        "exist",
        "defined",
        "errorlevel",
        "cmdextversion",
    ] {
        for (suffix, child_kind) in [
            ("\"x\"", "string"),
            ("%N%", "variable"),
            ("!N!", "delayed_variable"),
            ("^x", "escape_sequence"),
        ] {
            let operand = format!("{prefix}{suffix}");
            let source = format!("if {operand}==right echo matched\n");
            let tree = parser()
                .parse(&source, None)
                .expect("parser should return a tree");
            let root = tree.root_node();
            assert!(!root.has_error(), "{source}: {}", root.to_sexp());
            let statement = root
                .named_child(0)
                .expect("fixture should have this named child");
            let condition = statement
                .child_by_field_name("condition")
                .expect("fixture should have a condition field");
            assert_eq!(condition.kind(), "comparison");
            let left = condition
                .child_by_field_name("left")
                .expect("fixture should have a left field");
            assert_eq!(&source[left.byte_range()], operand);
            assert_eq!(node_sources(left, "text", &source), [prefix]);
            assert_eq!(node_sources(left, child_kind, &source), [suffix]);
            assert_eq!(
                &source[condition
                    .child_by_field_name("right")
                    .expect("fixture should have a right field")
                    .byte_range()],
                "right"
            );
            for kind in ["if_flag", "not", "condition_keyword"] {
                assert!(
                    node_sources(root, kind, &source).is_empty(),
                    "{source}: {kind}"
                );
            }
        }
    }
    for separator in [" ", "\t", ",", ";", "="] {
        let source = format!("if /I{separator}NOT{separator}DEFINED{separator}name echo matched\n");
        let tree = parser()
            .parse(&source, None)
            .expect("parser should return a tree");
        let root = tree.root_node();
        assert!(!root.has_error(), "{source}: {}", root.to_sexp());
        assert_eq!(node_sources(root, "if_flag", &source), ["/I"]);
        assert_eq!(node_sources(root, "not", &source), ["NOT"]);
        assert_eq!(
            node_sources(root, "condition_keyword", &source),
            ["DEFINED"]
        );
    }
}

#[test]
fn for_binder_requires_a_source_separator_before_in() {
    for separator in [" ", "\t", ",", ";", "=", " ,;= "] {
        let source = format!("for %%a{separator}in (x) do echo %%a\n");
        let tree = parser()
            .parse(&source, None)
            .expect("parser should return a tree");
        assert!(!tree.root_node().has_error(), "{source}");
        assert_eq!(
            node_sources(tree.root_node(), "loop_variable_declaration", &source),
            ["%%a"]
        );
    }
    for spelling in ["%%ain", "%%aIN", "%%a^ in", "%%a\"in\""] {
        let source = format!("for {spelling} (x) do echo %%a\n");
        let tree = parser()
            .parse(&source, None)
            .expect("parser should return a tree");
        assert!(tree.root_node().has_error(), "{source}");
    }
}

#[test]
fn goto_target_segments_do_not_include_removed_redirections() {
    for (source, names, redirects) in [
        ("goto foo>nul bar\n", vec!["foo", "bar"], vec![">nul"]),
        ("goto foo 2>nul bar\n", vec!["foo", "bar"], vec!["2>nul"]),
        (
            "goto foo>nul bar>err baz\n",
            vec!["foo", "bar", "baz"],
            vec![">nul", ">err"],
        ),
        ("goto foo;ignored>nul more\n", vec!["foo"], vec![">nul"]),
        ("goto foo;ignored 2>nul more\n", vec!["foo"], vec!["2>nul"]),
        (
            "goto foo;ignored>nul more 2>err end\n",
            vec!["foo"],
            vec![">nul", "2>err"],
        ),
        (
            "goto foo>nul ;ignored>err more\n",
            vec!["foo"],
            vec![">nul", ">err"],
        ),
        ("goto ::skip>nul later\n", vec![], vec![">nul"]),
    ] {
        let tree = parser()
            .parse(source, None)
            .expect("parser should return a tree");
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
        let tree = parser()
            .parse(&source, None)
            .expect("parser should return a tree");
        let root = tree.root_node();
        assert!(!root.has_error());
        assert_eq!(node_sources(root, "label_text", &source), [text]);
        assert_eq!(node_sources(root, "redirect_file", &source), [redirect]);
    }
    let source = "goto foo;ignored>nul more\n";
    let tree = parser()
        .parse(source, None)
        .expect("parser should return a tree");
    assert_eq!(
        node_sources(tree.root_node(), "label_text", source),
        [";ignored", "more"]
    );
    let source = "goto foo>nul\n";
    let tree = parser()
        .parse(source, None)
        .expect("parser should return a tree");
    let statement = tree
        .root_node()
        .named_child(0)
        .expect("fixture should have this named child");
    assert!(!statement.has_error());
    assert!(statement.child_by_field_name("redirect").is_some());
    assert_eq!(node_sources(statement, "label_reference", source), ["foo"]);
    for suffix in ["^&echo after", "\"a&b\"", "^\n&echo after"] {
        let source = format!("goto foo;ignored>nul {suffix}\n");
        let tree = parser()
            .parse(&source, None)
            .expect("parser should return a tree");
        let root = tree.root_node();
        assert!(!root.has_error());
        assert_eq!(
            node_sources(root, "label_text", &source),
            [";ignored", suffix]
        );
        assert!(node_sources(root, "command_name", &source).is_empty());
    }
}

#[test]
fn goto_quotes_preserve_outer_boundaries_and_source_spelling() {
    for (source, names, redirects, blocks) in [
        (
            "goto \"foo&echo bad\"\n",
            vec!["\"foo&echo"],
            vec![],
            vec![],
        ),
        (
            "goto \"foo|bar<in>out\"\n",
            vec!["\"foo|bar<in>out\""],
            vec![],
            vec![],
        ),
        (
            "(goto \"foo)bar\")\n",
            vec!["\"foo)bar\""],
            vec![],
            vec!["(goto \"foo)bar\")"],
        ),
        (
            "goto pre\"foo>bar\"post>nul tail\n",
            vec!["pre\"foo>bar\"post", "tail"],
            vec![">nul"],
            vec![],
        ),
        (
            "goto foo>nul \"bar&baz\"\n",
            vec!["foo", "\"bar&baz\""],
            vec![">nul"],
            vec![],
        ),
        (
            "goto \"foo&bar\";ignored>nul more\n",
            vec!["\"foo&bar\""],
            vec![">nul"],
            vec![],
        ),
        ("goto \"foo&bar\n", vec!["\"foo&bar"], vec![], vec![]),
    ] {
        let tree = parser()
            .parse(source, None)
            .expect("parser should return a tree");
        let root = tree.root_node();
        assert!(!root.has_error(), "{source}: {}", root.to_sexp());
        assert_eq!(node_sources(root, "label_name", source), names);
        assert_eq!(node_sources(root, "redirect_file", source), redirects);
        assert_eq!(node_sources(root, "block", source), blocks);
        assert_eq!(node_sources(root, "goto_statement", source).len(), 1);
        assert!(node_sources(root, "command", source).is_empty());
        assert!(node_sources(root, "pipeline", source).is_empty());
    }

    let source = "goto \"%target%&!suffix!\"\n";
    let tree = parser()
        .parse(source, None)
        .expect("parser should return a tree");
    let root = tree.root_node();
    assert!(!root.has_error());
    assert_eq!(
        node_sources(root, "string", source),
        ["\"%target%&!suffix!\""]
    );
    assert_eq!(node_sources(root, "variable", source), ["%target%"]);
    assert_eq!(node_sources(root, "delayed_variable", source), ["!suffix!"]);
}

#[test]
fn escaped_text_does_not_start_a_redirection_descriptor() {
    for (tail, argument, descriptor, operator, target) in [
        ("a^b2>out", "a^b2", None, ">", "out"),
        ("a^\nb2>out", "a^\nb2", None, ">", "out"),
        ("a^\r\nb2>out", "a^\r\nb2", None, ">", "out"),
        ("a^b22>out", "a^b22", None, ">", "out"),
        ("a^b2x>out", "a^b2x", None, ">", "out"),
        ("a^b2<input", "a^b2", None, "<", "input"),
        ("a^b2>&1", "a^b2", None, ">&", "1"),
        ("a^b 2>out", "a^b", Some("2"), ">", "out"),
        ("\"ab\"2>out", "\"ab\"", Some("2"), ">", "out"),
        ("a^ 2>out", "a^ ", Some("2"), ">", "out"),
        ("a^&2>out", "a^&", Some("2"), ">", "out"),
    ] {
        let source = format!("echo {tail}\necho tail\n");
        let tree = parser()
            .parse(&source, None)
            .expect("parser should return a tree");
        let root = tree.root_node();
        assert!(!root.has_error(), "{source:?}: {}", root.to_sexp());
        assert_eq!(root.named_child_count(), 2);
        let command = root
            .named_child(0)
            .expect("fixture should have this named child");
        let argument_node = command
            .child_by_field_name("argument")
            .expect("fixture should have a argument field");
        assert_eq!(&source[argument_node.byte_range()], argument, "{source:?}");
        let redirect = command
            .child_by_field_name("redirect")
            .expect("fixture should have a redirect field");
        assert_eq!(
            redirect
                .child_by_field_name("source")
                .map(|node| &source[node.byte_range()]),
            descriptor,
            "{source:?}",
        );
        for (field, expected) in [("operator", operator), ("target", target)] {
            let node = redirect
                .child_by_field_name(field)
                .expect("fixture should have the requested field");
            assert_eq!(&source[node.byte_range()], expected, "{source:?}");
        }
    }

    for (source, kind, expected) in [
        ("call a^b2>out\n", "argument", vec!["a^b2", "out"]),
        ("set x=a^b2>out\n", "argument", vec!["a^b2", "out"]),
        ("set a^b2>out=value\n", "variable_name", vec!["a^b2"]),
        ("goto a^b2>out\n", "label_name", vec!["a^b2"]),
        ("echo >a^b2>out\n", "argument", vec!["a^b2", "out"]),
    ] {
        let tree = parser()
            .parse(source, None)
            .expect("parser should return a tree");
        let root = tree.root_node();
        assert!(!root.has_error(), "{source:?}: {}", root.to_sexp());
        assert!(node_sources(root, "file_descriptor", source).is_empty());
        assert_eq!(node_sources(root, kind, source), expected, "{source:?}");
    }
}

#[test]
fn escaped_if_operands_do_not_capture_spaced_descriptors() {
    for (prefix, field) in [("if exist ", "argument"), ("if x==", "right")] {
        for (operand, expected, descriptor) in [
            ("a^b2", "a^b2", None),
            ("a^b 2", "a^b", Some("2")),
            ("a^b\t2", "a^b", Some("2")),
            ("a^\r\nb2", "a^\r\nb2", None),
            ("a^\r\nb 2", "a^\r\nb", Some("2")),
        ] {
            let source = format!("{prefix}{operand}>nul echo yes\n");
            let tree = parser()
                .parse(&source, None)
                .expect("parser should return a tree");
            let root = tree.root_node();
            assert!(!root.has_error(), "{source:?}: {}", root.to_sexp());
            let statement = root
                .named_child(0)
                .expect("fixture should have this named child");
            let condition = statement
                .child_by_field_name("condition")
                .expect("fixture should have a condition field");
            let argument = condition
                .child_by_field_name(field)
                .expect("fixture should have the requested field");
            assert_eq!(&source[argument.byte_range()], expected, "{source:?}");
            let consequence = statement
                .child_by_field_name("consequence")
                .expect("fixture should have a consequence field");
            let redirect = consequence
                .child_by_field_name("redirect")
                .expect("fixture should have a redirect field");
            assert_eq!(
                redirect
                    .child_by_field_name("source")
                    .map(|node| &source[node.byte_range()]),
                descriptor,
                "{source:?}",
            );
            let expected_body = if descriptor.is_some() {
                "2>nul echo yes"
            } else {
                ">nul echo yes"
            };
            assert_eq!(
                &source[consequence.byte_range()],
                expected_body,
                "{source:?}"
            );
        }
    }
}

#[test]
fn quoted_set_values_keep_segments_around_redirections() {
    for prefix in ["set ", "set /p "] {
        let field = if prefix == "set " { "value" } else { "prompt" };
        for (tail, values, redirects) in [
            ("a\"b>out c\"", vec!["a\"b", " c"], vec![">out"]),
            ("a\"b>\"out q\" c\"", vec!["a\"b", " c"], vec![">\"out q\""]),
            ("a\"b>&2 c\"", vec!["a\"b", " c"], vec![">&2"]),
            (
                "a\"b>out c\"d\"e>two f\"",
                vec!["a\"b", " c\"d\"e", " f"],
                vec![">out", ">two"],
            ),
            ("a\"2>out c\"", vec!["a\"", " c"], vec!["2>out"]),
            ("a\"b2>out c\"", vec!["a\"b2", " c"], vec![">out"]),
            ("a\"b>%v:\"=% c\"", vec!["a\"b", " c"], vec![">%v:\"=%"]),
            (
                "a\"b>\"%v:\"=%\" c\"",
                vec!["a\"b", " c"],
                vec![">\"%v:\"=%\""],
            ),
            ("a\"b>%v: =% c\"", vec!["a\"b", " c"], vec![">%v: =%"]),
            ("a\"b>%v:&=% c\"", vec!["a\"b", " c"], vec![">%v:&=%"]),
            ("a\"b>out% c\"", vec!["a\"b", " c"], vec![">out%"]),
        ] {
            let source = format!("{prefix}\"x={tail}\n");
            let tree = parser()
                .parse(&source, None)
                .expect("parser should return a tree");
            let root = tree.root_node();
            assert!(!root.has_error(), "{source}: {}", root.to_sexp());
            let binding = root
                .named_child(0)
                .expect("fixture should have this named child")
                .named_child(1)
                .expect("fixture should have this named child");
            let mut cursor = binding.walk();
            let actual_values: Vec<_> = binding
                .children_by_field_name(field, &mut cursor)
                .map(|node| &source[node.byte_range()])
                .collect();
            assert_eq!(actual_values, values, "{source}");
            let actual_redirects: Vec<_> = binding
                .children_by_field_name("redirect", &mut cursor)
                .map(|node| &source[node.byte_range()])
                .collect();
            assert_eq!(actual_redirects, redirects, "{source}");
            assert!(node_sources(root, "set_ignored_suffix", &source).is_empty());
        }
    }
    for tail in [
        "b>\"out q\"",
        "b>out&echo \"after\"",
        "b>out|echo \"after\"",
        "b>%v:\"=%",
        "b>out%\" c\"",
    ] {
        let source = format!("set \"x=a\"{tail}\n");
        let tree = parser()
            .parse(&source, None)
            .expect("parser should return a tree");
        let root = tree.root_node();
        assert!(!root.has_error(), "{source}: {}", root.to_sexp());
        assert_eq!(node_sources(root, "set_ignored_suffix", &source), ["b"]);
    }
}

#[test]
fn leading_command_escapes_keep_their_source_ranges() {
    for (source, name, escape) in [
        ("^echo hi\n", "^echo", "^e"),
        ("^a2>out\n", "^a2", "^a"),
        ("^a 2>out\n", "^a", "^a"),
        ("^\necho hi\n", "^\necho", "^\ne"),
        ("^\r\necho hi\r\n", "^\r\necho", "^\r\ne"),
        ("@^echo hi\n", "^echo", "^e"),
        ("^@echo hi\n", "^@echo", "^@"),
        ("^:label hi\n", "^:label", "^:"),
        ("^%CMD% hi\n", "^%CMD%", "^"),
        ("^!CMD! hi\n", "^!CMD!", "^"),
        ("for %%a in (x) do ^echo hi\n", "^echo", "^e"),
    ] {
        let tree = parser()
            .parse(source, None)
            .expect("parser should return a tree");
        let root = tree.root_node();
        assert!(!root.has_error(), "{source}: {}", root.to_sexp());
        assert_eq!(node_sources(root, "command_name", source), [name]);
        assert_eq!(node_sources(root, "escape_sequence", source), [escape]);
    }

    let source = "^if a==b echo hi\n";
    let tree = parser()
        .parse(source, None)
        .expect("parser should return a tree");
    let root = tree.root_node();
    assert!(!root.has_error());
    assert_eq!(node_sources(root, "command_name", source), ["^if"]);
    assert!(node_sources(root, "if_statement", source).is_empty());
    assert!(node_sources(root, "keyword", source).is_empty());
}

#[test]
fn if_body_starts_do_not_join_the_condition_across_whitespace() {
    for (source, condition, body) in [
        ("if exist x rem note\n", "exist x", "rem note"),
        ("if exist x\trem note\n", "exist x", "rem note"),
        ("if exist x rem-tool arg\n", "exist x", "rem-tool arg"),
        ("if exist x 123 arg\n", "exist x", "123 arg"),
        ("if exist x !CMD! arg\n", "exist x", "!CMD! arg"),
        ("if 1==1 rem note\n", "1==1", "rem note"),
        ("if 1==1 2>nul echo hi\n", "1==1", "2>nul echo hi"),
        (
            "if exist pre!SUFFIX! rem note\n",
            "exist pre!SUFFIX!",
            "rem note",
        ),
        ("if exist pre^x rem note\n", "exist pre^x", "rem note"),
    ] {
        let tree = parser()
            .parse(source, None)
            .expect("parser should return a tree");
        let root = tree.root_node();
        assert!(!root.has_error(), "{source}: {}", root.to_sexp());
        assert_eq!(root.named_child_count(), 1);
        let statement = root
            .named_child(0)
            .expect("fixture should have this named child");
        let condition_node = statement
            .child_by_field_name("condition")
            .expect("fixture should have a condition field");
        let body_node = statement
            .child_by_field_name("consequence")
            .expect("fixture should have a consequence field");
        assert_eq!(&source[condition_node.byte_range()], condition);
        assert_eq!(&source[body_node.byte_range()], body);
    }
}

#[test]
fn quoted_goto_names_end_at_lookup_delimiters() {
    for delimiter in [";", ",", "=", "+", ":", " ", "\t"] {
        for prefix in ["\"foo", "pre\"foo", "\"pre\"more\"foo"] {
            let tail = format!("{delimiter}ignored&echo bad\"");
            let source = format!("goto {prefix}{tail}>nul later\n");
            let tree = parser()
                .parse(&source, None)
                .expect("parser should return a tree");
            let root = tree.root_node();
            assert!(!root.has_error(), "{source}: {}", root.to_sexp());
            assert_eq!(node_sources(root, "label_name", &source), [prefix]);
            assert_eq!(
                node_sources(root, "label_text", &source),
                [tail.as_str(), "later"]
            );
            assert_eq!(node_sources(root, "redirect_file", &source), [">nul"]);
            assert!(node_sources(root, "command", &source).is_empty());
        }
    }
    let source = "goto \"%target%;!suffix!&echo bad\"\n";
    let tree = parser()
        .parse(source, None)
        .expect("parser should return a tree");
    let root = tree.root_node();
    assert!(!root.has_error(), "{}", root.to_sexp());
    assert_eq!(node_sources(root, "label_name", source), ["\"%target%"]);
    assert_eq!(
        node_sources(root, "label_text", source),
        [";!suffix!&echo bad\""]
    );
    assert_eq!(node_sources(root, "variable", source), ["%target%"]);
    assert_eq!(node_sources(root, "delayed_variable", source), ["!suffix!"]);
}

#[test]
fn set_values_resume_after_expanded_duplication_targets() {
    for expansion in [
        "%fd%",
        "%fd:\"=%",
        "%1",
        "%~1",
        "%~dp1",
        "%~$PATH:1",
        "%~$1PATH:1",
        "%~$-PATH:1",
        "%~$1\"PATH:1",
        "%~$\"PATH:1",
        "%~$P\"Q:1",
        "%*",
    ] {
        for (prefix, field) in [("set ", "value"), ("set /p ", "prompt")] {
            for (first, source_fd) in [("a\"b", ""), ("a\"", "2")] {
                let redirect_text = format!("{source_fd}>&{expansion}");
                let source = format!("{prefix}\"x={first}{redirect_text}c\"\n");
                let tree = parser()
                    .parse(&source, None)
                    .expect("parser should return a tree");
                let root = tree.root_node();
                assert!(!root.has_error(), "{source}: {}", root.to_sexp());
                let binding = root
                    .named_child(0)
                    .expect("fixture should have this named child")
                    .named_child(1)
                    .expect("fixture should have this named child");
                let mut cursor = binding.walk();
                let values: Vec<_> = binding
                    .children_by_field_name(field, &mut cursor)
                    .map(|node| &source[node.byte_range()])
                    .collect();
                assert_eq!(values, [first, "c"], "{source}");
                let redirect = binding
                    .child_by_field_name("redirect")
                    .expect("fixture should have a redirect field");
                assert_eq!(&source[redirect.byte_range()], redirect_text);
                let target = redirect
                    .child_by_field_name("target")
                    .expect("fixture should have a target field");
                assert_eq!(&source[target.byte_range()], expansion);
                assert_eq!(
                    target.kind(),
                    if expansion.starts_with("%~") {
                        "parameter_tilde"
                    } else if expansion == "%1" {
                        "parameter"
                    } else if expansion == "%*" {
                        "all_arguments"
                    } else {
                        "variable"
                    }
                );
                assert_eq!(
                    redirect
                        .child_by_field_name("source")
                        .map(|node| &source[node.byte_range()]),
                    if source_fd.is_empty() {
                        None
                    } else {
                        Some(source_fd)
                    },
                );
                assert!(node_sources(root, "set_ignored_suffix", &source).is_empty());
            }
        }
    }
}

#[test]
fn quoted_set_name_quotes_stay_in_the_name_range() {
    for (prefix, binding_kind) in [("set ", "set_quoted"), ("set /p ", "set_prompt")] {
        let source = format!("{prefix}\"a\"b\"=x\"\n");
        let tree = parser()
            .parse(&source, None)
            .expect("parser should return a tree");
        assert!(!tree.root_node().has_error(), "{source}");
        let binding = tree
            .root_node()
            .named_child(0)
            .expect("fixture should have this named child")
            .named_child(1)
            .expect("fixture should have this named child");
        assert_eq!(binding.kind(), binding_kind);
        let name = binding
            .child_by_field_name("name")
            .expect("fixture should have a name field");
        assert_eq!(&source[name.byte_range()], "a\"b\"");
        let value = binding
            .child_by_field_name(if binding_kind == "set_prompt" {
                "prompt"
            } else {
                "value"
            })
            .expect("fixture should have the requested field");
        assert_eq!(&source[value.byte_range()], "x");
    }
}

#[test]
fn quoted_set_names_keep_segments_around_redirections() {
    for (prefix, kind, field) in [
        ("set ", "set_quoted", "value"),
        ("set /p ", "set_prompt", "prompt"),
    ] {
        for (name, names, redirects) in [
            ("a\"b>nul c\"", vec!["a\"b", " c\""], vec![">nul"]),
            (
                "a\"b>\"out q\" c\"",
                vec!["a\"b", " c\""],
                vec![">\"out q\""],
            ),
            ("a\"b>&2c\"", vec!["a\"b", "c\""], vec![">&2"]),
            (
                "a\"b>nul c\"d\"e>out f\"",
                vec!["a\"b", " c\"d\"e", " f\""],
                vec![">nul", ">out"],
            ),
            ("a\"b>nul", vec!["a\"b"], vec![">nul"]),
            ("a\"b>nul ", vec!["a\"b", " "], vec![">nul"]),
            (
                "a\"b>nul>out c\"",
                vec!["a\"b", " c\""],
                vec![">nul", ">out"],
            ),
            ("a\"b>&%~1c\"", vec!["a\"b", "c\""], vec![">&%~1"]),
            ("a\"b>nul %V% c\"", vec!["a\"b", " %V% c\""], vec![">nul"]),
            ("a\"2>nul c\"", vec!["a\"", " c\""], vec!["2>nul"]),
            ("a\"b2>nul c\"", vec!["a\"b2", " c\""], vec![">nul"]),
            ("a\"b 2>nul c\"", vec!["a\"b ", " c\""], vec!["2>nul"]),
        ] {
            let source = format!("{prefix}\"{name}=x\"\necho tail\n");
            let tree = parser()
                .parse(&source, None)
                .expect("parser should return a tree");
            let root = tree.root_node();
            assert!(!root.has_error(), "{source}: {}", root.to_sexp());
            let binding = root
                .named_child(0)
                .expect("fixture should have this named child")
                .named_child(1)
                .expect("fixture should have this named child");
            assert_eq!(binding.kind(), kind, "{source}");
            let mut cursor = binding.walk();
            let actual_names: Vec<_> = binding
                .children_by_field_name("name", &mut cursor)
                .map(|node| &source[node.byte_range()])
                .collect();
            assert_eq!(actual_names, names, "{source}");
            let actual_redirects: Vec<_> = binding
                .children_by_field_name("redirect", &mut cursor)
                .map(|node| &source[node.byte_range()])
                .collect();
            assert_eq!(actual_redirects, redirects, "{source}");
            assert_eq!(
                &source[binding
                    .child_by_field_name(field)
                    .expect("fixture should have the requested field")
                    .byte_range()],
                "x",
                "{source}"
            );
            assert!(
                node_sources(root, "set_ignored_suffix", &source).is_empty(),
                "{source}"
            );
            assert_eq!(node_sources(root, "command_name", &source), ["echo"]);
        }
    }
    for (source, names, redirects, ignored) in [
        (
            "set \"a\"b>nul c\"\n",
            vec!["a\"b", " c"],
            vec![">nul"],
            vec![],
        ),
        ("set \"a\"b>&2\"\n", vec!["a\"b"], vec![">&2"], vec![]),
        (
            "set \"a\"b>\"out q\"\n",
            vec!["a"],
            vec![">\"out q\""],
            vec!["b"],
        ),
        (
            "set \"a\"b>nul&echo tail\n",
            vec!["a"],
            vec![">nul"],
            vec!["b"],
        ),
    ] {
        let tree = parser()
            .parse(source, None)
            .expect("parser should return a tree");
        let root = tree.root_node();
        assert!(!root.has_error(), "{source}: {}", root.to_sexp());
        assert_eq!(
            node_sources(root, "variable_name", source),
            names,
            "{source}"
        );
        let actual_redirects = if source.contains(">&") {
            node_sources(root, "redirect_dup", source)
        } else {
            node_sources(root, "redirect_file", source)
        };
        assert_eq!(actual_redirects, redirects, "{source}");
        assert_eq!(
            node_sources(root, "set_ignored_suffix", source),
            ignored,
            "{source}"
        );
        assert!(
            node_sources(root, "set_quoted", source).is_empty(),
            "{source}"
        );
    }
}

#[test]
fn path_search_references_keep_non_identifier_environment_names() {
    for name in ["1PATH", "-PATH", "path name", "[PATH]", "éPATH"] {
        for newline in ["\n", "\r\n"] {
            let parameter = format!("%~dp${name}:1");
            let loop_reference = format!("%%~${name}:a");
            let source = format!("echo {parameter} {loop_reference}{newline}");
            let tree = parser().parse(&source, None).expect("path-search tree");
            let root = tree.root_node();
            assert!(!root.has_error(), "{source}: {}", root.to_sexp());
            assert_eq!(node_sources(root, "parameter_tilde", &source), [parameter]);
            assert_eq!(
                node_sources(root, "loop_variable", &source),
                [loop_reference]
            );
        }
    }
}

#[test]
fn unquoted_set_values_and_prompts_preserve_whitespace() {
    for (prefix, field) in [("set ", "value"), ("set /p ", "prompt")] {
        for newline in ["\n", "\r\n"] {
            for (payload, values, redirects) in [
                ("", vec![], vec![]),
                (" ", vec![" "], vec![]),
                ("\t", vec!["\t"], vec![]),
                (" foo ", vec![" foo "], vec![]),
                ("\tfoo \t bar\t", vec!["\tfoo \t bar\t"], vec![]),
                (" %V% !D! ", vec![" %V% !D! "], vec![]),
                (" a^b2>nul c ", vec![" a^b2", " c "], vec![">nul"]),
                (" a 2>nul b ", vec![" a ", " b "], vec!["2>nul"]),
                (" a>nul ", vec![" a", " "], vec![">nul"]),
                (" >nul", vec![" "], vec![">nul"]),
                ("a= b", vec!["a= b"], vec![]),
                (" a^&b ", vec![" a^&b "], vec![]),
                (" \"a&b\" ", vec![" \"a&b\" "], vec![]),
            ] {
                let source = format!("{prefix}x={payload}{newline}");
                let tree = parser().parse(&source, None).expect("unquoted SET tree");
                let root = tree.root_node();
                assert!(!root.has_error(), "{source:?}: {}", root.to_sexp());
                let binding = root
                    .named_child(0)
                    .expect("SET")
                    .named_child(1)
                    .expect("binding");
                let actual: Vec<_> = binding
                    .children_by_field_name(field, &mut binding.walk())
                    .map(|node| &source[node.byte_range()])
                    .collect();
                assert_eq!(actual, values, "{source:?}");
                assert_eq!(
                    node_sources(root, "redirect_file", &source),
                    redirects,
                    "{source:?}"
                );
            }
        }
    }
    for source in ["(set x= foo )&echo tail\n", "set x= foo &echo tail\n"] {
        let tree = parser().parse(source, None).expect("bounded SET tree");
        let root = tree.root_node();
        assert!(!root.has_error(), "{source:?}: {}", root.to_sexp());
        assert_eq!(node_sources(root, "command_name", source), ["echo"]);
        assert_eq!(node_sources(root, "set_assignment", source), ["x= foo "]);
    }
}
