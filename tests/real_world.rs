use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use tree_sitter::{Node, Parser};

fn parser() -> Parser {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_cmd::LANGUAGE.into())
        .expect("loading Cmd grammar");
    parser
}

fn escaped_line(source: &[u8], byte: usize) -> String {
    let byte = byte.min(source.len());
    let start = source[..byte]
        .iter()
        .rposition(|&ch| ch == b'\n')
        .map_or(0, |index| index + 1);
    let end = source[byte..]
        .iter()
        .position(|&ch| ch == b'\n')
        .map_or(source.len(), |index| byte + index);

    String::from_utf8_lossy(&source[start..end])
        .chars()
        .flat_map(char::escape_default)
        .collect()
}

fn collect_problems(node: Node<'_>, source: &[u8], problems: &mut Vec<String>) {
    if node.is_error() || node.is_missing() {
        let point = node.start_position();
        let description = if node.is_missing() {
            format!("MISSING {}", node.kind())
        } else {
            "ERROR".to_owned()
        };
        problems.push(format!(
            "{}:{} (bytes {}..{}): {}: {}",
            point.row + 1,
            point.column + 1,
            node.start_byte(),
            node.end_byte(),
            description,
            escaped_line(source, node.start_byte()),
        ));
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_problems(child, source, problems);
    }
}

fn manifest_entries(manifest: &Path) -> Vec<String> {
    let text = fs::read_to_string(manifest)
        .unwrap_or_else(|error| panic!("reading {}: {error}", manifest.display()));
    let mut names = Vec::new();
    let mut seen = BTreeSet::new();

    for (index, line) in text.lines().enumerate() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let (name, source) = line.split_once('\t').unwrap_or_else(|| {
            panic!(
                "{}:{}: expected <filename>\\t<source-url>",
                manifest.display(),
                index + 1,
            )
        });
        assert!(
            !name.is_empty() && !source.is_empty() && !source.contains('\t'),
            "{}:{}: malformed manifest row",
            manifest.display(),
            index + 1,
        );
        assert!(
            Path::new(name).file_name().and_then(|name| name.to_str()) == Some(name),
            "{}:{}: fixture must be a file name: {name}",
            manifest.display(),
            index + 1,
        );
        assert!(
            is_script(Path::new(name)),
            "{}:{}: fixture must have a .bat or .cmd extension: {name}",
            manifest.display(),
            index + 1,
        );
        assert!(
            seen.insert(name),
            "{}:{}: duplicate fixture {name}",
            manifest.display(),
            index + 1,
        );
        names.push(name.to_owned());
    }

    assert!(
        !names.is_empty(),
        "{} must list at least one fixture",
        manifest.display(),
    );
    names
}

fn is_script(path: &Path) -> bool {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some(extension) => {
            extension.eq_ignore_ascii_case("bat") || extension.eq_ignore_ascii_case("cmd")
        }
        None => false,
    }
}

fn fixture_files(directory: &Path) -> BTreeSet<String> {
    fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("reading {}: {error}", directory.display()))
        .map(|entry| entry.expect("reading fixture directory entry").path())
        .filter(|path| path.is_file() && is_script(path))
        .map(|path| {
            path.file_name()
                .expect("fixture file name")
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}

type StructuralContracts = BTreeMap<String, BTreeMap<String, usize>>;

fn structural_contracts(path: &Path) -> StructuralContracts {
    let text = fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()));
    let mut contracts = StructuralContracts::new();

    for (index, line) in text.lines().enumerate() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let mut fields = line.split('\t');
        let fixture = fields.next().expect("split always has one field");
        let kind = fields.next().unwrap_or_else(|| {
            panic!(
                "{}:{}: expected <filename>\\t<node-kind>\\t<minimum-count>",
                path.display(),
                index + 1,
            )
        });
        let minimum = fields.next().unwrap_or_else(|| {
            panic!(
                "{}:{}: expected <filename>\\t<node-kind>\\t<minimum-count>",
                path.display(),
                index + 1,
            )
        });
        assert!(
            fields.next().is_none() && !fixture.is_empty() && !kind.is_empty(),
            "{}:{}: malformed structural contract",
            path.display(),
            index + 1,
        );
        let minimum = minimum.parse::<usize>().unwrap_or_else(|error| {
            panic!(
                "{}:{}: invalid minimum count {minimum:?}: {error}",
                path.display(),
                index + 1,
            )
        });
        assert!(
            minimum > 0,
            "{}:{}: minimum count must be greater than zero",
            path.display(),
            index + 1,
        );

        let previous = contracts
            .entry(fixture.to_owned())
            .or_default()
            .insert(kind.to_owned(), minimum);
        assert!(
            previous.is_none(),
            "{}:{}: duplicate contract for {fixture} node kind {kind}",
            path.display(),
            index + 1,
        );
    }

    assert!(
        !contracts.is_empty(),
        "{} must list at least one structural contract",
        path.display(),
    );
    contracts
}

fn collect_node_kinds(node: Node<'_>, counts: &mut BTreeMap<String, usize>) {
    *counts.entry(node.kind().to_owned()).or_default() += 1;

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_node_kinds(child, counts);
    }
}

fn assert_no_empty_command_names(node: Node<'_>) {
    if node.kind() == "command_name" {
        assert_ne!(
            node.start_byte(),
            node.end_byte(),
            "recovery synthesized an empty command_name",
        );
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        assert_no_empty_command_names(child);
    }
}

#[test]
fn script_extensions_are_case_insensitive() {
    assert!(is_script(Path::new("fixture.bat")));
    assert!(is_script(Path::new("fixture.BAT")));
    assert!(is_script(Path::new("fixture.Cmd")));
    assert!(!is_script(Path::new("fixture.bat.LICENSE")));
}

#[test]
fn recovery_node_diagnostics_include_locations() {
    let cases: &[(&[u8], &str)] = &[
        (
            b"echo before >\necho after\n",
            "1:14 (bytes 13..13): MISSING text",
        ),
        (b"(\necho before\n", "3:1 (bytes 14..14): MISSING )"),
        (b"()\necho after\n", "1:2 (bytes 1..1): MISSING _cmd_text"),
        (
            b"if exist marker\n",
            "1:16 (bytes 15..15): MISSING command",
        ),
        (
            b"for %%i in (one) do\n",
            "1:20 (bytes 19..19): MISSING command",
        ),
    ];

    for &(source, expected) in cases {
        let tree = parser().parse(source, None).expect("recovery parse");
        let mut problems = Vec::new();
        collect_problems(tree.root_node(), source, &mut problems);
        assert!(
            problems.iter().any(|problem| problem.contains(expected)),
            "missing {expected:?} in {problems:?}",
        );
    }
}

#[test]
fn malformed_control_flow_bodies_stay_local() {
    let next_line_cases: &[(&[u8], &str, &str)] = &[
        (
            b"if exist marker\necho tail\n",
            "if_statement",
            "command",
        ),
        (
            b"for %%i in (one) do\necho tail\n",
            "for_statement",
            "command",
        ),
        (
            b"if exist marker echo yes else\necho tail\n",
            "if_statement",
            "command",
        ),
        (
            b"if exist marker\ngoto tail\n",
            "if_statement",
            "goto_statement",
        ),
        (
            b"for %%i in (one) do\n(echo tail)\n",
            "for_statement",
            "block",
        ),
    ];

    for &(source, controller_kind, tail_kind) in next_line_cases {
        let tree = parser().parse(source, None).expect("malformed parse");
        let root = tree.root_node();
        assert!(root.has_error(), "missing body parsed without recovery");
        assert_eq!(root.named_child_count(), 2);

        let controller = root.named_child(0).expect("controller");
        let tail = root.named_child(1).expect("tail command");
        assert_eq!(controller.kind(), controller_kind);
        assert!(controller.has_error(), "controller lost missing-body state");
        assert_eq!(controller.end_position().row, 0);
        assert_eq!(tail.kind(), tail_kind);
        assert_eq!(tail.start_position().row, 1);
        assert!(!tail.has_error(), "next-line command entered recovery");
        assert_no_empty_command_names(root);
    }

    for source in [
        b"if exist marker".as_slice(),
        b"for %%i in (one) do".as_slice(),
        b"if exist marker echo yes else".as_slice(),
    ] {
        let tree = parser().parse(source, None).expect("malformed EOF parse");
        let root = tree.root_node();
        assert!(root.has_error(), "missing EOF body parsed cleanly");
        assert!(
            root.named_child(0).expect("controller").has_error(),
            "controller lost EOF missing-body state",
        );
        assert_no_empty_command_names(root);
    }
}

#[test]
fn malformed_redirections_retain_recovery_state() {
    let cases: &[&[u8]] = &[
        b"echo >\n",
        b"cmd 2>&\n",
        b"cmd 2>&x\n",
        b"cmd 2>&^\n1\n",
    ];

    for &source in cases {
        let tree = parser().parse(source, None).expect("malformed parse");
        assert!(
            tree.root_node().has_error(),
            "malformed redirection parsed without recovery: {}",
            tree.root_node().to_sexp(),
        );
    }
}

#[test]
fn required_goto_and_call_targets_retain_recovery_state() {
    let cases: &[(&[u8], &str)] = &[
        (b"goto\necho tail\n", "ERROR"),
        (b"call\necho tail\n", "ERROR"),
        (b"goto >log\necho tail\n", "goto_statement"),
        (b"call <input\necho tail\n", "call_statement"),
    ];

    for &(source, controller_kind) in cases {
        let tree = parser().parse(source, None).expect("malformed parse");
        let root = tree.root_node();
        assert!(root.has_error(), "missing target parsed without recovery");
        assert_eq!(root.named_child_count(), 2);

        let controller = root.named_child(0).expect("controller");
        let tail = root.named_child(1).expect("tail command");
        assert_eq!(controller.kind(), controller_kind);
        assert!(controller.has_error(), "controller lost missing-target state");
        assert_eq!(controller.end_position().row, 0);
        assert_eq!(tail.kind(), "command");
        assert_eq!(tail.start_position().row, 1);
        assert!(!tail.has_error(), "next-line command entered recovery");
    }
}

#[test]
fn real_world_fixtures_parse_without_recovery() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let real_world = root.join("test/real-world");
    let fixtures = real_world.join("fixtures");
    let names = manifest_entries(&real_world.join("sources.tsv"));
    let listed: BTreeSet<_> = names.iter().cloned().collect();
    let contracts = structural_contracts(&real_world.join("contracts.tsv"));
    let fixture_files = fixture_files(&fixtures);

    assert!(!fixture_files.is_empty(), "fixture directory has no scripts");
    assert_eq!(
        listed,
        fixture_files,
        "sources.tsv and the fixture directory must list the same scripts",
    );
    for name in &names {
        let license = fixtures.join(format!("{name}.LICENSE"));
        assert!(
            license.is_file(),
            "fixture {name} is missing its LICENSE sibling: {}",
            license.display(),
        );
    }
    for fixture in contracts.keys() {
        assert!(
            listed.contains(fixture),
            "contracts.tsv refers to unlisted fixture {fixture}",
        );
    }

    let mut failures = Vec::new();
    let mut parsed = 0;
    for name in names {
        let path = fixtures.join(&name);
        let source = match fs::read(&path) {
            Ok(source) => source,
            Err(error) => {
                failures.push(format!("{name}: read failed: {error}"));
                continue;
            }
        };
        if let Err(error) = std::str::from_utf8(&source) {
            failures.push(format!("{name}: fixture is not UTF-8: {error}"));
            continue;
        }
        let Some(tree) = parser().parse(source.as_slice(), None) else {
            failures.push(format!("{name}: Parser::parse returned None"));
            continue;
        };
        parsed += 1;

        let mut problems = Vec::new();
        collect_problems(tree.root_node(), &source, &mut problems);
        for problem in problems {
            failures.push(format!("{name}:{problem}"));
        }

        if let Some(contract) = contracts.get(&name) {
            let mut node_kinds = BTreeMap::new();
            collect_node_kinds(tree.root_node(), &mut node_kinds);
            for (kind, minimum) in contract {
                let actual = node_kinds.get(kind).copied().unwrap_or(0);
                if actual < *minimum {
                    failures.push(format!(
                        "{name}: expected at least {minimum} {kind} nodes, found {actual}",
                    ));
                }
            }
        }
    }

    assert!(parsed > 0, "no real-world fixtures were parsed");
    assert!(
        failures.is_empty(),
        "real-world parse failures:\n{}",
        failures.join("\n"),
    );
}
