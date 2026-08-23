use std::collections::BTreeSet;
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
            seen.insert(name),
            "{}:{}: duplicate fixture {name}",
            manifest.display(),
            index + 1,
        );
        names.push(name.to_owned());
    }

    names
}

fn fixture_files(directory: &Path) -> BTreeSet<String> {
    fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("reading {}: {error}", directory.display()))
        .map(|entry| entry.expect("reading fixture directory entry").path())
        .filter(|path| {
            matches!(
                path.extension().and_then(|extension| extension.to_str()),
                Some("bat" | "cmd")
            )
        })
        .map(|path| {
            path.file_name()
                .expect("fixture file name")
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}

#[test]
fn recovery_node_diagnostics_include_locations() {
    let cases: &[(&[u8], &str)] = &[
        (b"echo before >\necho after\n", "1:13 (bytes 12..13): ERROR"),
        (b"(\necho before\n", "3:1 (bytes 14..14): MISSING )"),
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
fn real_world_fixtures_parse_without_recovery() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let real_world = root.join("test/real-world");
    let fixtures = real_world.join("fixtures");
    let names = manifest_entries(&real_world.join("sources.tsv"));
    let listed: BTreeSet<_> = names.iter().cloned().collect();

    assert_eq!(
        listed,
        fixture_files(&fixtures),
        "sources.tsv and the fixture directory must list the same scripts",
    );

    let mut failures = Vec::new();
    for name in names {
        let path = fixtures.join(&name);
        let source = match fs::read(&path) {
            Ok(source) => source,
            Err(error) => {
                failures.push(format!("{name}: read failed: {error}"));
                continue;
            }
        };
        let Some(tree) = parser().parse(source.as_slice(), None) else {
            failures.push(format!("{name}: Parser::parse returned None"));
            continue;
        };

        let mut problems = Vec::new();
        collect_problems(tree.root_node(), &source, &mut problems);
        for problem in problems {
            failures.push(format!("{name}:{problem}"));
        }
    }

    assert!(
        failures.is_empty(),
        "real-world parse failures:\n{}",
        failures.join("\n"),
    );
}
