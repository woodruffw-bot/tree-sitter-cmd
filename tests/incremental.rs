use std::ops::Range;

use tree_sitter::{InputEdit, Parser, Point, Tree};

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
        "initial source must parse cleanly",
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
