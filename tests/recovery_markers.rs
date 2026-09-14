use tree_sitter::{InputEdit, Node, Parser, Point};

fn parser() -> Parser {
    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_cmd::LANGUAGE.into()).unwrap();
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
        assert!(node.parent().unwrap().has_error());
        result.push(node);
    } else if node.byte_range().is_empty() {
        assert!(node.is_missing(), "unexpected empty node: {}", node.kind());
    }
    for index in 0..node.child_count() {
        let child = node.child(index as u32).unwrap();
        if child.kind() == "_body_boundary" {
            assert_eq!(node.field_name_for_child(index as u32), None);
        }
        result.extend(markers(child));
    }
    result
}

#[test]
fn missing_bodies_expose_only_anonymous_boundary_markers() {
    for (prefix, field) in [
        ("if exist marker", "consequence"),
        ("for %%i in (one) do", "body"),
        ("if exist marker (echo yes) else", "alternative"),
        ("@", "body"),
    ] {
        for ending in ["", "\n", "\r\n", "\necho tail\n", "\r\necho tail\r\n"] {
            let source = format!("{prefix}{ending}");
            let tree = parser().parse(&source, None).unwrap();
            let root = tree.root_node();
            assert!(root.has_error(), "{source}");
            let controller = root.named_child(0).unwrap();
            let missing = controller.child_by_field_name(field).unwrap();
            assert_eq!(missing.kind(), "command");
            assert!(!missing.is_named());
            assert!(missing.is_missing());
            assert_eq!(missing.byte_range(), prefix.len()..prefix.len());
            assert_eq!(missing.start_position(), Point::new(0, prefix.len()));

            let boundaries = markers(root);
            assert_eq!(boundaries.len(), 2, "{source}");
            for boundary in boundaries {
                assert_eq!(boundary.byte_range(), missing.byte_range());
                assert_eq!(boundary.parent().unwrap(), controller);
            }
            if ending.contains("echo tail") {
                assert_eq!(root.named_child_count(), 2);
                let tail = root.named_child(1).unwrap();
                assert_eq!(&source[tail.byte_range()], "echo tail");
                assert_eq!(tail.start_position(), Point::new(1, 0));
                assert!(!tail.has_error());
            }
        }
    }
}

#[test]
fn empty_block_marker_retains_a_missing_name_and_outer_scope() {
    for (source, boundary_byte) in [
        ("()\necho tail\n", 1),
        ("( )\necho tail\n", 2),
        ("(() & echo right)\necho tail\n", 2),
    ] {
        let tree = parser().parse(source, None).unwrap();
        let root = tree.root_node();
        assert!(root.has_error());
        let boundaries = markers(root);
        assert_eq!(boundaries.len(), 1);
        let boundary = boundaries[0];
        assert_eq!(boundary.byte_range(), boundary_byte..boundary_byte);
        let block = boundary.parent().unwrap();
        assert_eq!(block.kind(), "block");
        let name = block.named_child(0).unwrap();
        assert_eq!(name.kind(), "command_name");
        assert!(name.is_missing());
        assert_eq!(name.byte_range(), boundary.byte_range());
        assert_eq!(root.named_child_count(), 2);
        let outer = root.named_child(0).unwrap();
        assert_eq!(outer.kind(), "block");
        assert_eq!(&source[outer.byte_range()], source.lines().next().unwrap());
        let tail = root.named_child(1).unwrap();
        assert_eq!(&source[tail.byte_range()], "echo tail");
        assert!(!tail.has_error());
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
    for index in 0..fresh.child_count() as u32 {
        assert_eq!(incremental.field_name_for_child(index), fresh.field_name_for_child(index));
        assert_same_children(incremental.child(index).unwrap(), fresh.child(index).unwrap());
    }
}

#[test]
fn deleting_and_restoring_bodies_preserves_the_complete_cst() {
    for (prefix, body, suffix) in [
        ("if exist marker", " echo body", "\necho tail\n"),
        ("for %%i in (one) do", " echo body", "\necho tail\n"),
        ("if exist marker (echo yes) else", " echo body", "\necho tail\n"),
        ("@", "echo body", "\necho tail\n"),
        ("(", "echo body", ")\necho tail\n"),
        ("((", "echo body", ") & echo right)\necho tail\n"),
    ] {
        let mut parser = parser();
        let mut source = format!("{prefix}{body}{suffix}");
        let mut tree = parser.parse(&source, None).unwrap();
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
            tree = parser.parse(&source, Some(&tree)).unwrap();
            let fresh = self::parser().parse(&source, None).unwrap();
            assert_same_children(tree.root_node(), fresh.root_node());
            assert_eq!(tree.root_node().has_error(), replacement.is_empty());
            assert_eq!(markers(tree.root_node()).is_empty(), !replacement.is_empty());
            body_len = replacement.len();
        }
    }
}
