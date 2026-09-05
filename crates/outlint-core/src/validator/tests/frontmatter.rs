use crate::validator::{validate, DiagnosticId, DiagnosticTarget, PreparedValidator};
use crate::{
    load_schema, parse_markdown, FrontmatterPolicy, FrontmatterSchema, MarkdownOptions, SchemaNode,
};

#[test]
fn validates_required_frontmatter_against_json_schema() {
    let mut schema = load_schema("version: 2\nfrontmatter: { required: true }\nsections: []\n")
        .expect("test schema is valid")
        .schema;
    let object = serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object",
        "required": ["status"],
        "properties": { "status": { "enum": ["draft", "final"] } }
    });
    schema.frontmatter = FrontmatterPolicy::Required {
        schema: Some(FrontmatterSchema {
            root_uri: "https://outlint.invalid/root.json".into(),
            root: object,
            resources: std::collections::BTreeMap::new(),
        }),
    };

    let absent = parse_markdown("# Title\n", MarkdownOptions::default());
    assert_eq!(
        validate(&schema, &absent).expect("schema prepares")[0].id,
        DiagnosticId::MissingFrontmatter
    );

    let invalid = parse_markdown(
        "---\nstatus: proposed\n---\n# Title\n",
        MarkdownOptions::default(),
    );
    let diagnostics = validate(&schema, &invalid).expect("schema prepares");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].id, DiagnosticId::FrontmatterSchema);
    let DiagnosticTarget::Frontmatter { block: Some(block) } = &diagnostics[0].target else {
        panic!("a frontmatter schema diagnostic targets a present block");
    };
    assert_eq!(block.json_pointer.as_deref(), Some("/status"));
    assert_eq!(
        (block.line_range.start_line, block.line_range.end_line),
        (1, 3)
    );
    assert_eq!(
        diagnostics[0].schema_node,
        Some(SchemaNode::FrontmatterSchemaDocument)
    );

    let valid = parse_markdown(
        "---\nstatus: final\n---\n# Title\n",
        MarkdownOptions::default(),
    );
    assert!(validate(&schema, &valid)
        .expect("schema prepares")
        .is_empty());
}

#[test]
fn frontmatter_schema_messages_quote_document_number_spellings() {
    let mut schema = load_schema("version: 2\ntitle: null\nsections: []\n")
        .expect("test schema is valid")
        .schema;
    schema.frontmatter = FrontmatterPolicy::Optional {
        schema: Some(FrontmatterSchema {
            root_uri: "https://outlint.invalid/root.json".into(),
            root: serde_json::json!({
                "type": "object",
                "properties": {
                    "whole": { "maximum": 1 },
                    "fraction": { "maximum": 1 },
                    "lower_exponent": { "maximum": 1 },
                    "upper_exponent": { "maximum": 1 }
                }
            }),
            resources: std::collections::BTreeMap::new(),
        }),
    };
    let document = parse_markdown(
        "---\nwhole: 100.0\nfraction: 1.5\nlower_exponent: 1e2\nupper_exponent: 1E2\n---\n",
        MarkdownOptions::default(),
    );
    let messages = validate(&schema, &document)
        .expect("schema prepares")
        .into_iter()
        .map(|diagnostic| diagnostic.message)
        .collect::<Vec<_>>();

    assert_eq!(
        messages,
        [
            "1.5 is greater than the maximum of 1",
            "1e2 is greater than the maximum of 1",
            "1E2 is greater than the maximum of 1",
            "100.0 is greater than the maximum of 1",
        ]
    );
}

#[test]
fn manually_constructed_frontmatter_schema_denies_remote_retrieval() {
    let remote_uri = "https://example.invalid/frontmatter.schema.json";
    let mut schema = load_schema("version: 2\nsections: []\n")
        .expect("test schema is valid")
        .schema;
    schema.frontmatter = FrontmatterPolicy::Optional {
        schema: Some(FrontmatterSchema {
            root_uri: "https://outlint.invalid/root.json".into(),
            root: serde_json::json!({"$ref": remote_uri}),
            resources: std::collections::BTreeMap::new(),
        }),
    };

    let error = match PreparedValidator::new(&schema) {
        Err(error) => error,
        Ok(_) => panic!("remote refs cannot be retrieved during preparation"),
    };
    assert!(
        error.message.contains(&format!(
            "JSON Schema resource `{remote_uri}` was not preloaded"
        )),
        "unexpected retrieval diagnostic: {}",
        error.message
    );
    assert!(
        !error.message.contains("Default retriever"),
        "unexpected retrieval diagnostic: {}",
        error.message
    );
}

#[test]
fn reports_invalid_and_forbidden_frontmatter_without_schema_execution() {
    let schema =
        load_schema("version: 2\nfrontmatter: { allow: false }\ntitle: null\nsections: []\n")
            .expect("test schema is valid")
            .schema;
    let document = parse_markdown("---\n- item\n---\n", MarkdownOptions::default());
    let ids = validate(&schema, &document)
        .expect("schema prepares")
        .into_iter()
        .map(|diagnostic| diagnostic.id)
        .collect::<Vec<_>>();
    assert_eq!(
        ids,
        [
            DiagnosticId::ForbiddenFrontmatter,
            DiagnosticId::InvalidFrontmatter
        ]
    );
}

#[test]
fn optional_forbidden_and_file_suppression_apply_to_json_schema() {
    let json_schema = FrontmatterSchema {
        root_uri: "https://outlint.invalid/root.json".into(),
        root: serde_json::Value::Bool(false),
        resources: std::collections::BTreeMap::new(),
    };
    let mut schema = load_schema("version: 2\ntitle: null\nsections: []\n")
        .expect("test schema is valid")
        .schema;
    schema.frontmatter = FrontmatterPolicy::Optional {
        schema: Some(json_schema.clone()),
    };
    let absent = parse_markdown("## Title\n", MarkdownOptions::default());
    assert!(validate(&schema, &absent)
        .expect("schema prepares")
        .is_empty());

    let suppressed = parse_markdown(
        "---\nstatus: draft\n---\n<!-- outlint-disable-file frontmatter-schema -->\n",
        MarkdownOptions::default(),
    );
    assert!(validate(&schema, &suppressed)
        .expect("schema prepares")
        .is_empty());

    schema.frontmatter = FrontmatterPolicy::Forbidden {
        schema: Some(json_schema),
    };
    let present = parse_markdown("---\nstatus: draft\n---\n", MarkdownOptions::default());
    let ids = validate(&schema, &present)
        .expect("schema prepares")
        .into_iter()
        .map(|diagnostic| diagnostic.id)
        .collect::<Vec<_>>();
    assert_eq!(
        ids,
        [
            DiagnosticId::ForbiddenFrontmatter,
            DiagnosticId::FrontmatterSchema
        ]
    );
}
