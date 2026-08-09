use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use outlint_core::{
    load_schema_with_label, parse_markdown, validate, MarkdownOptions, SourceLabel,
};
use serde::Deserialize;

#[derive(Debug, Deserialize, PartialEq, Eq)]
struct ExpectedDiagnostic {
    id: String,
    path: String,
}

#[test]
fn shared_testdata_corpus_conforms() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata");
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
    let schema_path = directory.join("schema.outlint.yml");
    let schema_source = fs::read_to_string(&schema_path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", schema_path.display()));
    let loaded = load_schema_with_label(
        &schema_source,
        Some(SourceLabel(schema_path.display().to_string())),
    )
    .unwrap_or_else(|invalid| {
        panic!(
            "invalid fixture schema {}: {invalid:#?}",
            schema_path.display()
        )
    });
    let expected_path = directory.join("expected.json");
    let expected_source = fs::read_to_string(&expected_path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", expected_path.display()));
    let expected: BTreeMap<String, Vec<ExpectedDiagnostic>> =
        serde_json::from_str(&expected_source)
            .unwrap_or_else(|error| panic!("invalid {}: {error}", expected_path.display()));
    let markdown_names = fs::read_dir(directory)
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
        .collect::<BTreeSet<_>>();
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

    for (markdown_name, expected_diagnostics) in expected {
        let markdown_path = directory.join(&markdown_name);
        let markdown = fs::read_to_string(&markdown_path)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", markdown_path.display()));
        let document = parse_markdown(
            &markdown,
            MarkdownOptions {
                strip_inline_markup: loaded.schema.options.strip_inline_markup,
            },
        );
        let actual = validate(&loaded.schema, &document)
            .into_iter()
            .map(|diagnostic| ExpectedDiagnostic {
                id: diagnostic.id.as_str().to_owned(),
                path: diagnostic.path.display(),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            actual,
            expected_diagnostics,
            "conformance mismatch for {}",
            markdown_path.display()
        );
    }
}
