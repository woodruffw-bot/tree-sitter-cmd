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
        ("if /?\necho tail\n", "/?", "exist marker echo body"),
        ("for /?\necho tail\n", "/?", "%%i in (one) do echo %%i"),
        ("rem /?\necho tail\n", "/?", "/? note"),
        ("if exist x if /?\necho tail\n", "/?", "exist y echo body"),
        (
            "for %%x in (a) do for /?\necho tail\n",
            "/?",
            "%%y in (b) do echo %%y",
        ),
        (
            "if exist x echo yes else if /?\necho tail\n",
            "/?",
            "exist y echo no",
        ),
        ("if exist x if /? >nul\necho tail\n", ">nul", "2>log"),
        ("IF /? >nul\necho tail\n", ">nul", "2>log"),
        ("REM /?<log\necho tail\n", "<log", ">out"),
        (
            "IF /? >foo=^\r\n;2>bar\r\necho tail\r\n",
            "bar",
            "baz",
        ),
        (
            "IF /? >%\" \";p\r\necho tail\r\n",
            ";p",
            ";q",
        ),
        (
            "IF /? >%1x=%\r\necho tail\r\n",
            "x",
            "y",
        ),
        (
            "IF /? >=%1\"%1 \"\r\necho tail\r\n",
            "%1 ",
            "%2 ",
        ),
        (
            "IF /? >a^\r\n=%1\r\necho tail\r\n",
            "%1",
            "%2",
        ),
        (
            "IF /? >a^\r\n=%1\r\necho tail\r\n",
            ">a^",
            ">%1^",
        ),
        ("IF /? > %1,b\necho tail\n", ",b", ";(="),
        ("(IF /? >=x)\necho tail\n", "x", "("),
        ("IF /? >a^\r\n =%\r\necho tail\r\n", "=%", "==%"),
    ];

    for (before, needle, replacement) in cases {
        assert_incremental_matches_fresh(before, needle, replacement);
    }
}

#[test]
fn help_boundary_edits_preserve_malformed_state() {
    let cases = [
        ("IF /?\necho tail\n", "/?", "/? extra"),
        ("if exist x if /?\necho tail\n", "/?", "/? extra"),
        ("FOR /? 2>nul\necho tail\n", "2>nul", "2 >nul"),
        ("IF /? >nul\necho tail\n", ">nul", ">nul extra"),
        ("FOR /? 2>nul\necho tail\n", "2>nul", "2>nul extra"),
        ("IF /? >%foo\necho tail\n", "%foo", "%foo extra"),
        ("IF /? > %a\necho tail\n", "%a", "%=a"),
        ("IF /? 2>nul\necho tail\n", "? ", "?"),
        ("IF /? > a(x\necho tail\n", "(x", "(=b"),
        ("IF /? >!a!\necho tail\n", "!a!", "!!a!=!"),
        ("IF /? > %1,b\necho tail\n", ",", "="),
        (
            "IF /? >foo=^\r\n;2>bar\r\necho tail\r\n",
            ";2",
            "2",
        ),
    ];

    for (before, needle, replacement) in cases {
        let start = before.find(needle).expect("help edit");
        let (incremental, edited) = edited_tree(
            before.as_bytes(),
            start..start + needle.len(),
            replacement.as_bytes(),
        );
        let fresh = parser()
            .parse(edited.as_slice(), None)
            .expect("fresh malformed parse");

        assert_eq!(
            incremental.root_node().to_sexp(),
            fresh.root_node().to_sexp()
        );
        assert_eq!(
            incremental.root_node().has_error(),
            fresh.root_node().has_error(),
        );
        assert!(fresh.root_node().has_error());
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
fn deleting_control_flow_body_keeps_next_line_separate() {
    let cases = [
        (
            "if exist marker echo body\necho tail\n",
            " echo body",
            "if_statement",
        ),
        (
            "for %%i in (one) do echo body\necho tail\n",
            " echo body",
            "for_statement",
        ),
        (
            "if exist marker echo yes else echo no\necho tail\n",
            " echo no",
            "if_statement",
        ),
    ];

    for (before, needle, controller_kind) in cases {
        let start = before.find(needle).expect("body text");
        let (incremental, edited) = edited_tree(
            before.as_bytes(),
            start..start + needle.len(),
            b"",
        );
        let fresh = parser()
            .parse(edited.as_slice(), None)
            .expect("fresh malformed parse");

        assert_eq!(incremental.root_node().to_sexp(), fresh.root_node().to_sexp());
        assert!(incremental.root_node().has_error());
        assert!(fresh.root_node().has_error());

        for tree in [&incremental, &fresh] {
            let root = tree.root_node();
            assert_eq!(root.named_child_count(), 2);
            let controller = root.named_child(0).expect("controller");
            let tail = root.named_child(1).expect("tail command");
            assert_eq!(controller.kind(), controller_kind);
            assert!(controller.has_error());
            assert_eq!(controller.end_position().row, 0);
            assert_eq!(tail.kind(), "command");
            assert_eq!(tail.start_position().row, 1);
            assert!(!tail.has_error());
        }
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
        "IF /?\r\n",
        "FOR /?\r\n",
        "REM /?\r\n",
        "if exist x if /? >nul\r\n",
        "if exist x echo yes else REM /?<log\r\n",
        "for %%x in (a) do FoR /? 2>nul\r\n",
        "IF /? >nul\r\n",
        "FOR /? 2>nul\r\n",
        "REM /?<log\r\n",
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
