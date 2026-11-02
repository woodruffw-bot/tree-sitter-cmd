use tree_sitter::{InputEdit, Node, Parser, Point};

fn parser() -> Parser {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_cmd::LANGUAGE.into())
        .expect("loading Cmd grammar");
    parser
}

fn markers<'tree>(node: Node<'tree>) -> Vec<Node<'tree>> {
    let mut result = Vec::new();
    if node.kind() == "_body_boundary" {
        assert!(!node.is_named());
        assert!(!node.is_missing());
        assert!(!node.is_error());
        assert!(!node.has_error());
        assert!(node.byte_range().is_empty());
        assert_eq!(node.start_position(), node.end_position());
        assert!(node
            .parent()
            .expect("recovery node should have a parent")
            .has_error());
        result.push(node);
    } else if node.byte_range().is_empty() {
        assert!(node.is_missing(), "unexpected empty node: {}", node.kind());
    }
    for index in 0..node.child_count() {
        let child = node.child(index).expect("child index should be in range");
        if child.kind() == "_body_boundary" {
            assert_eq!(node.field_name_for_child(index), None);
        }
        result.extend(markers(child));
    }
    result
}

#[test]
fn missing_bodies_expose_only_anonymous_boundary_markers() {
    for (prefix, field, kind) in [
        ("if exist marker", "consequence", "if_statement"),
        ("for %%i in (one) do", "body", "for_statement"),
        (
            "if exist marker (echo yes) else",
            "alternative",
            "if_statement",
        ),
        ("@", "body", "quiet_statement"),
    ] {
        for ending in [
            "",
            "\n",
            "\r\n",
            "\necho tail\n",
            "\r\necho tail\r\n",
            "\necho after\n",
            "\ngoto tail\n",
            "\n(echo tail)\n",
        ] {
            let source = format!("{prefix}{ending}");
            let tree = parser()
                .parse(&source, None)
                .expect("parser should return a tree");
            let root = tree.root_node();
            assert!(root.has_error(), "{source}");
            let controller = root
                .named_child(0)
                .expect("fixture should have this named child");
            assert_eq!(controller.kind(), kind);
            assert!(controller.has_error());
            assert_eq!(controller.end_position().row, 0);
            let missing = controller
                .child_by_field_name(field)
                .expect("fixture should have the requested field");
            assert_eq!(missing.kind(), "command");
            assert!(!missing.is_named());
            assert!(missing.is_missing());
            assert_eq!(missing.byte_range(), prefix.len()..prefix.len());
            assert_eq!(missing.start_position(), Point::new(0, prefix.len()));

            let boundaries = markers(root);
            assert_eq!(boundaries.len(), 2, "{source}");
            for boundary in boundaries {
                assert_eq!(boundary.byte_range(), missing.byte_range());
                assert_eq!(
                    boundary
                        .parent()
                        .expect("recovery node should have a parent"),
                    controller
                );
            }
            if let Some(tail_source) = ending.lines().nth(1) {
                assert_eq!(root.named_child_count(), 2);
                let tail = root
                    .named_child(1)
                    .expect("fixture should have this named child");
                assert_eq!(&source[tail.byte_range()], tail_source);
                let tail_kind = if tail_source.starts_with("goto") {
                    "goto_statement"
                } else if tail_source.starts_with('(') {
                    "block"
                } else {
                    "command"
                };
                assert_eq!(tail.kind(), tail_kind);
                assert_eq!(tail.start_position(), Point::new(1, 0));
                assert!(!tail.has_error());
            }
        }
    }
}

#[test]
fn empty_block_marker_retains_a_missing_name_and_outer_scope() {
    for (source, boundary_byte, right_source) in [
        ("()\necho tail\n", 1, None),
        ("( )\necho tail\n", 2, None),
        ("(() & echo right)\necho tail\n", 2, Some("echo right")),
        ("(() & echo x)\n", 2, Some("echo x")),
    ] {
        let tree = parser()
            .parse(source, None)
            .expect("parser should return a tree");
        let root = tree.root_node();
        assert!(root.has_error());
        let boundaries = markers(root);
        assert_eq!(boundaries.len(), 1);
        let boundary = boundaries[0];
        assert_eq!(boundary.byte_range(), boundary_byte..boundary_byte);
        let block = boundary
            .parent()
            .expect("recovery node should have a parent");
        assert_eq!(block.kind(), "block");
        assert_eq!(block.named_child_count(), 1);
        let name = block
            .named_child(0)
            .expect("fixture should have this named child");
        assert_eq!(name.kind(), "command_name");
        assert!(name.is_missing());
        assert_eq!(name.byte_range(), boundary.byte_range());
        assert_eq!(root.named_child_count(), source.lines().count());
        let outer = root
            .named_child(0)
            .expect("fixture should have this named child");
        assert_eq!(outer.kind(), "block");
        assert_eq!(
            &source[outer.byte_range()],
            source
                .lines()
                .next()
                .expect("fixture should contain a first line")
        );
        if let Some(right_source) = right_source {
            let sequence = outer
                .named_child(0)
                .expect("fixture should have this named child");
            assert_eq!(sequence.kind(), "seq_list");
            assert_eq!(
                sequence
                    .child_by_field_name("left")
                    .expect("fixture should have a left field"),
                block
            );
            let right = sequence
                .child_by_field_name("right")
                .expect("fixture should have a right field");
            assert_eq!(right.kind(), "command");
            assert_eq!(&source[right.byte_range()], right_source);
            assert!(!right.has_error());
        }
        if let Some(tail) = root.named_child(1) {
            assert_eq!(tail.kind(), "command");
            assert_eq!(&source[tail.byte_range()], "echo tail");
            assert!(!tail.has_error());
        }
    }
}

fn assert_same_children(incremental: Node<'_>, fresh: Node<'_>) {
    assert_eq!(incremental.kind(), fresh.kind());
    assert_eq!(incremental.is_named(), fresh.is_named());
    assert_eq!(incremental.is_missing(), fresh.is_missing());
    assert_eq!(incremental.is_error(), fresh.is_error());
    assert_eq!(incremental.has_error(), fresh.has_error());
    assert_eq!(incremental.range(), fresh.range());
    assert_eq!(incremental.child_count(), fresh.child_count());
    for index in 0..fresh.child_count() {
        assert_eq!(
            incremental.field_name_for_child(index),
            fresh.field_name_for_child(index)
        );
        assert_same_children(
            incremental
                .child(index)
                .expect("child index should be in range"),
            fresh.child(index).expect("child index should be in range"),
        );
    }
}

#[test]
fn deleting_and_restoring_bodies_preserves_the_complete_cst() {
    for (prefix, body, suffix, kind) in [
        (
            "if exist marker",
            " echo body",
            "\necho tail\n",
            "if_statement",
        ),
        (
            "for %%i in (one) do",
            " echo body",
            "\necho tail\n",
            "for_statement",
        ),
        (
            "if exist marker (echo yes) else",
            " echo no",
            "\necho tail\n",
            "if_statement",
        ),
        ("@", "echo body", "\necho tail\n", "quiet_statement"),
        ("(", "echo body", ")\necho tail\n", "block"),
        ("((", "echo body", ") & echo right)\necho tail\n", "block"),
    ] {
        let mut parser = parser();
        let mut source = format!("{prefix}{body}{suffix}");
        let mut tree = parser
            .parse(&source, None)
            .expect("parser should return a tree");
        assert!(!tree.root_node().has_error());
        let mut body_len = body.len();
        for replacement in ["", body] {
            let start = prefix.len();
            tree.edit(&InputEdit {
                start_byte: start,
                old_end_byte: start + body_len,
                new_end_byte: start + replacement.len(),
                start_position: Point::new(0, start),
                old_end_position: Point::new(0, start + body_len),
                new_end_position: Point::new(0, start + replacement.len()),
            });
            source.replace_range(start..start + body_len, replacement);
            tree = parser
                .parse(&source, Some(&tree))
                .expect("parser should return a tree");
            let fresh = self::parser()
                .parse(&source, None)
                .expect("parser should return a tree");
            assert_same_children(tree.root_node(), fresh.root_node());
            assert_eq!(tree.root_node().has_error(), replacement.is_empty());
            assert_eq!(
                markers(tree.root_node()).is_empty(),
                !replacement.is_empty()
            );
            let root = tree.root_node();
            assert_eq!(root.named_child_count(), 2);
            let controller = root
                .named_child(0)
                .expect("fixture should have this named child");
            assert_eq!(controller.kind(), kind);
            assert_eq!(controller.has_error(), replacement.is_empty());
            assert_eq!(controller.end_position().row, 0);
            let tail = root
                .named_child(1)
                .expect("fixture should have this named child");
            assert_eq!(tail.kind(), "command");
            assert_eq!(tail.start_position().row, 1);
            assert!(!tail.has_error());
            body_len = replacement.len();
        }
    }
}

#[test]
fn required_goto_and_call_targets_retain_recovery_state() {
    let cases: &[(&[u8], &str)] = &[
        (b"goto\necho tail\n", "ERROR"),
        (b"call\necho tail\n", "ERROR"),
        (b"goto >log\necho tail\n", "goto_statement"),
        (b"call <input\necho tail\n", "call_statement"),
    ];

    for &(source, controller_kind) in cases {
        let tree = parser().parse(source, None).expect("malformed parse");
        let root = tree.root_node();
        assert!(root.has_error(), "missing target parsed without recovery");
        assert_eq!(root.named_child_count(), 2);

        let controller = root.named_child(0).expect("controller");
        let tail = root.named_child(1).expect("tail command");
        assert_eq!(controller.kind(), controller_kind);
        assert!(
            controller.has_error(),
            "controller lost missing-target state"
        );
        assert_eq!(controller.end_position().row, 0);
        assert_eq!(tail.kind(), "command");
        assert_eq!(tail.start_position().row, 1);
        assert!(!tail.has_error(), "next-line command entered recovery");
    }
}
