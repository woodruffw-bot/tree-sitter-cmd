//! This crate provides Cmd language support for the [tree-sitter] parsing library.
//!
//! Typically, you will use the [`LANGUAGE`] constant to add this language to a
//! tree-sitter [`Parser`], and then use the parser to parse some code:
//!
//! ```
//! let code = "@echo off\r\nif exist input.txt echo found\r\n";
//! let mut parser = tree_sitter::Parser::new();
//! let language = tree_sitter_cmd::LANGUAGE;
//! parser
//!     .set_language(&language.into())
//!     .expect("Error loading Cmd parser");
//! let tree = parser.parse(code, None).unwrap();
//! assert!(!tree.root_node().has_error());
//! ```
//!
//! [`Parser`]: https://docs.rs/tree-sitter/0.26/tree_sitter/struct.Parser.html
//! [tree-sitter]: https://tree-sitter.github.io/

use tree_sitter_language::LanguageFn;

extern "C" {
    fn tree_sitter_cmd() -> *const ();
}

/// The tree-sitter [`LanguageFn`] for this grammar.
pub const LANGUAGE: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_cmd) };

/// The content of the [`node-types.json`] file for this grammar.
///
/// [`node-types.json`]: https://tree-sitter.github.io/tree-sitter/using-parsers/6-static-node-types
pub const NODE_TYPES: &str = include_str!("../../src/node-types.json");

/// The syntax-highlighting query for this language.
pub const HIGHLIGHTS_QUERY: &str = include_str!("../../queries/highlights.scm");

/// The language-injection query for this language.
pub const INJECTIONS_QUERY: &str = include_str!("../../queries/injections.scm");

#[cfg(test)]
mod tests {
    use tree_sitter::{Node, Query, QueryCursor, StreamingIterator};

    fn language() -> tree_sitter::Language {
        super::LANGUAGE.into()
    }

    fn parse(source: &str) -> tree_sitter::Tree {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&language())
            .expect("Error loading Cmd parser");
        parser.parse(source, None).expect("parser returned no tree")
    }

    fn capture_texts(source: &str, query: &Query, capture_name: &str) -> Vec<String> {
        let tree = parse(source);
        let capture_index = query
            .capture_index_for_name(capture_name)
            .expect("query is missing the requested capture");
        let mut cursor = QueryCursor::new();
        let mut captures = cursor.captures(query, tree.root_node(), source.as_bytes());
        let mut texts = Vec::new();

        while let Some((query_match, match_capture_index)) = captures.next() {
            let capture = query_match.captures[*match_capture_index];
            if capture.index == capture_index {
                texts.push(source[capture.node.byte_range()].to_owned());
            }
        }

        texts
    }

    fn field_children<'tree>(node: Node<'tree>, name: &str) -> Vec<Node<'tree>> {
        let mut cursor = node.walk();
        node.children_by_field_name(name, &mut cursor).collect()
    }

    fn only_field<'tree>(node: Node<'tree>, name: &str) -> Node<'tree> {
        let children = field_children(node, name);
        assert_eq!(
            children.len(),
            1,
            "{} must have exactly one {name:?} field, got {children:?}",
            node.kind(),
        );
        children[0]
    }

    #[test]
    fn test_can_load_grammar() {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&language())
            .expect("Error loading Cmd parser");
    }

    #[test]
    fn test_queries_compile() {
        let highlights = Query::new(&language(), super::HIGHLIGHTS_QUERY)
            .expect("highlights query should compile");
        Query::new(&language(), super::INJECTIONS_QUERY).expect("injections query should compile");

        for unsupported in ["keyword.operator", "label"] {
            assert!(
                !highlights.capture_names().contains(&unsupported),
                "unsupported highlight capture {unsupported:?}",
            );
        }
    }

    #[test]
    fn test_highlights_query_captures_common_syntax() {
        let source = concat!(
            ":start\r\n",
            "if not exist \"input.txt\" echo %PATH% & rem note\r\n",
            "set /a x=1 2>&1\r\n",
            "set /p answer=Prompt:\r\n",
            "goto start\r\n",
        );
        let query = Query::new(&language(), super::HIGHLIGHTS_QUERY)
            .expect("highlights query should compile");

        assert_eq!(
            capture_texts(source, &query, "keyword"),
            ["if", "not", "exist", "rem", "set", "/a", "set", "/p", "goto"]
        );
        assert_eq!(capture_texts(source, &query, "string"), ["\"input.txt\""]);
        assert_eq!(
            capture_texts(source, &query, "variable"),
            ["%PATH%", "answer"]
        );
        assert_eq!(
            capture_texts(source, &query, "constant"),
            ["start", "start"]
        );
        assert_eq!(capture_texts(source, &query, "number"), ["2", "1"]);
        assert_eq!(capture_texts(source, &query, "operator"), ["&", ">&", "="]);
    }

    #[test]
    fn test_redirected_rem_comment_highlight_excludes_redirect() {
        let source = ">nul rem note\r\n";
        let query = Query::new(&language(), super::HIGHLIGHTS_QUERY)
            .expect("highlights query should compile");

        let comments = capture_texts(source, &query, "comment");
        assert_eq!(
            comments
                .iter()
                .map(|text| text.trim())
                .collect::<Vec<_>>(),
            ["rem", "note"]
        );
        assert!(comments.iter().all(|text| !text.contains(">nul")));
    }

    #[test]
    fn test_statement_fields_have_one_concrete_node() {
        let source = concat!(
            "if exist x @echo yes else @@echo no\r\n",
            "for %%i in (x) do @echo %%i\r\n",
            "@echo left && @@echo right\r\n",
        );
        let tree = parse(source);
        assert!(!tree.root_node().has_error());
        let root = tree.root_node();

        let if_statement = root.named_child(0).expect("if statement");
        let consequence = only_field(if_statement, "consequence");
        let alternative = only_field(if_statement, "alternative");
        assert_eq!(consequence.kind(), "command");
        assert_eq!(alternative.kind(), "command");
        assert_eq!(field_children(consequence, "quiet").len(), 1);
        assert_eq!(field_children(alternative, "quiet").len(), 2);

        let for_statement = root.named_child(1).expect("for statement");
        let body = only_field(for_statement, "body");
        assert_eq!(body.kind(), "command");
        assert_eq!(field_children(body, "quiet").len(), 1);

        let and_list = root.named_child(2).expect("and list");
        let left = only_field(and_list, "left");
        let right = only_field(and_list, "right");
        assert_eq!(left.kind(), "command");
        assert_eq!(right.kind(), "command");
        assert_eq!(field_children(left, "quiet").len(), 1);
        assert_eq!(field_children(right, "quiet").len(), 2);
    }

    #[test]
    fn test_injections_follow_for_f_quote_mode() {
        let source = concat!(
            "for /f %%a in ('ver') do echo %%a\r\n",
            "for /f %%a in ('echo normal') do echo %%a\r\n",
            "for /f %%a in ('powershell -command \"ToString('yyyy-MM-dd')\"') do echo %%a\r\n",
            "for /f %%a in ('%1 -c \"sys.stdout.write('nt')\"') do echo %%a\r\n",
            "for /f %%a in ('\r\n  echo multiline\r\n') do echo %%a\r\n",
            "for /f %%a in (`not-a-command`) do echo %%a\r\n",
            "for /f \"tokens=*\" %%a in ('echo options') do echo %%a\r\n",
            "for /f \"delims=usebackq\" %%a in ('echo delimiter') do echo %%a\r\n",
            "for /f \"usebackq\" %%a in (`dir`) do echo %%a\r\n",
            "for /f \"usebackq\" %%a in (`echo backquoted`) do echo %%a\r\n",
            "for /f \"tokens=* usebackq\" %%a in (`echo combined`) do echo %%a\r\n",
            "for /f usebackq^ tokens^=* %%a in (`echo escaped options`) do echo %%a\r\n",
            "for /f usebackq^ tokens^=* %%a in ('not-a-command') do echo %%a\r\n",
            "for /f \"USEBACKQ\" %%a in ('not-a-command') do echo %%a\r\n",
            "for /r %%a in ('not-a-command') do echo %%a\r\n",
            "for /f \"usebackq\" %%a in (`echo unfinished",
        );
        let query = Query::new(&language(), super::INJECTIONS_QUERY)
            .expect("injections query should compile");

        assert_eq!(
            capture_texts(source, &query, "injection.content"),
            [
                "ver",
                "echo normal",
                "powershell -command \"ToString('yyyy-MM-dd')\"",
                "%1 -c \"sys.stdout.write('nt')\"",
                "\r\n  echo multiline\r\n",
                "echo options",
                "echo delimiter",
                "dir",
                "echo backquoted",
                "echo combined",
                "echo escaped options",
                "echo unfinished"
            ]
        );
    }

    #[test]
    fn test_unterminated_backquote_has_neutral_delimiter_free_content() {
        let source = "for /f \"usebackq\" %%a in (`echo unfinished";
        let query = Query::new(&language(), "(backquote_content) @content")
            .expect("backquote-content query should compile");

        assert_eq!(
            capture_texts(source, &query, "content"),
            ["echo unfinished"]
        );
    }

    #[test]
    fn test_for_rejects_illegal_mixed_switches() {
        for source in [
            "for /d /f %%a in (x) do echo %%a\r\n",
            "for /f /r %%a in (x) do echo %%a\r\n",
            "for /l /d %%a in (1,1,2) do echo %%a\r\n",
            "for /r /l %%a in (x) do echo %%a\r\n",
        ] {
            let tree = parse(source);
            assert!(
                tree.root_node().has_error(),
                "illegal mixed FOR switches parsed cleanly: {source:?}",
            );
        }
    }
}
