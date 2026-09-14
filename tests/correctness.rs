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
fn caret_set_delayed_fragments_preserve_operator_and_bang_boundaries() {
    for operator in ["&", "|", "<", ">"] {
        let source = format!("set ^\"x=!a\"b!{operator}echo hidden^\"\n");
        let tree = parser().parse(&source, None).unwrap();
        let root = tree.root_node();
        assert!(!root.has_error(), "{source}: {}", root.to_sexp());
        assert_eq!(root.named_child_count(), 1);
        assert_eq!(root.named_child(0).unwrap().kind(), "set_statement");
        assert!(node_sources(root, "delayed_variable", &source).is_empty());
        assert_eq!(node_sources(root, "text", &source), ["x=", &format!("!a\"b!{operator}echo hidden^\"")]);
    }

    for suffix in ["&echo hidden^\"", " !c!&echo hidden^\"", "&echo hidden"] {
        let source = format!("set ^\"x=!a\"b!{suffix}\n");
        let tree = parser().parse(&source, None).unwrap();
        let root = tree.root_node();
        assert!(!root.has_error(), "{source}: {}", root.to_sexp());
        assert_eq!(root.named_child(0).unwrap().kind(), "set_statement");
        assert!(node_sources(root, "delayed_variable", &source).is_empty());
        assert!(node_sources(root, "command_name", &source).is_empty());
        assert_eq!(node_sources(root, "text", &source), ["x=", &format!("!a\"b!{suffix}")]);
    }

    let source = "set ^\"x=!a\"b!\"&echo visible\n";
    let tree = parser().parse(source, None).unwrap();
    let root = tree.root_node();
    assert!(!root.has_error(), "{}", root.to_sexp());
    assert_eq!(node_sources(root, "seq_list", source), [source.trim_end()]);
    assert_eq!(node_sources(root, "command_name", source), ["echo"]);

    let source = "(set ^\"x=!a\"b!)&echo hidden^\")\n";
    let tree = parser().parse(source, None).unwrap();
    let root = tree.root_node();
    assert!(!root.has_error(), "{}", root.to_sexp());
    assert_eq!(node_sources(root, "block", source), [source.trim_end()]);
    assert!(node_sources(root, "seq_list", source).is_empty());

    let source = "set ^\"x=!a\"b\"c!&echo visible^\"\n";
    let tree = parser().parse(source, None).unwrap();
    let root = tree.root_node();
    assert!(!root.has_error(), "{}", root.to_sexp());
    assert_eq!(node_sources(root, "delayed_variable", source), ["!a\"b\"c!"]);
    assert_eq!(node_sources(root, "command_name", source), ["echo"]);
    assert_eq!(node_sources(root, "seq_list", source), [source.trim_end()]);
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
fn caret_quoted_set_preserves_outer_syntax_and_source_fragments() {
    for (source, kind, expected) in [
        ("set ^\"x=foo&echo second^\"\n", "command", "echo second^\""),
        ("set ^\"x=foo&&echo second^\"\n", "and_list", "set ^\"x=foo&&echo second^\""),
        ("set ^\"x=foo||echo second^\"\n", "or_list", "set ^\"x=foo||echo second^\""),
        ("set ^\"x=foo|echo second^\"\n", "pipeline", "set ^\"x=foo|echo second^\""),
        ("set ^\"x=foo>out tail^\"\n", "redirect_file", ">out"),
        ("set ^\"x=foo>out\n", "redirect_file", ">out"),
        ("(set ^\"x=foo) & echo second^\"\n", "block", "(set ^\"x=foo)"),
    ] {
        let tree = parser().parse(source, None).unwrap();
        let root = tree.root_node();
        assert!(!root.has_error(), "{source}: {}", root.to_sexp());
        assert_eq!(node_sources(root, kind, source), [expected]);
    }

    let source = "set ^\"x=%PATH%!SUFFIX!^&done^\"\n";
    let tree = parser().parse(source, None).unwrap();
    let root = tree.root_node();
    assert!(!root.has_error(), "{}", root.to_sexp());
    assert_eq!(node_sources(root, "variable", source), ["%PATH%"]);
    assert_eq!(node_sources(root, "delayed_variable", source), ["!SUFFIX!"]);
    assert_eq!(node_sources(root, "escape_sequence", source), ["^\"", "^&", "^\""]);
    assert!(node_sources(root, "seq_list", source).is_empty());
    assert!(node_sources(root, "string", source).is_empty());
}

#[test]
fn quoted_set_names_preserve_metacharacters_and_expansions() {
    for name in ["x(1)", "x&y", "x|y", "x<y", "x>y", "x^y", "x%N%(1)", "x!N!&y "] {
        for payload in [format!("\"{name}=value\""), format!("/p \"{name}=prompt\""), format!("\"{name}\"")] {
            for source in [format!("set {payload}\n"), format!("(set {payload})\n")] {
                let tree = parser().parse(&source, None).unwrap();
                let root = tree.root_node();
                assert!(!root.has_error(), "{source}: {}", root.to_sexp());
                assert_eq!(node_sources(root, "variable_name", &source), [name]);
                for kind in ["seq_list", "pipeline", "redirect_file", "escape_sequence"] {
                    assert!(node_sources(root, kind, &source).is_empty(), "{source}: {kind}");
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
    let tree = parser().parse(source, None).unwrap();
    let root = tree.root_node();
    assert!(!root.has_error(), "{}", root.to_sexp());
    assert_eq!(node_sources(root, "command_name", source), ["echo"]);
    assert_eq!(node_sources(root, "redirect_file", source), [">out"]);
    assert_eq!(node_sources(root, "escape_sequence", source), ["^&"]);
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
        let tree = parser().parse(&source, None).unwrap();
        let root = tree.root_node();
        assert!(!root.has_error(), "{source}: {}", root.to_sexp());
        let statement = root.named_child(0).unwrap();
        let consequence = statement.child_by_field_name("consequence").unwrap();
        assert_eq!(&source[consequence.byte_range()], tail);
        assert!(statement.child_by_field_name("alternative").is_none());
        assert!(node_sources(consequence, kind, &source).contains(&text), "{source}");
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
        let tree = parser().parse(&source, None).unwrap();
        let root = tree.root_node();
        assert!(!root.has_error(), "{source}: {}", root.to_sexp());
        let statement = root.named_child(0).unwrap();
        let body = statement.child_by_field_name("consequence").unwrap();
        let alternative = statement.child_by_field_name("alternative").unwrap();
        assert_eq!(&source[body.byte_range()], consequence);
        assert_eq!(&source[alternative.byte_range()], "(echo no)");
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
fn if_control_words_do_not_split_compound_operands() {
    for prefix in ["/i", "not", "exist", "defined", "errorlevel", "cmdextversion"] {
        for (suffix, child_kind) in [("\"x\"", "string"), ("%N%", "variable"), ("!N!", "delayed_variable"), ("^x", "escape_sequence")] {
            let operand = format!("{prefix}{suffix}");
            let source = format!("if {operand}==right echo matched\n");
            let tree = parser().parse(&source, None).unwrap();
            let root = tree.root_node();
            assert!(!root.has_error(), "{source}: {}", root.to_sexp());
            let statement = root.named_child(0).unwrap();
            let condition = statement.child_by_field_name("condition").unwrap();
            assert_eq!(condition.kind(), "comparison");
            let left = condition.child_by_field_name("left").unwrap();
            assert_eq!(&source[left.byte_range()], operand);
            assert_eq!(node_sources(left, "text", &source), [prefix]);
            assert_eq!(node_sources(left, child_kind, &source), [suffix]);
            assert_eq!(&source[condition.child_by_field_name("right").unwrap().byte_range()], "right");
            for kind in ["if_flag", "not", "condition_keyword"] {
                assert!(node_sources(root, kind, &source).is_empty(), "{source}: {kind}");
            }
        }
    }
    for separator in [" ", "\t", ",", ";", "="] {
        let source = format!("if /I{separator}NOT{separator}DEFINED{separator}name echo matched\n");
        let tree = parser().parse(&source, None).unwrap();
        let root = tree.root_node();
        assert!(!root.has_error(), "{source}: {}", root.to_sexp());
        assert_eq!(node_sources(root, "if_flag", &source), ["/I"]);
        assert_eq!(node_sources(root, "not", &source), ["NOT"]);
        assert_eq!(node_sources(root, "condition_keyword", &source), ["DEFINED"]);
    }
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

#[test]
fn goto_quotes_preserve_outer_boundaries_and_source_spelling() {
    for (source, names, redirects, blocks) in [
        ("goto \"foo&echo bad\"\n", vec!["\"foo&echo bad\""], vec![], vec![]),
        ("goto \"foo|bar<in>out\"\n", vec!["\"foo|bar<in>out\""], vec![], vec![]),
        ("(goto \"foo)bar\")\n", vec!["\"foo)bar\""], vec![], vec!["(goto \"foo)bar\")"]),
        ("goto pre\"foo>bar\"post>nul tail\n", vec!["pre\"foo>bar\"post", "tail"], vec![">nul"], vec![]),
        ("goto foo>nul \"bar&baz\"\n", vec!["foo", "\"bar&baz\""], vec![">nul"], vec![]),
        ("goto \"foo&bar\";ignored>nul more\n", vec!["\"foo&bar\""], vec![">nul"], vec![]),
        ("goto \"foo&bar\n", vec!["\"foo&bar"], vec![], vec![]),
    ] {
        let tree = parser().parse(source, None).unwrap();
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
    let tree = parser().parse(source, None).unwrap();
    let root = tree.root_node();
    assert!(!root.has_error());
    assert_eq!(node_sources(root, "string", source), ["\"%target%&!suffix!\""]);
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
        let tree = parser().parse(&source, None).unwrap();
        let root = tree.root_node();
        assert!(!root.has_error(), "{source:?}: {}", root.to_sexp());
        assert_eq!(root.named_child_count(), 2);
        let command = root.named_child(0).unwrap();
        let argument_node = command.child_by_field_name("argument").unwrap();
        assert_eq!(&source[argument_node.byte_range()], argument, "{source:?}");
        let redirect = command.child_by_field_name("redirect").unwrap();
        assert_eq!(
            redirect.child_by_field_name("source").map(|node| &source[node.byte_range()]),
            descriptor,
            "{source:?}",
        );
        for (field, expected) in [("operator", operator), ("target", target)] {
            let node = redirect.child_by_field_name(field).unwrap();
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
        let tree = parser().parse(source, None).unwrap();
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
            let tree = parser().parse(&source, None).unwrap();
            let root = tree.root_node();
            assert!(!root.has_error(), "{source:?}: {}", root.to_sexp());
            let statement = root.named_child(0).unwrap();
            let condition = statement.child_by_field_name("condition").unwrap();
            let argument = condition.child_by_field_name(field).unwrap();
            assert_eq!(&source[argument.byte_range()], expected, "{source:?}");
            let consequence = statement.child_by_field_name("consequence").unwrap();
            let redirect = consequence.child_by_field_name("redirect").unwrap();
            assert_eq!(
                redirect.child_by_field_name("source").map(|node| &source[node.byte_range()]),
                descriptor,
                "{source:?}",
            );
            let expected_body = if descriptor.is_some() { "2>nul echo yes" } else { ">nul echo yes" };
            assert_eq!(&source[consequence.byte_range()], expected_body, "{source:?}");
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
            ("a\"b>out c\"d\"e>two f\"", vec!["a\"b", " c\"d\"e", " f"], vec![">out", ">two"]),
            ("a\"2>out c\"", vec!["a\"", " c"], vec!["2>out"]),
            ("a\"b2>out c\"", vec!["a\"b2", " c"], vec![">out"]),
            ("a\"b>%v:\"=% c\"", vec!["a\"b", " c"], vec![">%v:\"=%"]),
            ("a\"b>\"%v:\"=%\" c\"", vec!["a\"b", " c"], vec![">\"%v:\"=%\""]),
            ("a\"b>%v: =% c\"", vec!["a\"b", " c"], vec![">%v: =%"]),
            ("a\"b>%v:&=% c\"", vec!["a\"b", " c"], vec![">%v:&=%"]),
            ("a\"b>out% c\"", vec!["a\"b", " c"], vec![">out%"]),
        ] {
            let source = format!("{prefix}\"x={tail}\n");
            let tree = parser().parse(&source, None).unwrap();
            let root = tree.root_node();
            assert!(!root.has_error(), "{source}: {}", root.to_sexp());
            let binding = root.named_child(0).unwrap().named_child(1).unwrap();
            let mut cursor = binding.walk();
            let actual_values: Vec<_> = binding.children_by_field_name(field, &mut cursor)
                .map(|node| &source[node.byte_range()]).collect();
            assert_eq!(actual_values, values, "{source}");
            let actual_redirects: Vec<_> = binding.children_by_field_name("redirect", &mut cursor)
                .map(|node| &source[node.byte_range()]).collect();
            assert_eq!(actual_redirects, redirects, "{source}");
            assert!(node_sources(root, "set_ignored_suffix", &source).is_empty());
        }
    }
    for tail in ["b>\"out q\"", "b>out&echo \"after\"", "b>out|echo \"after\"", "b>%v:\"=%", "b>out%\" c\""] {
        let source = format!("set \"x=a\"{tail}\n");
        let tree = parser().parse(&source, None).unwrap();
        let root = tree.root_node();
        assert!(!root.has_error(), "{source}: {}", root.to_sexp());
        assert_eq!(node_sources(root, "set_ignored_suffix", &source), ["b"]);
    }
}
