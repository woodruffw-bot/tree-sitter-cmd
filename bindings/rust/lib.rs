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
    fn test_statement_fields_follow_quiet_scope() {
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
        assert_eq!(consequence.kind(), "quiet_statement");
        assert_eq!(&source[consequence.byte_range()], "@echo yes");
        assert_eq!(only_field(consequence, "quiet").kind(), "quiet");
        assert_eq!(only_field(consequence, "body").kind(), "command");
        assert_eq!(alternative.kind(), "quiet_statement");
        assert_eq!(&source[alternative.byte_range()], "@@echo no");
        assert_eq!(only_field(alternative, "quiet").kind(), "quiet");
        let inner_alternative = only_field(alternative, "body");
        assert_eq!(inner_alternative.kind(), "quiet_statement");
        assert_eq!(only_field(inner_alternative, "body").kind(), "command");

        let for_statement = root.named_child(1).expect("for statement");
        let body = only_field(for_statement, "body");
        assert_eq!(body.kind(), "quiet_statement");
        assert_eq!(&source[body.byte_range()], "@echo %%i");
        assert_eq!(only_field(body, "quiet").kind(), "quiet");
        assert_eq!(only_field(body, "body").kind(), "command");

        let outer_quiet = root.named_child(2).expect("outer quiet statement");
        assert_eq!(outer_quiet.kind(), "quiet_statement");
        assert_eq!(only_field(outer_quiet, "quiet").kind(), "quiet");
        let and_list = only_field(outer_quiet, "body");
        assert_eq!(and_list.kind(), "and_list");
        let left = only_field(and_list, "left");
        let right = only_field(and_list, "right");
        assert_eq!(left.kind(), "command");
        assert_eq!(right.kind(), "quiet_statement");
        assert_eq!(&source[right.byte_range()], "@@echo right");
        let inner_right = only_field(right, "body");
        assert_eq!(inner_right.kind(), "quiet_statement");
        assert_eq!(only_field(inner_right, "body").kind(), "command");
    }

    #[test]
    fn test_if_operands_and_call_target_have_one_field() {
        let source = "if (a)==(b) call :sub arg\r\n";
        let tree = parse(source);
        assert!(!tree.root_node().has_error());

        let if_statement = tree.root_node().named_child(0).expect("if statement");
        let comparison = only_field(if_statement, "condition");
        let left = only_field(comparison, "left");
        let right = only_field(comparison, "right");
        assert_eq!(&source[left.byte_range()], "(a)");
        assert_eq!(&source[right.byte_range()], "(b)");

        let call = only_field(if_statement, "consequence");
        let target = only_field(call, "target");
        assert_eq!(&source[target.byte_range()], ":sub");
        assert_eq!(field_children(call, "argument").len(), 1);
    }

    #[test]
    fn test_for_quote_modes_do_not_claim_injection_semantics() {
        let source = concat!(
            "for /f %%a in ('ver') do echo %%a\r\n",
            "for /f %%b in ('echo unmatched) do echo %%b\r\n",
            "for /f \"usebackq\" %%c in (`dir`) do echo %%c\r\n",
            "for /f \"usebackq\" %%d in (`echo unmatched) do echo %%d\r\n",
        );
        let query = Query::new(&language(), super::INJECTIONS_QUERY)
            .expect("injections query should compile");

        assert!(query.capture_names().is_empty());
        assert!(!parse(source).root_node().has_error());
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
