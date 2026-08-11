use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    fs,
    path::Path,
    process::Command,
};

use serde_json::Value;

/// One portable expected diagnostic: exactly `{ "id", "path" }` and nothing else.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Entry {
    id: String,
    path: String,
}

#[test]
fn shared_testdata_corpus_conforms_through_the_cli_binary() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata");
    assert!(
        root.is_dir(),
        "the conformance corpus directory {} is missing; run from a full outlint checkout",
        root.display()
    );
    // Only so failure messages name fixtures by a readable absolute path.
    let root = root.canonicalize().unwrap_or(root);
    let mut fixture_dirs = fs::read_dir(&root)
        .expect("the repository testdata directory must be readable")
        .map(|entry| entry.expect("every testdata directory entry must be readable"))
        .filter(|entry| {
            entry
                .file_type()
                .expect("every testdata entry type must be readable")
                .is_dir()
        })
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    fixture_dirs.sort();
    assert!(!fixture_dirs.is_empty(), "the conformance corpus is empty");

    for fixture_dir in fixture_dirs {
        run_fixture(&fixture_dir);
    }
}

fn run_fixture(directory: &Path) {
    let expected_path = directory.join("expected.json");
    let expected_source = fs::read_to_string(&expected_path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", expected_path.display()));
    let declared: BTreeMap<String, Vec<Value>> = serde_json::from_str(&expected_source)
        .unwrap_or_else(|error| panic!("invalid {}: {error}", expected_path.display()));
    let expected = declared
        .into_iter()
        .map(|(name, entries)| {
            let entries = entries
                .iter()
                .map(|entry| expected_entry(entry, &expected_path))
                .collect::<Vec<_>>();
            (name, entries)
        })
        .collect::<BTreeMap<_, _>>();

    let markdown_names = markdown_names(directory);
    assert!(
        !markdown_names.is_empty(),
        "fixture {} has no Markdown documents",
        directory.display()
    );
    let expected_names = expected.keys().cloned().collect::<BTreeSet<_>>();
    assert_eq!(
        markdown_names,
        expected_names,
        "expected.json must name every and only Markdown document in {}",
        directory.display()
    );

    let actual = run_cli(directory, &markdown_names);
    let actual_names = actual.keys().cloned().collect::<BTreeSet<_>>();
    assert_eq!(
        actual_names,
        markdown_names,
        "the CLI must report exactly one document result per Markdown document in {}",
        directory.display()
    );

    for (markdown_name, expected_diagnostics) in expected {
        let produced = actual
            .get(&markdown_name)
            .expect("every Markdown document has a reported result");
        compare_multisets(
            &directory.join(&markdown_name),
            &expected_diagnostics,
            produced,
        );
    }
}

/// Reads one expected diagnostic, enforcing the portable `{id, path}` shape.
fn expected_entry(entry: &Value, expected_path: &Path) -> Entry {
    let object = entry.as_object().unwrap_or_else(|| {
        panic!(
            "{}: expected diagnostic {entry} is not an object",
            expected_path.display()
        )
    });
    let field = |name: &str| {
        object
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                panic!(
                    "{}: expected diagnostic {entry} needs a string `{name}`",
                    expected_path.display()
                )
            })
            .to_owned()
    };
    let entries = Entry {
        id: field("id"),
        path: field("path"),
    };
    assert_eq!(
        object.len(),
        2,
        "{}: expected diagnostic {entry} must carry exactly `id` and `path`",
        expected_path.display()
    );
    entries
}

fn markdown_names(directory: &Path) -> BTreeSet<String> {
    fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("cannot enumerate {}: {error}", directory.display()))
        .map(|entry| {
            entry.unwrap_or_else(|error| {
                panic!("cannot read an entry in {}: {error}", directory.display())
            })
        })
        .filter(|entry| {
            entry
                .file_type()
                .unwrap_or_else(|error| {
                    panic!("cannot inspect {}: {error}", entry.path().display())
                })
                .is_file()
                && entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "md")
        })
        .map(|entry| {
            entry
                .file_name()
                .into_string()
                .unwrap_or_else(|name| panic!("fixture Markdown filename is not UTF-8: {name:?}"))
        })
        .collect()
}

/// Validates every fixture document with the built binary and returns the
/// portable `{id, path}` entries it reported, keyed by document path.
fn run_cli(directory: &Path, markdown_names: &BTreeSet<String>) -> BTreeMap<String, Vec<Entry>> {
    let mut arguments = vec!["check".to_owned()];
    arguments.extend(markdown_names.iter().cloned());
    arguments.extend([
        "--schema".to_owned(),
        "schema.outlint.yml".to_owned(),
        "--format".to_owned(),
        "json".to_owned(),
    ]);
    let output = Command::new(env!("CARGO_BIN_EXE_outlint"))
        .args(&arguments)
        .current_dir(directory)
        .output()
        .unwrap_or_else(|error| panic!("cannot run outlint in {}: {error}", directory.display()));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        matches!(output.status.code(), Some(0 | 1)),
        "outlint failed in {}: status {:?}, stderr:\n{stderr}",
        directory.display(),
        output.status.code()
    );
    assert!(
        stderr.is_empty(),
        "outlint wrote to stderr in {}:\n{stderr}",
        directory.display()
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "outlint did not emit one JSON document in {}: {error}",
            directory.display()
        )
    });
    let results = report["results"]
        .as_array()
        .unwrap_or_else(|| panic!("outlint report in {} has no results", directory.display()));

    let mut reported = BTreeMap::new();
    for result in results {
        assert_eq!(
            result["kind"],
            "document",
            "fixture {} produced a non-document result: {result}",
            directory.display()
        );
        let path = result["path"]
            .as_str()
            .unwrap_or_else(|| panic!("a result in {} has no path", directory.display()))
            .to_owned();
        let diagnostics = result["diagnostics"]
            .as_array()
            .unwrap_or_else(|| {
                panic!(
                    "result {path} in {} has no diagnostics array",
                    directory.display()
                )
            })
            .iter()
            .map(|diagnostic| entry_from_json(diagnostic, &path))
            .collect::<Vec<_>>();
        assert!(
            reported.insert(path.clone(), diagnostics).is_none(),
            "outlint reported {path} twice in {}",
            directory.display()
        );
    }
    reported
}

fn entry_from_json(diagnostic: &Value, document: &str) -> Entry {
    let id = diagnostic["id"]
        .as_str()
        .unwrap_or_else(|| panic!("a diagnostic for {document} has no id: {diagnostic}"))
        .to_owned();
    let segments = diagnostic["header_path"]
        .as_array()
        .unwrap_or_else(|| panic!("diagnostic {id} for {document} has no header_path"))
        .iter()
        .map(|segment| {
            segment
                .as_str()
                .unwrap_or_else(|| {
                    panic!("diagnostic {id} for {document} has a non-string header_path segment")
                })
                .to_owned()
        })
        .collect::<Vec<_>>();
    Entry {
        id,
        path: segments.join(" > "),
    }
}

/// Compares expected and produced entries as multisets, per the fixture
/// contract in `testdata/README.md`: same elements with the same
/// multiplicities, in any order.
fn compare_multisets(document: &Path, expected: &[Entry], produced: &[Entry]) {
    let mut balance: BTreeMap<&Entry, i64> = BTreeMap::new();
    for entry in expected {
        *balance.entry(entry).or_default() += 1;
    }
    for entry in produced {
        *balance.entry(entry).or_default() -= 1;
    }
    balance.retain(|_, count| *count != 0);
    if balance.is_empty() {
        return;
    }

    let mut message = format!("conformance mismatch for {}\n", document.display());
    let mut section = |title: &str, entries: Vec<(&Entry, i64)>| {
        if entries.is_empty() {
            return;
        }
        message.push_str(title);
        for (entry, count) in entries {
            let _ = write!(
                message,
                "\n  {count} x {{ \"id\": {:?}, \"path\": {:?} }}",
                entry.id, entry.path
            );
        }
        message.push('\n');
    };
    section(
        "expected but missing:",
        balance
            .iter()
            .filter(|(_, count)| **count > 0)
            .map(|(entry, count)| (*entry, *count))
            .collect(),
    );
    section(
        "produced but unexpected:",
        balance
            .iter()
            .filter(|(_, count)| **count < 0)
            .map(|(entry, count)| (*entry, -*count))
            .collect(),
    );
    let _ = write!(
        message,
        "expected {} diagnostic(s), produced {}\n\
         (order is not part of the contract; multiplicity is)",
        expected.len(),
        produced.len()
    );
    panic!("{message}");
}
