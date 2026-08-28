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
    use tree_sitter::{Node, Point, Query, QueryCursor, StreamingIterator};

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
    fn test_documented_help_forms_are_generic_commands() {
        let source = "IF /?\r\nFoR\t/?\r\nREM /?";
        let tree = parse(source);
        assert!(!tree.root_node().has_error());

        for (index, (expected_name, expected_argument)) in
            [("IF", "/?"), ("FoR", "/?"), ("REM", "/?")]
                .into_iter()
                .enumerate()
        {
            let command = tree
                .root_node()
                .named_child(index as u32)
                .expect("help command");
            assert_eq!(command.kind(), "command");
            let name = only_field(command, "name");
            let argument = only_field(command, "argument");
            assert_eq!(name.kind(), "command_name");
            assert_eq!(argument.kind(), "argument");
            assert_eq!(&source[name.byte_range()], expected_name);
            assert_eq!(&source[argument.byte_range()], expected_argument);
        }
    }

    #[test]
    fn test_help_forms_are_concrete_control_flow_bodies() {
        let source = concat!(
            "if exist x if /? >nul\r\n",
            "if exist x echo yes else REM /?<log\r\n",
            "for %%x in (a) do FoR /? 2>nul\r\n",
            "if exist x ^\r\nif /? >nul\r\n",
            "for %%x in (a) do ^\r\nFoR /? 2>nul\r\n",
        );
        let tree = parse(source);
        let root = tree.root_node();
        assert!(!root.has_error());

        let consequence = only_field(root.named_child(0).expect("first IF"), "consequence");
        let alternative = only_field(root.named_child(1).expect("second IF"), "alternative");
        let body = only_field(root.named_child(2).expect("FOR"), "body");
        let continued_consequence =
            only_field(root.named_child(3).expect("continued IF"), "consequence");
        let continued_body =
            only_field(root.named_child(4).expect("continued FOR"), "body");
        for (node, expected) in [
            (consequence, "if /? >nul"),
            (alternative, "REM /?<log"),
            (body, "FoR /? 2>nul"),
            (continued_consequence, "if /? >nul"),
            (continued_body, "FoR /? 2>nul"),
        ] {
            assert_eq!(node.kind(), "command");
            assert_eq!(&source[node.byte_range()], expected);
        }

        let adjacent = "if exist xif /?\r\n";
        let adjacent_tree = parse(adjacent);
        assert!(!adjacent_tree.root_node().has_error());
        let statement = adjacent_tree.root_node().named_child(0).expect("IF");
        let condition = only_field(statement, "condition");
        let operand = only_field(condition, "argument");
        assert_eq!(&adjacent[operand.byte_range()], "xif");
    }

    #[test]
    fn test_redirected_help_forms_preserve_source_ranges() {
        let source = "IF /? >nul\r\nFOR /? 2>nul\r\nREM /?<log\r\n";
        let tree = parse(source);
        let root = tree.root_node();
        assert!(!root.has_error());

        let expected = [
            (0..10, 0..2, 3..5, 6..10, Point::new(0, 6), ">nul"),
            (12..24, 12..15, 16..18, 19..24, Point::new(1, 7), "2>nul"),
            (26..36, 26..29, 30..32, 32..36, Point::new(2, 6), "<log"),
        ];
        for (
            index,
            (command_range, name_range, argument_range, redirect_range, point, redirect_text),
        ) in expected.into_iter().enumerate()
        {
            let command = root.named_child(index as u32).expect("help command");
            assert_eq!(command.kind(), "command");
            assert_eq!(command.byte_range(), command_range);
            assert_eq!(only_field(command, "name").byte_range(), name_range);
            let argument = only_field(command, "argument");
            assert_eq!(argument.byte_range(), argument_range);
            let redirect = only_field(command, "redirect");
            assert_eq!(redirect.byte_range(), redirect_range.clone());
            assert_eq!(redirect.start_position(), point);
            assert_eq!(&source[redirect_range], redirect_text);
        }
    }

    #[test]
    fn test_help_exception_does_not_accept_trailing_text() {
        for source in [
            "IF /? extra\r\n",
            "FOR /? extra\r\n",
            "IF /? 22>nul\r\n",
            "FOR /? 2 >nul\r\n",
            "IF /? >foo=extra\r\n",
            "FOR /? >foo,extra\r\n",
            "IF /? >foo;extra\r\n",
            "IF /? >%% extra\r\n",
            "IF /? >%%^ extra\r\n",
            "(IF /? >foo(bar)\r\n",
            "IF /? ^\r\n2>nul\r\n",
            "IF /? >foo ^\r\n2>bar\r\n",
            "IF /? >%FOO ^\r\nBAR%\r\n",
            "IF /? >!FOO ^\r\nBAR!\r\n",
            "IF /? >%~$FOO ^\r\nBAR:1\r\n",
            "IF /? 2>&%FOO%extra\r\n",
            "IF /? >%%~$A ^\r\nB:x\r\n",
            "IF /? >%1=%\r\n",
            "IF /? >&x\r\n",
            "FOR /? >&x\r\n",
            "IF /? > %=a\r\n",
            "FOR /? > %=a\r\n",
            "IF /?2>nul\r\n",
            "FOR /?2>nul\r\n",
            "IF /? > a(=b\r\n",
            "FOR /? > a(=b\r\n",
            "IF /? >!!a!=!\r\n",
            "FOR /? >!!a!=!\r\n",
            "IF /? > %1=b\r\n",
            "FOR /? > %1=b\r\n",
            "if exist x if /? extra\r\n",
            "if exist x echo yes else if /? extra\r\n",
            "for %%x in (a) do for /? extra\r\n",
        ] {
            assert!(parse(source).root_node().has_error(), "{source:?}");
        }

        for source in [
            "REM /? extra\r\n",
            "REM /? 2 >nul\r\n",
            "REM /? >&x\r\n",
            "REM /? > %=a\r\n",
            "REM /?2>nul\r\n",
            "REM /? > a(=b\r\n",
            "REM /? >!!a!=!\r\n",
            "REM /? > %1=b\r\n",
        ] {
            let rem = parse(source);
            assert!(!rem.root_node().has_error());
            assert_eq!(
                rem.root_node().named_child(0).expect("REM comment").kind(),
                "rem_comment",
            );
        }
    }

    #[test]
    fn test_help_duplication_target_reserves_final_loop_modifier() {
        for source in [
            "IF /? 2>&%%~d\r\n",
            "FOR /? 2>&%%~d\r\n",
            "REM /? 2>&%%~d\r\n",
        ] {
            let tree = parse(source);
            let command = tree.root_node().named_child(0).expect("help command");
            assert!(!tree.root_node().has_error(), "{source:?}");
            assert_eq!(command.kind(), "command", "{source:?}");
            let redirect = only_field(command, "redirect");
            assert_eq!(redirect.kind(), "redirect_dup");
            let target = only_field(redirect, "target");
            assert_eq!(target.kind(), "loop_variable");
            assert_eq!(&source[target.byte_range()], "%%~d");
        }
    }

    #[test]
    fn test_help_literal_sigil_targets_keep_following_boundaries() {
        for source in [
            "IF /? >%foo 2>bar\r\n",
            "IF /? >!foo && echo ok\r\n",
            "FOR /? >%foo|findstr x\r\n",
            "IF /? >foo=2>bar\r\n",
            "IF /? >%~$FOO BAR:1\r\n",
            "IF /? >%%a 2>bar\r\n",
            "IF /? >%%~dpA 2>bar\r\n",
            "IF /? ^\r\n>nul\r\n",
            "IF /? >x^%FOO BAR%\r\n",
            "IF /? >x^!FOO BAR!\r\n",
            "IF /? >\"foo ^\r\nbar\" extra\r\n",
            "IF /? >\"%VAR:\"=% foo\"\r\n",
            "IF /? >\"%VAR:\"=% foo\" ^\r\n>bar\r\n",
            "IF /? >foo= ; 2>bar\r\n",
            "IF /? >=^\r\n;foo\r\n",
            "IF /? >x^\r\n extra\r\n",
            "IF /? >%1,%\r\n",
            "IF /? >%~d1,%\r\n",
            "IF /? > foo;bar\r\n",
            "IF /? >%\" \";p\r\n",
            "IF /? >!\" \";p\r\n",
            "IF /? >foo=^\r\n;2>bar\r\n",
            "IF /? >%1x=%\r\n",
            "IF /? >=%1\"%1 \"\r\n",
            "IF /? >a^\r\n=%1\r\n",
            "IF /? >%1^\r\n=%1\r\n",
            "IF /? >\"a\"^\r\n=%1\r\n",
            "IF /? >!A!^\r\n=%1\r\n",
            "IF /? >%%a^\r\n=%1\r\n",
            "IF /? >a(^\r\n=%1\r\n",
            "IF /? >%^\r\n=%1\r\n",
            "IF /? > %1,b\r\n",
            "IF /? > %1;(=\r\n",
            "IF /? >a^\r\n ==%\r\n",
        ] {
            assert!(!parse(source).root_node().has_error(), "{source:?}");
        }
    }

    #[test]
    fn test_help_redirect_continuations_preserve_target_ranges() {
        for (source, expected_target) in [
            ("IF /? >=^\r\n;foo\r\n", "foo"),
            ("IF /? >x^\r\n extra\r\n", "x^\r\n extra"),
            ("IF /? >%1,%\r\n", "%1,%"),
            ("IF /? >%~d1,%\r\n", "%~d1,%"),
            ("IF /? > foo;bar\r\n", "foo;bar"),
            ("IF /? >%\" \";p\r\n", "%\" \";p"),
            ("IF /? >!\" \";p\r\n", "!\" \";p"),
            ("IF /? >%1x=%\r\n", "%1x=%"),
            ("IF /? >=%1\"%1 \"\r\n", "%1\"%1 \""),
            ("IF /? >a^\r\n=%1\r\n", "a^\r\n=%1"),
            ("IF /? >%1^\r\n=%1\r\n", "%1^\r\n=%1"),
            ("IF /? >\"a\"^\r\n=%1\r\n", "\"a\"^\r\n=%1"),
            ("IF /? >!A!^\r\n=%1\r\n", "!A!^\r\n=%1"),
            ("IF /? >%%a^\r\n=%1\r\n", "%%a^\r\n=%1"),
            ("IF /? >a(^\r\n=%1\r\n", "a(^\r\n=%1"),
            ("IF /? >%^\r\n=%1\r\n", "%^\r\n=%1"),
            ("IF /? > %1,b\r\n", "%1,b"),
            ("IF /? > %1;(=\r\n", "%1;("),
            ("IF /? >a^\r\n ==%\r\n", "a^\r\n ==%"),
        ] {
            let tree = parse(source);
            let command = tree.root_node().named_child(0).expect("help command");
            assert!(!tree.root_node().has_error(), "{source:?}");
            assert_eq!(command.kind(), "command");
            let target = only_field(only_field(command, "redirect"), "target");
            assert_eq!(target.kind(), "argument");
            assert_eq!(&source[target.byte_range()], expected_target);
        }
    }

    #[test]
    fn test_help_redirect_lookahead_matches_ordinary_target_tokenization() {
        let prefixes = [
            "a", "a(", "a)", "%1", "%~dp1", "%A%", "!A!", "%%a",
            "\"a\"", "%", "!", "(", ")",
        ];
        let joins = [
            "=", ",", ";", "^\r\n=", "^\r\n,", "^\r\n;", "^\r\n =", "^\r\n==",
            "^\r\n ==",
        ];
        let suffixes = [
            "b", "%1", "%A%", "!B!", "%%b", "\"b\"", "%", "!", "(b", ")b", "(=",
            ")=",
        ];

        let mut mismatches = Vec::new();
        for spacing in ["", " "] {
            for prefix in prefixes {
                for join in joins {
                    for suffix in suffixes {
                        let target = format!("{prefix}{join}{suffix}");
                        let ordinary_source = format!("echo /? >{spacing}{target}\r\n");
                        let ordinary = parse(&ordinary_source);
                        let ordinary_command = ordinary
                            .root_node()
                            .named_child(0)
                            .expect("ordinary command");
                        let ordinary_redirects = field_children(ordinary_command, "redirect");
                        let ordinary_is_redirect_only = !ordinary.root_node().has_error()
                            && field_children(ordinary_command, "argument").len() == 1
                            && ordinary_redirects.len() == 1
                            && ordinary_command.end_byte() == ordinary_source.len() - 2;

                        let help_source = format!("IF /? >{spacing}{target}\r\n");
                        let help = parse(&help_source);
                        if !help.root_node().has_error() != ordinary_is_redirect_only {
                            mismatches.push(format!(
                                "{target:?} with spacing {spacing:?}: ordinary={}, help={}",
                                ordinary.root_node().to_sexp(),
                                help.root_node().to_sexp(),
                            ));
                        } else if ordinary_is_redirect_only {
                            let help_command =
                                help.root_node().named_child(0).expect("help command");
                            let help_redirect = only_field(help_command, "redirect");
                            let ordinary_target = only_field(ordinary_redirects[0], "target");
                            assert_eq!(
                                &help_source[only_field(help_redirect, "target").byte_range()],
                                &ordinary_source[ordinary_target.byte_range()],
                            );
                        }
                    }
                }
            }
        }
        assert!(
            mismatches.is_empty(),
            "help lookahead disagrees with ordinary target tokenization:\n{}",
            mismatches.join("\n"),
        );
    }

    #[test]
    fn test_help_separator_prefixed_parenthesis_target_inside_block() {
        let source = "(IF /? >=()\r\n";
        let tree = parse(source);
        assert!(!tree.root_node().has_error());
        let block = tree.root_node().named_child(0).expect("block");
        assert_eq!(block.kind(), "block");
        let command = block.named_child(0).expect("help command");
        assert_eq!(command.kind(), "command");
        let redirect = only_field(command, "redirect");
        assert_eq!(&source[redirect.byte_range()], ">=(");
        let target = only_field(redirect, "target");
        assert_eq!(&source[target.byte_range()], "(");
    }

    #[test]
    fn test_help_separator_continuation_preserves_following_descriptor() {
        let source = "IF /? >foo=^\r\n;2>bar\r\n";
        let tree = parse(source);
        let command = tree.root_node().named_child(0).expect("help command");
        assert!(!tree.root_node().has_error());

        let redirects = field_children(command, "redirect");
        assert_eq!(redirects.len(), 2);
        let source_descriptor = only_field(redirects[1], "source");
        assert_eq!(source_descriptor.kind(), "file_descriptor");
        assert_eq!(&source[source_descriptor.byte_range()], "2");
    }

    #[test]
    fn test_help_long_continuation_chain_stays_linear_and_clean() {
        let mut source = String::from("IF /? >out");
        for _ in 0..2048 {
            source.push_str("^\r\nx");
        }
        source.push_str("\r\n");

        let tree = parse(&source);
        assert!(!tree.root_node().has_error());
        let command = tree.root_node().named_child(0).expect("help command");
        let target = only_field(only_field(command, "redirect"), "target");
        assert_eq!(&source[target.byte_range().start..][..3], "out");
        assert_eq!(target.end_byte(), source.len() - 2);
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
