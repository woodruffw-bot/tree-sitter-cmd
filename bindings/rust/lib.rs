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
    fn test_injections_follow_for_f_quote_mode() {
        let source = concat!(
            "for /f %%a in ('ver') do echo %%a\r\n",
            "for /f %%a in ('echo normal') do echo %%a\r\n",
            "for /f %%a in ('echo don't stop') do echo %%a\r\n",
            "for /f %%a in ('echo 'one' 'two'') do echo %%a\r\n",
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
                "echo don't stop",
                "echo 'one' 'two'",
                "powershell -command \"ToString('yyyy-MM-dd')\"",
                "%1 -c \"sys.stdout.write('nt')\"",
                "\r\n  echo multiline\r\n",
                "echo options",
                "echo delimiter",
                "dir",
                "echo backquoted",
                "echo combined",
                "echo escaped options"
            ]
        );
    }

    #[test]
    fn test_for_f_single_quote_uses_final_apostrophe() {
        let source = concat!(
            "for /f %%a in ('echo don't stop') do echo %%a\r\n",
            "for /f %%a in ('echo 'one' 'two'') do echo %%a\r\n",
            "for /f %%a in ('echo %%!foo!\"x'y\"') do echo %%a\r\n",
            "for %%a in (it's) do echo %%a\r\n",
        );
        let tree = parse(source);
        assert!(!tree.root_node().has_error());

        let for_statement = tree.root_node().named_child(0).expect("FOR statement");
        let for_set = only_field(for_statement, "set");
        let quoted = for_set.named_child(0).expect("single-quoted source");
        assert_eq!(quoted.kind(), "for_f_command_source");
        assert_eq!(quoted.child_count(), 3);
        assert_eq!(
            &source[quoted.child(0).expect("opening quote").byte_range()],
            "'"
        );
        assert_eq!(
            &source[quoted.child(2).expect("closing quote").byte_range()],
            "'"
        );

        let query = Query::new(&language(), "(for_f_command_content) @content")
            .expect("FOR /F command-content query should compile");
        assert_eq!(
            capture_texts(source, &query, "content"),
            ["echo don't stop", "echo 'one' 'two'", "echo %%!foo!\"x'y\""]
        );
    }

    #[test]
    fn test_for_f_paired_expansions_keep_active_delimiters_opaque() {
        let source = concat!(
            "for /f %%a in ('echo %foo'bar%' data) do echo %%a\r\n",
            "for /f \"usebackq\" %%b in (`echo !foo`bar!` data) do echo %%b\r\n",
            "for /f %%c in ('echo %foo'bar%') do echo %%c\r\n",
            "for /f \"usebackq\" %%d in (`echo !foo`bar!`) do echo %%d\r\n",
            "for /f %%e in ('echo %foo'bar%\"x'y\"') do echo %%e\r\n",
            "for /f \"usebackq\" %%f in (`echo !foo`bar!\"x`y\"`) do echo %%f\r\n",
        );
        let tree = parse(source);
        assert!(
            !tree.root_node().has_error(),
            "{}",
            tree.root_node().to_sexp()
        );

        for index in 0..2 {
            let statement = tree.root_node().named_child(index).expect("neutral FOR /F");
            let set = only_field(statement, "set");
            assert_eq!(set.named_child_count(), 2);
            assert!(matches!(
                set.named_child(0).expect("neutral quoted item").kind(),
                "single_quote_string" | "backquote_string"
            ));
        }

        let query = Query::new(&language(), "(for_f_command_content) @content")
            .expect("FOR /F command-content query should compile");
        assert_eq!(
            capture_texts(source, &query, "content"),
            [
                "echo %foo'bar%",
                "echo !foo`bar!",
                "echo %foo'bar%\"x'y\"",
                "echo !foo`bar!\"x`y\"",
            ],
        );

        let percent = Query::new(&language(), "(variable) @expansion")
            .expect("variable query should compile");
        assert_eq!(
            capture_texts(source, &percent, "expansion"),
            ["%foo'bar%", "%foo'bar%", "%foo'bar%"],
        );
        let delayed = Query::new(&language(), "(delayed_variable) @expansion")
            .expect("delayed-variable query should compile");
        assert_eq!(
            capture_texts(source, &delayed, "expansion"),
            ["!foo`bar!", "!foo`bar!", "!foo`bar!"],
        );

        for malformed in [
            "for /f %%a in ('echo %foo'bar%) do echo %%a\r\n",
            "for /f \"usebackq\" %%a in (`echo !foo`bar!) do echo %%a\r\n",
        ] {
            assert!(parse(malformed).root_node().has_error(), "{malformed:?}");
        }
    }

    #[test]
    fn test_for_f_inner_double_quote_ends_at_newline() {
        let source =
            "for /f \"usebackq\" %%a in (`echo \"foo\r\nbar`) do echo %%a\r\n";
        let tree = parse(source);
        assert!(!tree.root_node().has_error());

        let query = Query::new(&language(), "(for_f_command_content) @content")
            .expect("FOR /F command-content query should compile");
        assert_eq!(
            capture_texts(source, &query, "content"),
            ["echo \"foo\r\nbar"],
        );
    }

    #[test]
    fn test_plain_for_apostrophes_and_non_command_for_f_items_stay_neutral() {
        let source = concat!(
            "for %%a in (foo 'bar' baz) do echo %%a\r\n",
            "for /f %%b in ('literal' data.txt) do echo %%b\r\n",
            "for /f %%c in ('literal'x) do echo %%c\r\n",
            "for /f \"usebackq\" %%d in ('one' 'two') do echo %%d\r\n",
            "for /f \"usebackq\" %%e in ('literal don't stop') do echo %%e\r\n",
        );
        let tree = parse(source);
        assert!(!tree.root_node().has_error());

        let plain = tree.root_node().named_child(0).expect("plain FOR");
        let plain_set = only_field(plain, "set");
        let plain_items = (0..plain_set.named_child_count())
            .map(|index| plain_set.named_child(index as u32).expect("plain set item"))
            .collect::<Vec<_>>();
        assert_eq!(
            plain_items
                .iter()
                .map(|node| node.kind())
                .collect::<Vec<_>>(),
            ["argument", "argument", "argument"]
        );
        assert_eq!(
            plain_items
                .iter()
                .map(|node| &source[node.byte_range()])
                .collect::<Vec<_>>(),
            ["foo", "'bar'", "baz"]
        );

        for index in [1, 2] {
            let statement = tree.root_node().named_child(index).expect("FOR /F");
            let for_set = only_field(statement, "set");
            assert_eq!(
                for_set.named_child(0).expect("neutral item").kind(),
                "single_quote_string",
            );
            assert_eq!(
                for_set.named_child(1).expect("following item").kind(),
                "argument",
            );
        }

        let usebackq = tree.root_node().named_child(3).expect("usebackq FOR /F");
        let usebackq_set = only_field(usebackq, "set");
        assert_eq!(usebackq_set.named_child_count(), 2);
        assert_eq!(
            usebackq_set.named_child(0).expect("first literal").kind(),
            "single_quote_string",
        );
        assert_eq!(
            usebackq_set.named_child(1).expect("second literal").kind(),
            "single_quote_string",
        );

        let contents = Query::new(&language(), "(single_quote_content) @content")
            .expect("single-quote-content query should compile");
        assert_eq!(
            capture_texts(source, &contents, "content"),
            ["literal", "literal", "one", "two", "literal don"]
        );

        let injections = Query::new(&language(), super::INJECTIONS_QUERY)
            .expect("injections query should compile");
        assert!(capture_texts(source, &injections, "injection.content").is_empty());
    }

    #[test]
    fn test_for_f_single_quote_requires_outer_metacharacter_escaping() {
        for source in [
            "for /f %%a in ('echo ^(hi^)') do echo %%a\r\n",
            "for /f %%a in ('echo one ^& echo two') do echo %%a\r\n",
            "for /f %%a in ('echo one ^| find \"one\"') do echo %%a\r\n",
            "for /f %%a in ('echo one ^< input') do echo %%a\r\n",
            "for /f %%a in ('echo one ^> output') do echo %%a\r\n",
            "for /f %%a in ('cmd /c \"echo one & echo two\"') do echo %%a\r\n",
            "for /f \"usebackq\" %%a in (`echo ^(hi^)`) do echo %%a\r\n",
            "for /f \"usebackq\" %%a in (`echo one ^& echo two`) do echo %%a\r\n",
            "for /f \"usebackq\" %%a in (`echo one ^| find \"one\"`) do echo %%a\r\n",
            "for /f \"usebackq\" %%a in (`echo one ^< input`) do echo %%a\r\n",
            "for /f \"usebackq\" %%a in (`echo one ^> output`) do echo %%a\r\n",
            "for /f \"usebackq\" %%a in (`cmd /c \"echo one & echo two\"`) do echo %%a\r\n",
        ] {
            let tree = parse(source);
            assert!(
                !tree.root_node().has_error(),
                "escaped FOR /F command source failed: {source:?}",
            );
        }

        for source in [
            "for /f %%a in ('echo (hi)') do echo %%a\r\n",
            "for /f %%a in ('echo one & echo two') do echo %%a\r\n",
            "for /f %%a in ('echo one | find \"one\"') do echo %%a\r\n",
            "for /f %%a in ('echo one < input') do echo %%a\r\n",
            "for /f %%a in ('echo one > output') do echo %%a\r\n",
            "for /f \"usebackq\" %%a in (`echo (hi)`) do echo %%a\r\n",
            "for /f \"usebackq\" %%a in (`echo one & echo two`) do echo %%a\r\n",
            "for /f \"usebackq\" %%a in (`echo one | find \"one\"`) do echo %%a\r\n",
            "for /f \"usebackq\" %%a in (`echo one < input`) do echo %%a\r\n",
            "for /f \"usebackq\" %%a in (`echo one > output`) do echo %%a\r\n",
            "for /f %%a in ('echo %foo&bar') do echo %%a\r\n",
            "for /f %%a in ('echo !foo|bar') do echo %%a\r\n",
            "for /f \"usebackq\" %%a in (`echo %foo>bar`) do echo %%a\r\n",
            "for /f \"usebackq\" %%a in (`echo !foo)bar`) do echo %%a\r\n",
        ] {
            let tree = parse(source);
            assert!(
                tree.root_node().has_error(),
                "unescaped FOR /F metacharacter parsed cleanly: {source:?}",
            );
        }
    }

    #[test]
    fn test_for_f_single_quote_requires_final_delimiter() {
        let source = "for /f %%a in ('echo unfinished) do echo %%a\r\n";
        let tree = parse(source);
        let root = tree.root_node();

        assert!(root.has_error());
        assert!(
            root.to_sexp().contains("(MISSING \"'\")"),
            "unfinished source must retain a real missing delimiter: {}",
            root.to_sexp(),
        );
    }

    #[test]
    fn test_for_f_mode_is_explicit_and_options_stay_opaque() {
        let source = concat!(
            "for /f \"delims=usebackq\" %%a in ('echo default') do echo %%a\r\n",
            "for /f \"nousebackq\" %%b in (`literal`) do echo %%b\r\n",
            "for /f \"tokens=* usebackq\" %%c in (`echo combined`) do echo %%c\r\n",
            "for /f usebackq^ tokens^=* %%d in (`echo escaped`) do echo %%d\r\n",
            "for /f \"USEBACKQ\" %%e in ('literal text') do echo %%e\r\n",
            "for /f %%f in ('literal' data.txt) do echo %%f\r\n",
            "for /f \"usebackq\" %%g in (`literal` data.txt) do echo %%g\r\n",
            "for /f %%h in ('literal'x) do echo %%h\r\n",
            "for /f \"usebackq\" %%i in (`literal`x) do echo %%i\r\n",
            "for /f %%j in ('literal'\"x\") do echo %%j\r\n",
            "for /f \"usebackq\" %%k in (`literal`\"x\") do echo %%k\r\n",
        );
        let tree = parse(source);
        assert!(!tree.root_node().has_error());

        let sources = Query::new(&language(), "(for_f_command_source) @source")
            .expect("FOR /F source query should compile");
        assert_eq!(
            capture_texts(source, &sources, "source"),
            ["'echo default'", "`echo combined`", "`echo escaped`"]
        );

        let options = Query::new(&language(), "(for_option argument: (argument) @option)")
            .expect("FOR option query should compile");
        assert_eq!(
            capture_texts(source, &options, "option"),
            [
                "\"delims=usebackq\"",
                "\"nousebackq\"",
                "\"tokens=* usebackq\"",
                "usebackq^ tokens^=*",
                "\"USEBACKQ\"",
                "\"usebackq\"",
                "\"usebackq\"",
                "\"usebackq\""
            ]
        );
    }

    #[test]
    fn test_incomplete_path_search_keeps_following_item_neutral() {
        let source = "for /f %%a in ('echo %~$E^\"foo' data) do echo %%a\r\n";
        let tree = parse(source);
        let statement = tree.root_node().named_child(0).expect("FOR statement");
        let set = only_field(statement, "set");

        assert!(!tree.root_node().has_error());
        assert_eq!(
            set.named_child(0).expect("quoted item").kind(),
            "single_quote_string",
        );
        let data = set.named_child(1).expect("following item");
        assert_eq!(data.kind(), "argument");
        assert_eq!(&source[data.byte_range()], "data");
        let sources = Query::new(&language(), "(for_f_command_source) @source")
            .expect("FOR source query");
        assert!(capture_texts(source, &sources, "source").is_empty());
    }

    #[test]
    fn test_inactive_for_f_delimiters_do_not_protect_outer_metacharacters() {
        for source in [
            "for /f %%a in (`literal ^) ^& ^| ^< ^>`) do echo %%a\r\n",
            "for /f %%a in (`\"literal ) & | < >\"`) do echo %%a\r\n",
            "for /f \"usebackq\" %%a in ('literal ^) ^& ^| ^< ^>') do echo %%a\r\n",
            "for /f \"usebackq\" %%a in ('\"literal ) & | < >\"') do echo %%a\r\n",
        ] {
            let tree = parse(source);
            assert!(
                !tree.root_node().has_error(),
                "protected inactive delimiter content failed: {source:?}",
            );
        }

        for source in [
            "for /f %%a in (`literal ) text`) do echo %%a\r\n",
            "for /f %%a in (`literal & text`) do echo %%a\r\n",
            "for /f %%a in (`literal | text`) do echo %%a\r\n",
            "for /f %%a in (`literal < text`) do echo %%a\r\n",
            "for /f %%a in (`literal > text`) do echo %%a\r\n",
            "for /f \"usebackq\" %%a in ('literal ) text') do echo %%a\r\n",
            "for /f \"usebackq\" %%a in ('literal & text') do echo %%a\r\n",
            "for /f \"usebackq\" %%a in ('literal | text') do echo %%a\r\n",
            "for /f \"usebackq\" %%a in ('literal < text') do echo %%a\r\n",
            "for /f \"usebackq\" %%a in ('literal > text') do echo %%a\r\n",
        ] {
            let tree = parse(source);
            assert!(
                tree.root_node().has_error(),
                "raw outer metacharacter parsed cleanly: {source:?}",
            );
        }
    }

    #[test]
    fn test_for_f_quote_modes_preserve_line_continuations() {
        let source = concat!(
            "for /f %%a in ('echo one ^\ntwo') do echo %%a\n",
            "for /f \"usebackq\" %%b in (`echo one ^\r\ntwo`) do echo %%b\r\n",
            "for /f %%c in (`literal ^\ntext`) do echo %%c\n",
            "for /f \"usebackq\" %%d in ('literal ^\r\ntext') do echo %%d\r\n",
        );
        let tree = parse(source);
        assert!(!tree.root_node().has_error());

        let commands = Query::new(&language(), "(for_f_command_content) @content")
            .expect("FOR /F command-content query should compile");
        assert_eq!(
            capture_texts(source, &commands, "content"),
            ["echo one ^\ntwo", "echo one ^\r\ntwo"]
        );

        let inactive = Query::new(
            &language(),
            "[(backquote_content) (single_quote_content)] @content",
        )
        .expect("inactive quote-content query should compile");
        assert_eq!(
            capture_texts(source, &inactive, "content"),
            ["literal ^\ntext", "literal ^\r\ntext"]
        );
    }

    #[test]
    fn test_for_f_backquote_requires_final_delimiter() {
        let source = "for /f \"usebackq\" %%a in (`echo unfinished) do echo %%a\r\n";
        let tree = parse(source);
        let root = tree.root_node();

        assert!(root.has_error());
        assert!(
            root.to_sexp().contains("(MISSING \"`\")"),
            "unfinished source must retain a real missing delimiter: {}",
            root.to_sexp(),
        );

        let query = Query::new(&language(), "(for_f_command_content) @content")
            .expect("FOR /F command-content query should compile");
        assert_eq!(
            capture_texts(source, &query, "content"),
            ["echo unfinished"]
        );

        let eof_source = "for /f \"usebackq\" %%a in (`echo unfinished";
        assert!(parse(eof_source).root_node().has_error());
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
