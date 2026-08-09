use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use std::sync::Arc;

use outlint_core::{
    linked_frontmatter_schema_path, load_schema_with_resources, parse_markdown,
    JsonSchemaResourceInput, LinkedJsonSchemaInput, MarkdownOptions, PreparedValidator,
    SourceLabel,
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
    let external = linked_frontmatter_schema_path(&schema_source).map(|declared| {
        let root_name = Path::new(&declared)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_else(|| panic!("invalid linked schema path `{declared}`"));
        let mut resources = fs::read_dir(directory)
            .expect("fixture directory is readable")
            .map(|entry| entry.expect("fixture entry is readable").path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "json")
            })
            .filter(|path| path.file_name().is_some_and(|name| name != "expected.json"))
            .collect::<Vec<_>>();
        resources.sort();
        LinkedJsonSchemaInput {
            root_uri: "https://outlint.invalid/root.json".into(),
            resources: resources
                .into_iter()
                .map(|path| {
                    let name = path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .expect("fixture JSON filename is UTF-8");
                    let uri = if name == root_name {
                        "https://outlint.invalid/root.json".into()
                    } else {
                        format!("https://outlint.invalid/{name}")
                    };
                    JsonSchemaResourceInput {
                        uri,
                        label: Some(SourceLabel(path.display().to_string())),
                        text: Arc::from(fs::read_to_string(&path).unwrap_or_else(|error| {
                            panic!("cannot read {}: {error}", path.display())
                        })),
                    }
                })
                .collect(),
        }
    });
    let loaded = match load_schema_with_resources(
        &schema_source,
        Some(SourceLabel(schema_path.display().to_string())),
        external,
    ) {
        Ok(loaded) => loaded,
        Err(invalid) => panic!(
            "invalid fixture schema {}: {invalid:#?}",
            schema_path.display()
        ),
    };
    let validator = PreparedValidator::new(&loaded.schema).expect("fixture validator prepares");
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
        let actual = validator
            .validate(&document)
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
