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
    use tree_sitter::{Query, QueryCursor, StreamingIterator};

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

    #[test]
    fn test_can_load_grammar() {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&language())
            .expect("Error loading Cmd parser");
    }

    #[test]
    fn test_queries_compile() {
        Query::new(&language(), super::HIGHLIGHTS_QUERY).expect("highlights query should compile");
        Query::new(&language(), super::INJECTIONS_QUERY).expect("injections query should compile");
    }

    #[test]
    fn test_highlights_query_captures_common_syntax() {
        let source = "if not exist \"input.txt\" echo %PATH% & rem note\r\n";
        let query = Query::new(&language(), super::HIGHLIGHTS_QUERY)
            .expect("highlights query should compile");

        assert_eq!(capture_texts(source, &query, "keyword"), ["if", "rem"]);
        assert_eq!(
            capture_texts(source, &query, "keyword.operator"),
            ["not", "exist"]
        );
        assert_eq!(capture_texts(source, &query, "string"), ["\"input.txt\""]);
        assert_eq!(capture_texts(source, &query, "variable"), ["%PATH%"]);
        assert_eq!(capture_texts(source, &query, "operator"), ["&"]);
    }

    #[test]
    fn test_injections_follow_for_f_quote_mode() {
        let source = concat!(
            "for /f %%a in ('ver') do echo %%a\r\n",
            "for /f %%a in ('echo normal') do echo %%a\r\n",
            "for /f %%a in (`not-a-command`) do echo %%a\r\n",
            "for /f \"tokens=*\" %%a in ('echo options') do echo %%a\r\n",
            "for /f \"delims=usebackq\" %%a in ('echo delimiter') do echo %%a\r\n",
            "for /f \"usebackq\" %%a in (`dir`) do echo %%a\r\n",
            "for /f \"usebackq\" %%a in (`echo backquoted`) do echo %%a\r\n",
            "for /f \"tokens=* usebackq\" %%a in (`echo combined`) do echo %%a\r\n",
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
                "echo options",
                "echo delimiter",
                "dir",
                "echo backquoted",
                "echo combined",
                "echo unfinished"
            ]
        );
    }

    #[test]
    fn test_unterminated_backquote_has_delimiter_free_content() {
        let source = "for /f \"usebackq\" %%a in (`echo unfinished";
        let query = Query::new(&language(), "(command_content) @content")
            .expect("command-content query should compile");

        assert_eq!(
            capture_texts(source, &query, "content"),
            ["echo unfinished"]
        );
    }
}
