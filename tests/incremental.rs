use std::ops::Range;

use tree_sitter::{InputEdit, Node, Parser, Point, Tree};

fn parser() -> Parser {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_cmd::LANGUAGE.into())
        .expect("loading Cmd grammar");
    parser
}

fn point_at(source: &[u8], byte: usize) -> Point {
    let mut point = Point::new(0, 0);
    for &ch in &source[..byte] {
        if ch == b'\n' {
            point.row += 1;
            point.column = 0;
        } else {
            point.column += 1;
        }
    }
    point
}

fn point_after(mut point: Point, text: &[u8]) -> Point {
    for &ch in text {
        if ch == b'\n' {
            point.row += 1;
            point.column = 0;
        } else {
            point.column += 1;
        }
    }
    point
}

fn edited_tree(source: &[u8], range: Range<usize>, replacement: &[u8]) -> (Tree, Vec<u8>) {
    let mut parser = parser();
    let mut old_tree = parser.parse(source, None).expect("initial parse");
    assert!(
        !old_tree.root_node().has_error(),
        "initial source must parse without Tree-sitter recovery",
    );
    let start_position = point_at(source, range.start);
    let edit = InputEdit {
        start_byte: range.start,
        old_end_byte: range.end,
        new_end_byte: range.start + replacement.len(),
        start_position,
        old_end_position: point_at(source, range.end),
        new_end_position: point_after(start_position, replacement),
    };
    old_tree.edit(&edit);

    let mut edited = source.to_vec();
    edited.splice(range, replacement.iter().copied());
    let tree = parser
        .parse(edited.as_slice(), Some(&old_tree))
        .expect("incremental parse");
    (tree, edited)
}

fn assert_incremental_matches_fresh(before: &str, needle: &str, replacement: &str) {
    let start = before.find(needle).expect("edit needle");
    let range = start..start + needle.len();
    let (incremental, edited) = edited_tree(before.as_bytes(), range, replacement.as_bytes());
    let fresh = parser()
        .parse(edited.as_slice(), None)
        .expect("fresh parse");
    assert!(
        !fresh.root_node().has_error(),
        "edited source must parse cleanly: {}",
        String::from_utf8_lossy(&edited),
    );

    assert_eq!(
        incremental.root_node().to_sexp(),
        fresh.root_node().to_sexp(),
        "incremental and fresh trees differ for edit {needle:?} -> {replacement:?}",
    );
    assert_eq!(
        incremental.root_node().has_error(),
        fresh.root_node().has_error(),
        "incremental and fresh error states differ",
    );
    assert_eq!(incremental.root_node().end_byte(), edited.len());
}

#[test]
fn scanner_sensitive_edits_match_fresh_parses() {
    let cases = [
        (
            "if exist x (\r\n  echo a\r\n)\r\necho tail\r\n",
            "echo a",
            "if exist y (\r\n    echo b^)\r\n  )",
        ),
        (
            "echo \"value\"\necho tail\n",
            "\"value\"",
            "\"value %PATH%\"",
        ),
        ("remote note\necho tail\n", "remote note", "rem note"),
        ("echo one ^\r\n  two\r\n", "one ^", "one^"),
        (
            "set \"x=a\"tail\r\necho after\r\n",
            "tail",
            "tail\"",
        ),
        (
            "set \"x=a\"tail\"\r\necho after\r\n",
            "tail\"",
            "tail",
        ),
        (": target\necho after\n", "target", "  "),
        ("goto :loop\necho after\n", "loop", ""),
    ];

    for (before, needle, replacement) in cases {
        assert_incremental_matches_fresh(before, needle, replacement);
    }
}

#[test]
fn malformed_edit_keeps_error_inside_closed_block() {
    let before = "(\necho inside\n)\necho after\n";
    let needle = "echo inside";
    let start = before.find(needle).expect("edit needle");
    let (incremental, edited) = edited_tree(
        before.as_bytes(),
        start..start + needle.len(),
        b"if",
    );
    let fresh = parser()
        .parse(edited.as_slice(), None)
        .expect("fresh malformed parse");

    assert_eq!(
        incremental.root_node().to_sexp(),
        fresh.root_node().to_sexp(),
        "incremental recovery must match a fresh parse",
    );

    let root = incremental.root_node();
    let block = root.named_child(0).expect("recovered block");
    let tail = root.named_child(1).expect("command after block");
    let close_end = edited
        .windows(2)
        .position(|window| window == b")\n")
        .expect("block close")
        + 1;
    let tail_start = edited
        .windows(b"echo after".len())
        .position(|window| window == b"echo after")
        .expect("tail command");

    assert_eq!(block.kind(), "block");
    assert!(block.has_error(), "the malformed statement must stay marked");
    assert_eq!(block.end_byte(), close_end);
    assert_eq!(tail.kind(), "command");
    assert!(!tail.has_error(), "the later command must remain clean");
    assert_eq!(tail.start_byte(), tail_start);
}

#[test]
fn same_line_body_edits_keep_next_line_as_sibling() {
    let cases = [
        (
            "if exist marker echo body\necho after\n",
            "echo body",
            "",
            "if_statement",
            "consequence",
            true,
        ),
        (
            "if exist marker\necho after\n",
            "\n",
            " echo body\n",
            "if_statement",
            "consequence",
            false,
        ),
        (
            "for %%i in (one) do echo %%i\r\necho after\r\n",
            "echo %%i",
            "",
            "for_statement",
            "body",
            true,
        ),
        (
            "for %%i in (one) do\r\necho after\r\n",
            "\r\n",
            " echo %%i\r\n",
            "for_statement",
            "body",
            false,
        ),
    ];

    for (before, needle, replacement, statement_kind, body_field, expect_sentinel) in cases {
        let start = before.find(needle).expect("edit needle");
        let range = start..start + needle.len();
        let (incremental, edited) =
            edited_tree(before.as_bytes(), range, replacement.as_bytes());
        let fresh = parser()
            .parse(edited.as_slice(), None)
            .expect("fresh body-edit parse");

        assert_eq!(
            incremental.root_node().to_sexp(),
            fresh.root_node().to_sexp(),
            "incremental sentinel parse must match fresh for {statement_kind}",
        );
        assert!(!incremental.root_node().has_error());
        assert!(!fresh.root_node().has_error());

        let root = incremental.root_node();
        assert_eq!(root.named_child_count(), 2);
        let statement = root.named_child(0).expect("edited statement");
        let body = statement
            .child_by_field_name(body_field)
            .expect("statement body field");
        let tail = root.named_child(1).expect("next-line command");
        let tail_start = edited
            .windows(b"echo after".len())
            .position(|window| window == b"echo after")
            .expect("tail command");

        assert_eq!(statement.kind(), statement_kind);
        assert!(!statement.has_error());
        assert_eq!(
            body.kind(),
            if expect_sentinel {
                "missing_statement"
            } else {
                "command"
            },
        );
        assert!(!body.is_missing());
        assert!(statement.end_byte() <= tail_start);
        assert_eq!(tail.kind(), "command");
        assert!(!tail.has_error(), "the next physical line must remain clean");
        assert_eq!(tail.start_byte(), tail_start);
        assert_eq!(root.end_byte(), edited.len());
    }
}

#[test]
fn same_line_block_close_survives_for_body_edits() {
    let cases = [
        (
            "(\nfor %%i in (one) do echo %%i )\necho after\n",
            "echo %%i",
            "",
            true,
        ),
        (
            "(\nfor %%i in (one) do )\necho after\n",
            " )",
            " echo %%i )",
            false,
        ),
    ];

    for (before, needle, replacement, expect_sentinel) in cases {
        let start = before.find(needle).expect("edit needle");
        let (incremental, edited) = edited_tree(
            before.as_bytes(),
            start..start + needle.len(),
            replacement.as_bytes(),
        );
        let fresh = parser()
            .parse(edited.as_slice(), None)
            .expect("fresh block-close parse");

        assert_eq!(
            incremental.root_node().to_sexp(),
            fresh.root_node().to_sexp(),
            "incremental and fresh block-close trees differ",
        );
        let root = incremental.root_node();
        assert!(!root.has_error());
        assert_eq!(root.named_child_count(), 2);
        let block = root.named_child(0).expect("block");
        let for_statement = block.named_child(0).expect("FOR in block");
        let body = for_statement
            .child_by_field_name("body")
            .expect("FOR body");
        let tail = root.named_child(1).expect("tail command");

        assert_eq!(block.kind(), "block");
        assert_eq!(for_statement.kind(), "for_statement");
        assert_eq!(
            body.kind(),
            if expect_sentinel {
                "missing_statement"
            } else {
                "command"
            },
        );
        assert_eq!(tail.kind(), "command");
        assert!(!tail.has_error());
        assert_eq!(root.end_byte(), edited.len());
    }
}

fn collect_missing_statement_offsets(node: Node<'_>, offsets: &mut Vec<usize>) {
    if node.kind() == "missing_statement" {
        offsets.push(node.start_byte());
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_missing_statement_offsets(child, offsets);
    }
}

fn missing_statement_offsets(tree: &Tree) -> Vec<usize> {
    let mut offsets = Vec::new();
    collect_missing_statement_offsets(tree.root_node(), &mut offsets);
    offsets
}

#[test]
fn distant_body_sentinel_does_not_change_edited_controller_shape() {
    let cases = [
        (
            "if exist marker echo first\necho after\nif exist other\necho after-two\n",
            "echo first",
            "",
            true,
            2,
        ),
        (
            "if exist marker\necho after\nif exist other\necho after-two\n",
            "\n",
            " echo first\n",
            false,
            1,
        ),
    ];

    for (before, needle, replacement, first_sentinel, sentinel_count) in cases {
        let start = before.find(needle).expect("edit needle");
        let (incremental, edited) = edited_tree(
            before.as_bytes(),
            start..start + needle.len(),
            replacement.as_bytes(),
        );
        let fresh = parser()
            .parse(edited.as_slice(), None)
            .expect("fresh repeated-controller parse");

        assert_eq!(
            incremental.root_node().to_sexp(),
            fresh.root_node().to_sexp(),
            "incremental and fresh repeated-controller trees differ",
        );
        assert_eq!(
            missing_statement_offsets(&incremental),
            missing_statement_offsets(&fresh),
            "incremental and fresh sentinel nodes differ",
        );
        assert_eq!(
            missing_statement_offsets(&incremental).len(),
            sentinel_count,
        );

        let root = incremental.root_node();
        assert!(!root.has_error());
        assert_eq!(root.named_child_count(), 4);
        let first = root.named_child(0).expect("first IF");
        let first_body = first
            .child_by_field_name("consequence")
            .expect("first IF consequence");
        let first_tail = root.named_child(1).expect("first sibling command");
        let second = root.named_child(2).expect("second IF");
        let second_body = second
            .child_by_field_name("consequence")
            .expect("second IF consequence");
        let second_tail = root.named_child(3).expect("second sibling command");

        assert_eq!(first.kind(), "if_statement");
        assert_eq!(
            first_body.kind(),
            if first_sentinel {
                "missing_statement"
            } else {
                "command"
            },
        );
        assert!(!first_body.is_missing());
        assert_eq!(first_tail.kind(), "command");
        assert!(!first_tail.has_error());
        assert_eq!(second.kind(), "if_statement");
        assert_eq!(second_body.kind(), "missing_statement");
        assert!(!second_body.is_missing());
        assert_eq!(second_tail.kind(), "command");
        assert!(!second_tail.has_error());
        assert_eq!(root.end_byte(), edited.len());
    }
}

fn parse_chunked(source: &[u8], chunk_size: usize) -> Tree {
    parser()
        .parse_with_options(
            &mut |byte, _| {
                if byte >= source.len() {
                    return &source[source.len()..];
                }
                let end = (byte + chunk_size).min(source.len());
                &source[byte..end]
            },
            None,
            None,
        )
        .expect("chunked parse")
}

#[test]
fn chunked_input_matches_contiguous_input() {
    let source = concat!(
        "@echo off\r\n",
        "if exist \"café.txt\" (\r\n",
        "  echo one ^\r\n",
        "    two^) %PATH%\r\n",
        ")\r\n",
        ":   \r\n",
        ": target\r\n",
        "(goto :loop ; ignored)\r\n",
        "goto :\r\n",
    )
    .as_bytes();
    let contiguous = parser().parse(source, None).expect("contiguous parse");
    assert!(!contiguous.root_node().has_error());

    for chunk_size in [1, 2, 7, 64] {
        let chunked = parse_chunked(source, chunk_size);
        assert_eq!(
            chunked.root_node().to_sexp(),
            contiguous.root_node().to_sexp(),
            "tree differs at chunk size {chunk_size}",
        );
        assert_eq!(
            chunked.root_node().has_error(),
            contiguous.root_node().has_error(),
            "error state differs at chunk size {chunk_size}",
        );
    }
}

#[test]
fn chunked_malformed_controllers_match_contiguous_input() {
    let source = concat!(
        "if exist marker\n",
        "for %%i in (one) do\n",
        "if exist other echo yes else\n",
        "echo tail\n",
    )
    .as_bytes();
    let contiguous = parser().parse(source, None).expect("contiguous parse");
    let root = contiguous.root_node();
    assert!(!root.has_error());
    assert_eq!(root.named_child_count(), 4);

    let first_if = root.named_child(0).expect("first IF");
    let for_statement = root.named_child(1).expect("FOR");
    let second_if = root.named_child(2).expect("second IF");
    let tail = root.named_child(3).expect("tail command");
    assert_eq!(
        first_if
            .child_by_field_name("consequence")
            .expect("first IF consequence")
            .kind(),
        "missing_statement",
    );
    assert_eq!(
        for_statement
            .child_by_field_name("body")
            .expect("FOR body")
            .kind(),
        "missing_statement",
    );
    assert_eq!(
        second_if
            .child_by_field_name("alternative")
            .expect("second IF alternative")
            .kind(),
        "missing_statement",
    );
    assert_eq!(tail.kind(), "command");
    assert!(!tail.has_error());
    let contiguous_sentinels = missing_statement_offsets(&contiguous);
    assert_eq!(contiguous_sentinels.len(), 3);

    for chunk_size in [1, 2, 7, 64] {
        let chunked = parse_chunked(source, chunk_size);
        assert_eq!(
            chunked.root_node().to_sexp(),
            contiguous.root_node().to_sexp(),
            "malformed tree differs at chunk size {chunk_size}",
        );
        assert_eq!(
            missing_statement_offsets(&chunked),
            contiguous_sentinels,
            "sentinel nodes differ at chunk size {chunk_size}",
        );
        assert_eq!(
            chunked.root_node().has_error(),
            contiguous.root_node().has_error(),
            "error state differs at chunk size {chunk_size}",
        );
    }
}

#[test]
fn body_sentinel_at_eof_is_stable_for_chunked_input() {
    let cases = [
        ("if exist marker", "if_statement", "consequence"),
        ("for %%i in (one) do", "for_statement", "body"),
        (
            "if exist marker echo yes else",
            "if_statement",
            "alternative",
        ),
    ];

    for (source, statement_kind, body_field) in cases {
        let bytes = source.as_bytes();
        let contiguous = parser().parse(bytes, None).expect("contiguous EOF parse");
        let root = contiguous.root_node();
        assert!(!root.has_error());
        assert_eq!(root.named_child_count(), 1);
        let statement = root.named_child(0).expect("controller at EOF");
        let body = statement
            .child_by_field_name(body_field)
            .expect("body sentinel at EOF");
        assert_eq!(statement.kind(), statement_kind);
        assert_eq!(body.kind(), "missing_statement");
        assert!(!body.is_missing());
        assert_eq!(body.start_byte(), bytes.len());
        assert_eq!(body.end_byte(), bytes.len());

        for chunk_size in [1, 2, 7, 64] {
            let chunked = parse_chunked(bytes, chunk_size);
            assert_eq!(
                chunked.root_node().to_sexp(),
                contiguous.root_node().to_sexp(),
                "EOF tree differs for {statement_kind} at chunk size {chunk_size}",
            );
            assert_eq!(
                missing_statement_offsets(&chunked),
                missing_statement_offsets(&contiguous),
                "EOF sentinel differs for {statement_kind} at chunk size {chunk_size}",
            );
            assert_eq!(
                chunked.root_node().has_error(),
                contiguous.root_node().has_error(),
                "EOF error state differs for {statement_kind} at chunk size {chunk_size}",
            );
        }
    }
}
