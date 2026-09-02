use std::fmt::Write as _;

use tree_sitter::{Node, Parser};

// Lock the source-backed structures used by analyzers without coupling this
// contract to every keyword or plain-text fragment in the selected examples.
const FINGERPRINT_KINDS: &[&str] = &[
    "program",
    "argument",
    "call_statement",
    "command",
    "command_name",
    "comparison",
    "comparison_operator",
    "condition_keyword",
    "file_descriptor",
    "for_set",
    "for_statement",
    "goto_statement",
    "if_statement",
    "label",
    "label_name",
    "label_reference",
    "label_text",
    "loop_variable",
    "loop_variable_declaration",
    "redirect_dup",
    "redirect_dup_operator",
    "redirect_file",
    "redirect_operator",
    "set_assignment",
    "set_prompt",
    "set_quoted",
    "set_statement",
    "unary_condition",
    "variable_name",
];

fn parser() -> Parser {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_cmd::LANGUAGE.into())
        .expect("loading Cmd grammar");
    parser
}

fn escaped_source(source: &[u8]) -> String {
    let mut escaped = String::new();
    for &byte in source {
        match byte {
            b'\\' => escaped.push_str("\\\\"),
            b'"' => escaped.push_str("\\\""),
            b'\n' => escaped.push_str("\\n"),
            b'\r' => escaped.push_str("\\r"),
            b'\t' => escaped.push_str("\\t"),
            b' '..=b'~' => escaped.push(char::from(byte)),
            _ => write!(escaped, "\\x{byte:02x}").expect("writing to a string"),
        }
    }
    escaped
}

fn append_fingerprint(
    node: Node<'_>,
    source: &[u8],
    field: Option<&str>,
    depth: usize,
    fingerprint: &mut String,
) {
    let included = node.is_error()
        || node.is_missing()
        || (node.is_named() && FINGERPRINT_KINDS.contains(&node.kind()));
    let child_depth = if included { depth + 1 } else { depth };

    if included {
        for _ in 0..depth {
            fingerprint.push_str("  ");
        }
        if let Some(field) = field {
            write!(fingerprint, "{field}: ").expect("writing to a string");
        }
        fingerprint.push_str(node.kind());
        if node.is_error() {
            fingerprint.push_str(" [error]");
        }
        if node.is_missing() {
            fingerprint.push_str(" [missing]");
        }
        writeln!(
            fingerprint,
            " @{}..{} \"{}\"",
            node.start_byte(),
            node.end_byte(),
            escaped_source(&source[node.byte_range()]),
        )
        .expect("writing to a string");
    }

    for child_index in 0..node.child_count() {
        let child = node
            .child(child_index as u32)
            .expect("child index is in range");
        append_fingerprint(
            child,
            source,
            node.field_name_for_child(child_index as u32),
            child_depth,
            fingerprint,
        );
    }
}

fn fingerprint(source: &str) -> (bool, String) {
    let tree = parser()
        .parse(source, None)
        .expect("parser returned no tree");
    let root = tree.root_node();
    let mut fingerprint = String::new();
    append_fingerprint(root, source.as_bytes(), None, 0, &mut fingerprint);
    (root.has_error(), fingerprint)
}

fn assert_fingerprint(name: &str, source: &str, expect_error: bool, expected: &str) {
    let (has_error, actual) = fingerprint(source);
    assert_eq!(has_error, expect_error, "unexpected error state for {name}");
    assert_eq!(actual, expected, "CST fingerprint changed for {name}");
}

#[test]
fn selected_cst_contracts_have_exact_source_fingerprints() {
    assert_fingerprint(
        "quoted SET",
        "set \"name=value\"\n",
        false,
        r#"program @0..17 "set \"name=value\"\n"
  set_statement @0..16 "set \"name=value\""
    set_quoted @4..16 "\"name=value\""
      name: variable_name @5..9 "name"
      value: argument @10..15 "value"
"#,
    );

    assert_fingerprint(
        "quoted SET /P",
        "set /p \"answer=Prompt: \"\n",
        false,
        r#"program @0..25 "set /p \"answer=Prompt: \"\n"
  set_statement @0..24 "set /p \"answer=Prompt: \""
    set_prompt @4..24 "/p \"answer=Prompt: \""
      name: variable_name @8..14 "answer"
      prompt: argument @15..23 "Prompt: "
"#,
    );

    assert_fingerprint(
        "redirected SET name segments",
        "set PA>out TH=value\n",
        false,
        r#"program @0..20 "set PA>out TH=value\n"
  set_statement @0..19 "set PA>out TH=value"
    set_assignment @4..19 "PA>out TH=value"
      name: variable_name @4..6 "PA"
      redirect: redirect_file @6..10 ">out"
        operator: redirect_operator @6..7 ">"
        target: argument @7..10 "out"
      name: variable_name @11..13 "TH"
      value: argument @14..19 "value"
"#,
    );

    assert_fingerprint(
        "redirected SET /P name segments",
        "set /p na>out me=prompt\n",
        false,
        r#"program @0..24 "set /p na>out me=prompt\n"
  set_statement @0..23 "set /p na>out me=prompt"
    set_prompt @4..23 "/p na>out me=prompt"
      name: variable_name @7..9 "na"
      redirect: redirect_file @9..13 ">out"
        operator: redirect_operator @9..10 ">"
        target: argument @10..13 "out"
      name: variable_name @14..16 "me"
      prompt: argument @17..23 "prompt"
"#,
    );

    assert_fingerprint(
        "incomplete redirected SET /P",
        "set /p na>out\n",
        true,
        r#"program @0..14 "set /p na>out\n"
  set_statement @0..3 "set"
  ERROR [error] @4..13 "/p na>out"
    variable_name @7..9 "na"
    redirect: redirect_file @9..13 ">out"
      operator: redirect_operator @9..10 ">"
      target: argument @10..13 "out"
"#,
    );

    assert_fingerprint(
        "label definition and reference",
        ":dest: ignored\ngoto :dest: ignored\n",
        false,
        r#"program @0..35 ":dest: ignored\ngoto :dest: ignored\n"
  label @0..14 ":dest: ignored"
    name: label_name @1..5 "dest"
    label_text @5..14 ": ignored"
  goto_statement @15..34 "goto :dest: ignored"
    target: label_reference @20..34 ":dest: ignored"
      name: label_name @21..25 "dest"
      label_text @25..34 ": ignored"
"#,
    );

    assert_fingerprint(
        "redirection fields",
        "cmd 2>& 1\n",
        false,
        r#"program @0..10 "cmd 2>& 1\n"
  command @0..9 "cmd 2>& 1"
    name: command_name @0..3 "cmd"
    redirect: redirect_dup @4..9 "2>& 1"
      source: file_descriptor @4..5 "2"
      operator: redirect_dup_operator @5..7 ">&"
      target: file_descriptor @8..9 "1"
"#,
    );

    assert_fingerprint(
        "FOR declaration and reference",
        "for %%A in (x) do echo %%~fA\n",
        false,
        r#"program @0..29 "for %%A in (x) do echo %%~fA\n"
  for_statement @0..28 "for %%A in (x) do echo %%~fA"
    variable: loop_variable_declaration @4..7 "%%A"
    set: for_set @12..13 "x"
      argument @12..13 "x"
    body: command @18..28 "echo %%~fA"
      name: command_name @18..22 "echo"
      argument: argument @23..28 "%%~fA"
        loop_variable @23..28 "%%~fA"
"#,
    );

    assert_fingerprint(
        "IF and CALL fields",
        "if, (a)==; (b) call:sub arg\n",
        false,
        r#"program @0..28 "if, (a)==; (b) call:sub arg\n"
  if_statement @0..27 "if, (a)==; (b) call:sub arg"
    condition: comparison @4..14 "(a)==; (b)"
      left: argument @4..7 "(a)"
      operator: comparison_operator @7..9 "=="
      right: argument @11..14 "(b)"
    consequence: call_statement @15..27 "call:sub arg"
      target: argument @19..23 ":sub"
      argument: argument @24..27 "arg"
"#,
    );

}
