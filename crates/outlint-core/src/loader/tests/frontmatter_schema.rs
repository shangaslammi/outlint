use std::sync::Arc;

use serde_json::Value;

use super::{error_kinds, invalid, source_slice, valid};
use crate::loader::frontmatter_schema::{external_source_id, invalid_inline_references};
use crate::loader::{
    json_schema_external_references, json_schema_reference_budget_message,
    json_schema_reference_count, linked_frontmatter_schema_path, load_schema,
    load_schema_with_resources, MAX_JSON_SCHEMA_REFERENCES,
};
use crate::{
    ByteOffset, FrontmatterPolicy, LinkedJsonSchemaInput, LoadSchemaResult, SchemaErrorKind,
    SchemaNode, SourceId, SourceLabel, TextRange,
};

#[test]
fn external_source_ids_report_exhaustion_instead_of_saturating() {
    assert_eq!(external_source_id(0), Some(SourceId(1)));
    #[cfg(target_pointer_width = "64")]
    assert_eq!(external_source_id(u32::MAX as usize), None);
}

#[test]
fn normalizes_frontmatter_presence_policy() {
    let schema = valid(
        r#"
version: 1
frontmatter: { required: true }
sections: []
"#,
    );
    assert_eq!(
        schema.frontmatter,
        FrontmatterPolicy::Required { schema: None }
    );
}

#[test]
fn inline_frontmatter_schema_validates_and_preserves_primary_source_provenance() {
    let source = r#"version: 1
frontmatter:
  schema:
    type: object
    required: [status]
    properties:
      status: { enum: [draft, final] }
title: null
sections: []
"#;
    let loaded = load_schema(source).expect("inline JSON Schema is valid");
    let declaration = loaded.locations.nodes[&SchemaNode::FrontmatterSchemaDeclaration];
    let document = loaded.locations.nodes[&SchemaNode::FrontmatterSchemaDocument];
    assert_eq!(declaration, document);
    assert_eq!(declaration.source, SourceId(0));
    assert_eq!(
        &source[declaration.range.start.0..declaration.range.end.0],
        "type: object\n    required: [status]\n    properties:\n      status: { enum: [draft, final] }\n"
    );

    let document = crate::parse_markdown(
        "---\nstatus: review\n---\n",
        crate::MarkdownOptions::default(),
    );
    let diagnostics =
        crate::validate(&loaded.schema, &document).expect("inline schema compiles again");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].id, crate::DiagnosticId::FrontmatterSchema);
    let crate::DiagnosticTarget::Frontmatter { block: Some(block) } = &diagnostics[0].target else {
        panic!("frontmatter schema diagnostic must target its block")
    };
    assert_eq!(block.json_pointer.as_deref(), Some("/status"));
}

#[test]
fn inline_frontmatter_schema_accepts_fragment_references_and_cycles() {
    let loaded = inline(
        r##"{
            "$ref": "#/$defs/node",
            "$defs": {
                "node": {
                    "type": "object",
                    "properties": { "child": { "$ref": "#/$defs/node" } }
                }
            }
        }"##,
    )
    .expect("fragment-only recursive schema is valid");
    let document = crate::parse_markdown(
        "---\nchild:\n  child: false\n---\n",
        crate::MarkdownOptions::default(),
    );
    let diagnostics =
        crate::validate(&loaded.schema, &document).expect("recursive inline schema compiles");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].id, crate::DiagnosticId::FrontmatterSchema);

    let loaded = inline(
        r##"{
            "$defs": {
                "node": {
                    "$dynamicAnchor": "node",
                    "type": "object",
                    "properties": { "child": { "$dynamicRef": "#node" } }
                }
            },
            "properties": { "node": { "$ref": "#/$defs/node" } }
        }"##,
    )
    .expect("fragment-only dynamic reference is valid");
    let document = crate::parse_markdown(
        "---\nnode:\n  child: false\n---\n",
        crate::MarkdownOptions::default(),
    );
    let diagnostics = crate::validate(&loaded.schema, &document)
        .expect("dynamic fragment reference validates without retrieval");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].id, crate::DiagnosticId::FrontmatterSchema);
}

#[test]
fn inline_frontmatter_schema_reserves_reference_members_in_every_object() {
    for (root, expected) in [
        (r#"{"const":{"$ref":"literal"}}"#, "fragment-only"),
        (r#"{"properties":{"$ref":"literal"}}"#, "fragment-only"),
        (
            r#"{"unknown":{"$dynamicRef":17}}"#,
            "must be a string beginning with `#`",
        ),
    ] {
        let invalid = inline(root).expect_err("reserved member is checked lexically");
        assert_eq!(invalid.errors.iter().count(), 1);
        assert_eq!(
            invalid.errors.first.kind,
            SchemaErrorKind::InvalidFrontmatterSchema
        );
        assert!(invalid.errors.first.message.contains(expected));
    }
}

#[test]
fn inline_reference_walk_covers_every_object_member() {
    let root = serde_json::json!({
        "$defs": { "defined": { "$ref": "defined.json" } },
        "properties": { "property": { "$ref": "property.json" } },
        "patternProperties": { ".*": { "$ref": "pattern.json" } },
        "dependentSchemas": { "key": { "$ref": "dependent.json" } },
        "unevaluatedProperties": { "$ref": "unevaluated-properties.json" },
        "unevaluatedItems": { "$ref": "unevaluated-items.json" },
        "if": { "$ref": "if.json" },
        "then": { "$ref": "then.json" },
        "else": { "$ref": "else.json" },
        "const": { "$ref": "literal.json" },
        "enum": [{ "$dynamicRef": "literal.json" }],
        "unknown": { "$ref": "unknown.json" }
    });
    assert_eq!(invalid_inline_references(&root).len(), 12);
    assert_eq!(json_schema_reference_count(&root), 12);
}

#[test]
fn inline_frontmatter_schema_rejects_pointer_hidden_external_references() {
    let invalid = inline(
        r##"{
            "$ref": "#/const",
            "const": { "$ref": "https://example.invalid/hidden.json" }
        }"##,
    )
    .expect_err("a pointer-targeted object cannot hide an external reference");
    assert_eq!(invalid.errors.iter().count(), 1);
    assert!(invalid.errors.first.message.contains("fragment-only"));
    assert!(invalid.errors.first.message.contains("hidden.json"));
}

#[test]
fn inline_frontmatter_schema_relative_ids_resolve_from_the_synthetic_base() {
    let root_id = inline(
        r##"{
            "$id": "schemas/root.json",
            "$defs": { "status": { "enum": ["draft", "final"] } },
            "properties": { "status": { "$ref": "#/$defs/status" } }
        }"##,
    )
    .expect("a root relative id resolves against the hierarchical base");
    let document = crate::parse_markdown(
        "---\nstatus: review\n---\n",
        crate::MarkdownOptions::default(),
    );
    assert_eq!(
        crate::validate(&root_id.schema, &document)
            .expect("root relative id compiles")
            .len(),
        1
    );

    let nested_id = inline(
        r##"{
            "$defs": {
                "node": {
                    "$id": "nested/node.json",
                    "type": "object",
                    "properties": { "child": { "$ref": "#" } }
                }
            },
            "properties": { "node": { "$ref": "#/$defs/node" } }
        }"##,
    )
    .expect("a nested relative id resolves against the hierarchical base");
    let document = crate::parse_markdown(
        "---\nnode:\n  child: false\n---\n",
        crate::MarkdownOptions::default(),
    );
    assert_eq!(
        crate::validate(&nested_id.schema, &document)
            .expect("nested relative id compiles")
            .len(),
        1
    );
}

#[test]
fn inline_frontmatter_schema_enforces_the_supported_dialect_and_meta_schema() {
    inline(
        r#"{
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object"
            }"#,
    )
    .expect("draft 2020-12 is supported inline");

    for root in [
        r#"{"$schema":"http://json-schema.org/draft-07/schema#"}"#,
        r#"{"type":17}"#,
    ] {
        let invalid = inline(root).expect_err("unsupported or malformed schema is invalid");
        assert_eq!(
            invalid.errors.first.kind,
            SchemaErrorKind::InvalidFrontmatterSchema
        );
        assert_eq!(invalid.errors.first.range.source, SourceId(0));
    }
}

#[test]
fn inline_frontmatter_schema_rejects_non_fragment_references() {
    for (keyword, reference) in [
        ("$ref", "defs.json#/$defs/value"),
        ("$ref", "/schemas/defs.json"),
        ("$ref", "file:///schemas/defs.json"),
        ("$ref", "https://example.invalid/schema.json"),
        ("$dynamicRef", "defs.json#node"),
    ] {
        let source = serde_json::json!({ keyword: reference }).to_string();
        let invalid = inline(&source).expect_err("external inline reference is invalid");
        assert_eq!(invalid.errors.iter().count(), 1);
        assert_eq!(
            invalid.errors.first.kind,
            SchemaErrorKind::InvalidFrontmatterSchema
        );
        assert_eq!(invalid.errors.first.range.source, SourceId(0));
        assert!(invalid.errors.first.message.contains("fragment-only"));
        assert!(invalid.errors.first.message.contains(reference));
        let range = invalid.errors.first.range.range;
        let primary = &invalid.sources.documents[&SourceId(0)].text;
        assert_eq!(&primary[range.start.0..range.end.0], source);
    }
}

#[test]
fn inline_frontmatter_schema_rejects_non_string_reference_values() {
    for value in [serde_json::Value::Null, serde_json::json!(17)] {
        let source = serde_json::json!({ "$ref": value }).to_string();
        let invalid = inline(&source).expect_err("reference must be a string");
        assert_eq!(invalid.errors.iter().count(), 1);
        assert_eq!(
            invalid.errors.first.message,
            "inline frontmatter JSON Schema `$ref` must be a string beginning with `#`"
        );
    }
}

#[test]
fn inline_frontmatter_schema_uses_the_shared_reference_budget() {
    let at_budget = hidden_reference_chain(MAX_JSON_SCHEMA_REFERENCES);
    assert_eq!(
        json_schema_reference_count(&at_budget),
        MAX_JSON_SCHEMA_REFERENCES
    );
    inline(&at_budget.to_string()).expect("a pointer-hidden chain may spend the whole budget");
    linked(&at_budget.to_string(), &[]).expect("the linked budget admits the same exact boundary");

    let over_budget = hidden_reference_chain(MAX_JSON_SCHEMA_REFERENCES + 1);
    let invalid =
        inline(&over_budget.to_string()).expect_err("one hidden reference more is refused");
    assert_eq!(invalid.errors.iter().count(), 1);
    assert_eq!(
        invalid.errors.first.message,
        json_schema_reference_budget_message()
    );
    assert_eq!(invalid.errors.first.range.source, SourceId(0));

    let linked_invalid = linked(&over_budget.to_string(), &[])
        .expect_err("linked graphs count pointer-hidden references too");
    assert_eq!(
        linked_invalid.errors.first.message,
        json_schema_reference_budget_message()
    );
    assert_eq!(linked_invalid.errors.first.range.source, SourceId(1));
}

#[test]
fn linked_frontmatter_schema_requires_file_context() {
    let kinds = error_kinds(
        r#"
version: 1
frontmatter: { schema: frontmatter.schema.json }
sections: []
"#,
    );
    assert_eq!(kinds, vec![SchemaErrorKind::InvalidFrontmatterSchema]);
}

#[test]
fn external_references_separate_physical_paths_from_id_based_logical_uris() {
    let references = json_schema_external_references(
        r#"{
                "$id": "https://example.com/schemas/root.json",
                "allOf": [
                    { "$ref": "defs.json" },
                    { "$id": "nested/child.json", "$ref": "more.json" }
                ]
            }"#,
        "file:///workspace/frontmatter.schema.json",
        "https://outlint.invalid/workspace/frontmatter.schema.json",
    )
    .expect("references resolve under both bases");

    assert_eq!(
        references,
        vec![
            crate::JsonSchemaExternalReference {
                physical_uri: "file:///workspace/defs.json".into(),
                logical_uri: "https://example.com/schemas/defs.json".into(),
            },
            crate::JsonSchemaExternalReference {
                physical_uri: "file:///workspace/more.json".into(),
                logical_uri: "https://example.com/schemas/nested/more.json".into(),
            },
        ]
    );
}

#[test]
fn loads_and_resolves_linked_frontmatter_schema_with_local_ref() {
    let loaded = linked(
        r#"{"$schema":"https://json-schema.org/draft/2020-12/schema","$ref":"defs.json#/$defs/frontmatter"}"#,
        &[(
            "https://outlint.invalid/defs.json",
            r#"{"$defs":{"frontmatter":{"type":"object","required":["status"],"properties":{"status":{"enum":["draft","final"]}}}}}"#,
        )],
    )
    .expect("linked JSON Schema is valid");
    let FrontmatterPolicy::Optional { schema: Some(_) } = &loaded.schema.frontmatter else {
        panic!("expected an optional linked frontmatter schema")
    };
    assert!(loaded.sources.documents.contains_key(&SourceId(1)));
    assert!(loaded.sources.documents.contains_key(&SourceId(2)));
    assert!(loaded
        .locations
        .nodes
        .contains_key(&SchemaNode::FrontmatterSchemaDocument));
    let document = crate::parse_markdown(
        "---\nstatus: review\n---\n",
        crate::MarkdownOptions::default(),
    );
    let diagnostics =
        crate::validate(&loaded.schema, &document).expect("loader-created validator compiles");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].id, crate::DiagnosticId::FrontmatterSchema);
}

#[test]
fn accepts_circular_fragment_refs_without_validation_time_io() {
    linked(
        r##"{
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "$ref": "#/$defs/node",
            "$defs": {
                "node": {
                    "type": "object",
                    "properties": { "child": { "$ref": "#/$defs/node" } }
                }
            }
        }"##,
        &[],
    )
    .expect("circular local refs are valid");
}

#[test]
fn accepts_cycles_across_preloaded_resources() {
    linked(
        r#"{"$ref":"other.json"}"#,
        &[(
            "https://outlint.invalid/other.json",
            r#"{"$ref":"root.json"}"#,
        )],
    )
    .expect("the immutable registry supports cross-resource cycles");
}

#[test]
fn semantic_schema_equality_ignores_resource_labels() {
    let first = linked("{\"type\":\"object\"}", &[]).expect("first schema is valid");
    let mut input = resource("https://outlint.invalid/root.json", "{\"type\":\"object\"}");
    input.label = Some(SourceLabel("a/different/location.json".into()));
    let second = load_schema_with_resources(
        linked_schema_source(),
        Some(SourceLabel("elsewhere/schema.yml".into())),
        Some(LinkedJsonSchemaInput {
            root_uri: "https://outlint.invalid/root.json".into(),
            resources: vec![input],
        }),
    )
    .expect("second schema is valid");
    assert_eq!(first.schema, second.schema);
}

#[test]
fn rejects_remote_refs_without_network_retrieval() {
    let remote_uri = "https://example.invalid/frontmatter.schema.json";
    let invalid = linked(
        r#"{
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "$ref": "https://example.invalid/frontmatter.schema.json"
            }"#,
        &[],
    )
    .expect_err("remote refs are unsupported");
    assert_eq!(
        invalid.errors.first.kind,
        SchemaErrorKind::InvalidFrontmatterSchema
    );
    assert_eq!(invalid.errors.first.range.source, SourceId(1));
    assert!(
        invalid.errors.first.message.contains(&format!(
            "JSON Schema resource `{remote_uri}` was not preloaded"
        )),
        "unexpected retrieval diagnostic: {}",
        invalid.errors.first.message
    );
    assert!(!invalid.errors.first.message.contains("Default retriever"));
}

#[test]
fn preserves_ref_siblings_and_boolean_targets() {
    let loaded = linked(
        r#"{"$ref":"defs.json#/$defs/base","required":["sibling"]}"#,
        &[(
            "https://outlint.invalid/defs.json",
            r#"{"$defs":{"base":{"required":["target"]}}}"#,
        )],
    )
    .expect("ref with duplicate sibling keyword is valid");
    let document = crate::parse_markdown(
        "---\ntarget: true\n---\n",
        crate::MarkdownOptions::default(),
    );
    let diagnostics =
        crate::validate(&loaded.schema, &document).expect("loader-created validator compiles");
    assert_eq!(diagnostics.len(), 1, "`$ref` siblings must both apply");

    let loaded = linked(
        r#"{"$ref":"defs.json#/$defs/base","type":"object"}"#,
        &[(
            "https://outlint.invalid/defs.json",
            r#"{"$defs":{"base":false}}"#,
        )],
    )
    .expect("boolean ref target is valid");
    let diagnostics =
        crate::validate(&loaded.schema, &document).expect("loader-created validator compiles");
    assert_eq!(diagnostics.len(), 1, "false target must remain rejecting");
}

#[test]
fn positions_invalid_linked_json_schema_in_its_own_source() {
    let invalid = linked("{ invalid json }", &[]).expect_err("linked JSON Schema is invalid");
    assert_eq!(invalid.errors.iter().count(), 1);
    assert_eq!(
        invalid.errors.first.kind,
        SchemaErrorKind::InvalidFrontmatterSchema
    );
    assert_eq!(invalid.errors.first.range.source, SourceId(1));
    let source = invalid
        .sources
        .documents
        .get(&SourceId(1))
        .unwrap_or_else(|| panic!("missing linked JSON Schema source"));
    assert_eq!(source.label, Some(SourceLabel("root.json".into())));
}

#[test]
fn positions_invalid_transitive_resource_in_its_own_source() {
    let invalid = linked(
        r#"{"$ref":"defs.json"}"#,
        &[("https://outlint.invalid/defs.json", "{ invalid json }")],
    )
    .expect_err("transitive resource is invalid");
    assert_eq!(invalid.errors.first.range.source, SourceId(2));
    assert_eq!(
        invalid.sources.documents[&SourceId(2)].label,
        Some(SourceLabel("defs.json".into()))
    );
}

#[test]
fn collects_independent_linked_resource_errors_in_input_order() {
    let invalid = linked(
        r#"{"allOf":[{"$ref":"first.json"},{"$ref":"second.json"}]}"#,
        &[
            ("https://outlint.invalid/first.json", "{ invalid json }"),
            ("https://outlint.invalid/second.json", "[]"),
        ],
    )
    .expect_err("both invalid linked resources must be reported");
    let errors = invalid.errors.iter().collect::<Vec<_>>();

    assert_eq!(errors.len(), 2);
    assert_eq!(errors[0].kind, SchemaErrorKind::InvalidFrontmatterSchema);
    assert_eq!(errors[0].range.source, SourceId(2));
    assert!(errors[0]
        .message
        .starts_with("invalid linked JSON Schema document:"));
    assert_eq!(errors[1].kind, SchemaErrorKind::InvalidFrontmatterSchema);
    assert_eq!(errors[1].range.source, SourceId(3));
    assert_eq!(
        errors[1].message,
        "frontmatter JSON Schema root must be an object or boolean"
    );
}

#[test]
fn duplicate_linked_resource_error_uses_the_duplicate_occurrence_source() {
    let uri = "https://outlint.invalid/duplicate.json";
    let mut first = resource(uri, "{ invalid json }");
    first.label = Some(SourceLabel("first-duplicate.json".into()));
    let mut second = resource(uri, "{}");
    second.label = Some(SourceLabel("second-duplicate.json".into()));
    let invalid = load_schema_with_resources(
        linked_schema_source(),
        Some(SourceLabel("schema.yml".into())),
        Some(LinkedJsonSchemaInput {
            root_uri: "https://outlint.invalid/root.json".into(),
            resources: vec![
                resource(
                    "https://outlint.invalid/root.json",
                    r#"{"$ref":"duplicate.json"}"#,
                ),
                first,
                second,
            ],
        }),
    )
    .expect_err("invalid and duplicate resources must both be reported");
    let errors = invalid.errors.iter().collect::<Vec<_>>();

    assert_eq!(errors.len(), 2);
    assert_eq!(errors[0].range.source, SourceId(2));
    assert_eq!(errors[1].range.source, SourceId(3));
    assert!(errors[1]
        .message
        .starts_with("duplicate JSON Schema resource URI"));
    assert_eq!(
        invalid.sources.documents[&SourceId(3)].label,
        Some(SourceLabel("second-duplicate.json".into()))
    );
}

#[test]
fn positions_linked_schema_read_failures_at_the_unreadable_resource() {
    let message = "cannot inspect linked JSON Schema 'missing.json': not found";
    for (resources, expected_source, expected_label) in [
        (
            vec![failed_resource(
                "https://outlint.invalid/root.json",
                "missing-root.json",
                message,
            )],
            SourceId(1),
            "missing-root.json",
        ),
        (
            vec![
                resource(
                    "https://outlint.invalid/root.json",
                    r#"{"$ref":"missing.json"}"#,
                ),
                failed_resource(
                    "https://outlint.invalid/missing.json",
                    "missing.json",
                    message,
                ),
            ],
            SourceId(2),
            "missing.json",
        ),
    ] {
        let invalid = load_schema_with_resources(
            linked_schema_source(),
            Some(SourceLabel("schema.yml".into())),
            Some(LinkedJsonSchemaInput {
                root_uri: "https://outlint.invalid/root.json".into(),
                resources,
            }),
        )
        .expect_err("linked schema read failure is invalid");

        assert_eq!(
            invalid.errors.first.kind,
            SchemaErrorKind::InvalidFrontmatterSchema
        );
        assert_eq!(invalid.errors.first.message, message);
        assert_eq!(invalid.errors.first.range.source, expected_source);
        assert_eq!(
            invalid.errors.first.range.range,
            TextRange {
                start: ByteOffset(0),
                end: ByteOffset(0),
            }
        );
        let source = &invalid.sources.documents[&expected_source];
        assert_eq!(source.label, Some(SourceLabel(expected_label.into())));
        assert_eq!(&*source.text, "");
    }
}

#[test]
fn rejects_missing_and_unsupported_linked_json_schemas() {
    let root = resource("https://outlint.invalid/not-root.json", "{}");
    let missing = load_schema_with_resources(
        linked_schema_source(),
        None,
        Some(LinkedJsonSchemaInput {
            root_uri: "https://outlint.invalid/root.json".into(),
            resources: vec![root],
        }),
    )
    .expect_err("missing linked schema is invalid");
    assert_eq!(
        missing.errors.first.kind,
        SchemaErrorKind::InvalidFrontmatterSchema
    );
    assert_eq!(missing.errors.first.range.source, SourceId(0));

    let unsupported = linked(
        r#"{"$schema":"http://json-schema.org/draft-07/schema#","type":"object"}"#,
        &[],
    )
    .expect_err("unsupported dialect is invalid");
    assert_eq!(
        unsupported.errors.first.kind,
        SchemaErrorKind::InvalidFrontmatterSchema
    );
    assert_eq!(unsupported.errors.first.range.source, SourceId(1));
}

fn linked(root: &str, resources: &[(&str, &str)]) -> LoadSchemaResult {
    let mut rest = Vec::new();
    for (uri, source) in resources {
        rest.push(resource(uri, source));
    }
    load_schema_with_resources(
        linked_schema_source(),
        Some(SourceLabel("schema.yml".into())),
        Some(LinkedJsonSchemaInput {
            root_uri: "https://outlint.invalid/root.json".into(),
            resources: std::iter::once(resource("https://outlint.invalid/root.json", root))
                .chain(rest)
                .collect(),
        }),
    )
}

fn inline(root: &str) -> LoadSchemaResult {
    let root: Value = serde_json::from_str(root).expect("test inline schema is valid JSON");
    load_schema(&format!(
        "version: 1\nfrontmatter:\n  schema: {}\ntitle: null\nsections: []\n",
        serde_json::to_string(&root).expect("test inline schema serializes")
    ))
}

fn resource(uri: &str, source: &str) -> crate::JsonSchemaResourceInput {
    crate::JsonSchemaResourceInput {
        uri: uri.into(),
        label: Some(SourceLabel(
            uri.rsplit('/').next().unwrap_or(uri).to_owned(),
        )),
        contents: crate::JsonSchemaResourceContents::Loaded(Arc::from(source)),
    }
}

fn failed_resource(uri: &str, label: &str, message: &str) -> crate::JsonSchemaResourceInput {
    crate::JsonSchemaResourceInput {
        uri: uri.into(),
        label: Some(SourceLabel(label.into())),
        contents: crate::JsonSchemaResourceContents::ReadFailure(message.into()),
    }
}

fn linked_schema_source() -> &'static str {
    "version: 1\nfrontmatter:\n  schema: root.json\ntitle: null\nsections: []\n"
}

/// Builds a document whose root reference starts a chain of `links` hops,
/// the last of which targets `tail`. It declares `links + 1` references
/// and nests three levels however long the chain is.
fn reference_chain(links: usize, tail: &str) -> String {
    let mut definitions = serde_json::Map::new();
    definitions.insert("end".into(), serde_json::Value::Bool(true));
    for index in 0..links {
        let target = if index + 1 == links {
            tail.to_owned()
        } else {
            format!("#/$defs/{}", index + 1)
        };
        definitions.insert(index.to_string(), serde_json::json!({ "$ref": target }));
    }
    serde_json::json!({ "$ref": "#/$defs/0", "$defs": definitions }).to_string()
}

/// Builds a fragment chain whose schemas are hidden inside instance data.
///
/// The root pointer activates the first object in `const`; each object then
/// points at the next array element until the final `true` schema. The
/// result declares exactly `references` `$ref` members even though only
/// the root occupies a keyword position recognized as a subresource.
fn hidden_reference_chain(references: usize) -> Value {
    assert!(references > 0, "a reference chain has a root reference");
    let mut hidden = (0..references - 1)
        .map(|index| serde_json::json!({ "$ref": format!("#/const/{}", index + 1) }))
        .collect::<Vec<_>>();
    hidden.push(Value::Bool(true));
    serde_json::json!({ "$ref": "#/const/0", "const": hidden })
}

#[test]
fn refuses_a_reference_chain_longer_than_the_compiler_can_recurse_over() {
    // Compiling a reference re-enters the compiler at its target, so a
    // chain costs a stack frame per link while every link of it sits at
    // the same JSON depth: the YAML depth limit and `serde_json`'s parse
    // limit are both satisfied with room to spare by a chain long enough
    // to abort the process. The count is charged before the graph reaches
    // the compiler, so the boundary is pinned on both sides -- a graph
    // spending the whole budget must still load, or the bound would be
    // free to drift downwards unnoticed.
    let at_budget = reference_chain(MAX_JSON_SCHEMA_REFERENCES - 1, "#/$defs/end");
    assert_eq!(
        json_schema_reference_count(
            &serde_json::from_str(&at_budget).expect("chain is valid JSON")
        ),
        MAX_JSON_SCHEMA_REFERENCES
    );
    linked(&at_budget, &[]).expect("a graph spending the whole budget still loads");

    let over_budget = reference_chain(MAX_JSON_SCHEMA_REFERENCES, "#/$defs/end");
    let invalid = linked(&over_budget, &[]).expect_err("one reference more is refused");
    assert_eq!(
        invalid.errors.first.kind,
        SchemaErrorKind::InvalidFrontmatterSchema
    );
    assert_eq!(
        invalid.errors.first.message,
        json_schema_reference_budget_message()
    );
    assert_eq!(invalid.errors.first.range.source, SourceId(1));
    assert!(invalid.errors.rest.is_empty());
}

#[test]
fn the_reference_budget_counts_dynamic_references_too() {
    // `$dynamicRef` compiles through the same function as `$ref` and
    // re-enters the compiler the same way, so a chain of them aborts at
    // the same length. Counting only `$ref` would leave the crash
    // reachable by renaming one keyword.
    let over_budget = reference_chain(MAX_JSON_SCHEMA_REFERENCES, "#/$defs/end")
        .replace(r#""$ref""#, r#""$dynamicRef""#);
    let invalid = linked(&over_budget, &[]).expect_err("a dynamic chain is a chain");
    assert_eq!(
        invalid.errors.first.message,
        json_schema_reference_budget_message()
    );
}

#[test]
fn the_reference_budget_spans_the_graph_and_names_where_it_runs_out() {
    // The compiler recurses across resource boundaries as readily as
    // within one, so a per-document budget would be no budget at all: two
    // documents each under it can name a chain twice as long as either.
    // The total is therefore charged over the graph, and reported against
    // the resource whose references spend the last of it rather than the
    // root, so the diagnostic points at a document the author can shorten.
    let half = MAX_JSON_SCHEMA_REFERENCES / 2;
    let root = reference_chain(half - 1, "defs.json#/$defs/0");
    let definitions = reference_chain(half, "#/$defs/end");
    assert_eq!(
        json_schema_reference_count(&serde_json::from_str(&root).expect("root is valid JSON"))
            + json_schema_reference_count(
                &serde_json::from_str(&definitions).expect("defs are valid JSON")
            ),
        MAX_JSON_SCHEMA_REFERENCES + 1
    );

    let invalid = linked(
        &root,
        &[("https://outlint.invalid/defs.json", &definitions)],
    )
    .expect_err("a chain split across two documents is still one chain");
    assert_eq!(
        invalid.errors.first.kind,
        SchemaErrorKind::InvalidFrontmatterSchema
    );
    assert_eq!(
        invalid.errors.first.message,
        json_schema_reference_budget_message()
    );
    assert_eq!(invalid.errors.first.range.source, SourceId(2));
}

// ---------------------------------------------------------------------------
// Frontmatter capture guardrails
// ---------------------------------------------------------------------------
//
// These pin the answers a `frontmatter.captures` declaration already gets from
// the reader and the shape rule, before the frontmatter loader normalizes one.
// They belong here rather than beside the duplicate-classification walk because
// what they hold is the frontmatter loader's contract with that walk: the
// classification and its anchor are what the loader must not restate, and the
// JSON Schema behaviour is what declaring captures must not disturb.

/// §2.3 makes a key repeated inside `captures` `invalid-capture` rather than
/// `syntax`, and §6.3 anchors it at the later spelling: the one an author
/// would delete. Nothing downstream re-checks this, so the classification and
/// the anchor are pinned from the side that depends on them.
#[test]
fn repeated_frontmatter_capture_names_anchor_the_later_key() {
    let source = concat!(
        "version: 1\n",
        "frontmatter:\n",
        "  captures:\n",
        "    version:\n",
        "      type: semver\n",
        "    version:\n",
        "      type: text\n",
        "sections: []\n",
    );
    let refused = invalid(source);
    assert!(refused.errors.rest.is_empty());
    assert_eq!(refused.errors.first.kind, SchemaErrorKind::InvalidCapture);
    assert_eq!(
        refused.errors.first.message,
        "duplicate capture name `version`"
    );
    assert_eq!(refused.errors.first.range.source, SourceId(0));
    assert_eq!(source_slice(source, refused.errors.first.range), "version");
    let later = source
        .rfind("    version:\n")
        .expect("the source spells the name twice")
        + "    ".len();
    assert_eq!(
        refused.errors.first.range.range,
        TextRange {
            start: ByteOffset(later),
            end: ByteOffset(later + "version".len()),
        }
    );
}

/// §2.1's special classification covers the `captures` mapping's own keys and
/// stops there: a key repeated *inside* one declaration is ordinary duplicate
/// YAML and stays `syntax`. Pinned because the two mappings are one line apart
/// in the source and a widened scope would silently reclassify this one.
#[test]
fn capture_declaration_duplicates_remain_syntax() {
    let source = concat!(
        "version: 1\n",
        "frontmatter:\n",
        "  captures:\n",
        "    version:\n",
        "      type: semver\n",
        "      type: text\n",
        "sections: []\n",
    );
    let refused = invalid(source);
    assert!(refused.errors.rest.is_empty());
    assert_eq!(refused.errors.first.kind, SchemaErrorKind::Syntax);
    assert_eq!(
        refused.errors.first.message,
        "invalid YAML: duplicate mapping key `type`"
    );
    assert_eq!(refused.errors.first.range.source, SourceId(0));
    assert_eq!(source_slice(source, refused.errors.first.range), "type");
    let later = source
        .rfind("      type:")
        .expect("the declaration spells the key twice")
        + "      ".len();
    assert_eq!(
        refused.errors.first.range.range,
        TextRange {
            start: ByteOffset(later),
            end: ByteOffset(later + "type".len()),
        }
    );
}

/// A capture name is `[a-z][a-z0-9_]*` under §2.2, so the name checks the
/// frontmatter loader performs take string keys. A non-string key has already
/// failed the upstream rule that mapping keys are strings, and §6.3 forbids
/// attempting a check whose input could not be built, so the refusal stays
/// `invalid-document-shape` and never becomes a second `invalid-capture`.
#[test]
fn non_string_frontmatter_capture_keys_fail_shape_first() {
    let source = concat!(
        "version: 1\n",
        "frontmatter:\n",
        "  captures:\n",
        "    1:\n",
        "      type: int\n",
        "sections: []\n",
    );
    let refused = invalid(source);
    assert!(refused.errors.rest.is_empty());
    assert_eq!(
        refused.errors.first.kind,
        SchemaErrorKind::InvalidDocumentShape
    );
    assert_eq!(refused.errors.first.message, "mapping keys must be strings");
    assert_eq!(refused.errors.first.range.source, SourceId(0));
    assert_eq!(source_slice(source, refused.errors.first.range), "1");
    assert!(refused
        .errors
        .iter()
        .all(|error| error.kind != SchemaErrorKind::InvalidCapture));
}

/// The linked-schema path is read off the outer YAML before anything is
/// loaded, so the I/O shell can perform its reads first. A sibling `captures`
/// key is not on that path and must not change what it returns.
#[test]
fn a_captures_declaration_does_not_disturb_the_linked_schema_path() {
    let source = concat!(
        "version: 1\n",
        "frontmatter:\n",
        "  captures:\n",
        "    version:\n",
        "      type: semver\n",
        "  schema: linked.json\n",
        "sections: []\n",
    );
    assert_eq!(
        linked_frontmatter_schema_path(source).as_deref(),
        Some("linked.json")
    );
}

/// §2.3: "`frontmatter.schema` and `frontmatter.captures` are complementary."
/// Declaring captures must therefore leave inline schema preparation exactly
/// as it was — same declaration and document nodes, same primary source.
#[test]
fn inline_json_schema_loads_beside_a_captures_declaration() {
    let source = concat!(
        "version: 1\n",
        "frontmatter:\n",
        "  captures:\n",
        "    version:\n",
        "      type: semver\n",
        "  schema:\n",
        "    type: object\n",
        "title: null\n",
        "sections: []\n",
    );
    let loaded = load_schema(source).expect("captures do not disturb an inline schema");
    assert!(loaded.schema.frontmatter.schema().is_some());
    let declaration = loaded.locations.nodes[&SchemaNode::FrontmatterSchemaDeclaration];
    let document = loaded.locations.nodes[&SchemaNode::FrontmatterSchemaDocument];
    assert_eq!(declaration, document);
    assert_eq!(declaration.source, SourceId(0));
    assert_eq!(source_slice(source, declaration), "type: object\n");
}

/// The linked half of the same guarantee: external source ids, the external
/// document node, and the compiled schema all survive a sibling `captures`
/// declaration untouched.
#[test]
fn linked_json_schema_loads_beside_a_captures_declaration() {
    let source = concat!(
        "version: 1\n",
        "frontmatter:\n",
        "  captures:\n",
        "    version:\n",
        "      type: semver\n",
        "  schema: root.json\n",
        "title: null\n",
        "sections: []\n",
    );
    let loaded = load_schema_with_resources(
        source,
        Some(SourceLabel("schema.yml".into())),
        Some(LinkedJsonSchemaInput {
            root_uri: "https://outlint.invalid/root.json".into(),
            resources: vec![resource(
                "https://outlint.invalid/root.json",
                r#"{"type":"object","required":["status"]}"#,
            )],
        }),
    )
    .expect("captures do not disturb a linked schema");
    assert!(loaded.schema.frontmatter.schema().is_some());
    assert!(loaded.sources.documents.contains_key(&SourceId(1)));
    assert_eq!(
        loaded.locations.nodes[&SchemaNode::FrontmatterSchemaDocument].source,
        SourceId(1)
    );
    assert_eq!(
        loaded.locations.nodes[&SchemaNode::FrontmatterSchemaDeclaration].source,
        SourceId(0)
    );
    let document = crate::parse_markdown("---\ntitle: x\n---\n", crate::MarkdownOptions::default());
    let diagnostics =
        crate::validate(&loaded.schema, &document).expect("the linked schema compiles again");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].id, crate::DiagnosticId::FrontmatterSchema);
}

// ---------------------------------------------------------------------------
// Frontmatter capture normalization
// ---------------------------------------------------------------------------

/// Loads one frontmatter capture declaration and returns the policy's view.
fn captures_of(declarations: &str) -> crate::Schema {
    valid(&format!(
        "version: 1\nfrontmatter:\n  captures:\n{declarations}sections: []\n"
    ))
}

/// §2.4's type set is closed and its spellings are stable, so each of the six
/// resolves and reports itself back by the name the source used.
#[test]
fn every_declared_capture_type_normalizes() {
    let schema = captures_of(concat!(
        "    a:\n      type: int\n",
        "    b:\n      type: bool\n",
        "    c:\n      type: date\n",
        "    d:\n      type: semver\n",
        "    e:\n      type: dotted\n",
        "    f:\n      type: text\n",
    ));
    let captures = schema.frontmatter.captures();
    assert_eq!(captures.len(), 6);
    assert_eq!(
        captures
            .iter()
            .map(|(name, capture)| (name.as_str(), capture.type_name()))
            .collect::<Vec<_>>(),
        vec![
            ("a", "int"),
            ("b", "bool"),
            ("c", "date"),
            ("d", "semver"),
            ("e", "dotted"),
            ("f", "text"),
        ]
    );
}

/// §2.3 gives `required` the default `false`; an explicit value of either
/// polarity is kept. The default is what decides whether an absent value is
/// `missing-value`, so it is pinned rather than left to the reader.
#[test]
fn an_omitted_capture_required_flag_defaults_to_false() {
    let schema = captures_of(concat!(
        "    plain:\n      type: text\n",
        "    yes_flag:\n      type: text\n      required: true\n",
        "    no_flag:\n      type: text\n      required: false\n",
    ));
    let captures = schema.frontmatter.captures();
    assert_eq!(
        captures
            .iter()
            .map(|(name, capture)| (name.as_str(), capture.is_required()))
            .collect::<Vec<_>>(),
        vec![("no_flag", false), ("plain", false), ("yes_flag", true)]
    );
}

/// §2.3: "When omitted, it defaults to the capture name as one name segment;
/// for capture `version`, the default is equivalent to `$['version']`." The
/// default is spelled in bracket-quoted form so that one normalized spelling
/// serves every name the grammar admits.
#[test]
fn an_omitted_capture_path_defaults_to_the_bracketed_name() {
    let schema = captures_of("    version:\n      type: semver\n");
    let captures = schema.frontmatter.captures();
    let (name, _) = captures.iter().next().expect("the declaration normalized");
    assert_eq!(name.as_str(), "version");
    let capture = captures
        .get(&name.clone())
        .expect("the collection is keyed by that name");
    assert_eq!(capture.path_source(), "$['version']");
}

/// The default path is not merely equivalent to the explicit spelling, it is
/// that spelling: two schemas differing only in whether the author wrote it
/// compare equal, so no consumer can tell the defaulted one apart.
#[test]
fn a_defaulted_capture_path_equals_the_explicit_spelling() {
    let defaulted = captures_of("    version:\n      type: semver\n");
    let explicit = captures_of("    version:\n      type: semver\n      path: \"$['version']\"\n");
    assert_eq!(defaulted.frontmatter, explicit.frontmatter);
}

/// §2.3's presence policies and its capture declaration are independent, and
/// the model makes the two allowed combinations the only representable ones.
/// Both are reached here so neither variant can be produced by accident.
#[test]
fn capture_bearing_policies_follow_the_presence_policy() {
    let optional = captures_of("    version:\n      type: semver\n");
    assert!(matches!(
        optional.frontmatter,
        FrontmatterPolicy::OptionalWithCaptures { schema: None, .. }
    ));
    assert!(!optional.frontmatter.is_required());

    let required = valid(concat!(
        "version: 1\n",
        "frontmatter:\n",
        "  required: true\n",
        "  captures:\n",
        "    version:\n",
        "      type: semver\n",
        "sections: []\n",
    ));
    assert!(matches!(
        required.frontmatter,
        FrontmatterPolicy::RequiredWithCaptures { schema: None, .. }
    ));
    assert!(required.frontmatter.is_required());
    assert!(!required.frontmatter.is_forbidden());
}

/// The collection is keyed rather than ordered, so several declarations
/// arrive in capture-name order however the source spelled them, and each
/// keeps its own type, path, and flag.
#[test]
fn several_capture_declarations_normalize_into_one_keyed_collection() {
    let schema = captures_of(concat!(
        "    status:\n      type: text\n      path: \"$.meta['status']\"\n",
        "    version:\n      type: semver\n      required: true\n",
        "    build:\n      type: int\n      path: \"$.builds[-1]\"\n",
    ));
    let captures = schema.frontmatter.captures();
    assert_eq!(captures.len(), 3);
    assert_eq!(
        captures
            .iter()
            .map(|(name, capture)| (
                name.as_str(),
                capture.type_name(),
                capture.path_source(),
                capture.is_required()
            ))
            .collect::<Vec<_>>(),
        vec![
            ("build", "int", "$.builds[-1]", false),
            ("status", "text", "$.meta['status']", false),
            ("version", "semver", "$['version']", true),
        ]
    );
    assert!(captures
        .declared()
        .is_some_and(|declared| declared.len() == 3));
}

/// §6.3 anchors `invalid-capture` "at the offending capture declaration", so
/// the node the loader records has to be that whole entry — key through the
/// end of its value — and not just the name or just the declaration body.
#[test]
fn a_frontmatter_capture_node_covers_its_complete_declaration() {
    let source = concat!(
        "version: 1\n",
        "frontmatter:\n",
        "  captures:\n",
        "    version:\n",
        "      type: semver\n",
        "      required: true\n",
        "sections: []\n",
    );
    let loaded = load_schema(source).expect("the declaration normalizes");
    let captures = loaded.schema.frontmatter.captures();
    let (name, _) = captures.iter().next().expect("the declaration normalized");
    let range = loaded.locations.nodes[&SchemaNode::FrontmatterCapture(name.clone())];
    assert_eq!(range.source, SourceId(0));
    assert_eq!(
        source_slice(source, range),
        "version:\n      type: semver\n      required: true\n"
    );
}

/// §4.3: "Frontmatter captures occupy a separate named scope rooted at `fm`;
/// they do not collide with names at the schema root." A capture sharing a
/// root rule's default id is therefore an ordinary schema, not `duplicate-id`.
#[test]
fn frontmatter_captures_do_not_collide_with_outline_names() {
    let schema = valid(concat!(
        "version: 1\n",
        "frontmatter:\n",
        "  captures:\n",
        "    overview:\n",
        "      type: text\n",
        "outline:\n",
        "  - match: Overview\n",
    ));
    assert_eq!(schema.frontmatter.captures().len(), 1);
    assert_eq!(schema.outline.len(), 1);
}

// ---------------------------------------------------------------------------
// Frontmatter capture rejection
// ---------------------------------------------------------------------------

/// Builds a schema whose `frontmatter.captures` is spelled by `declarations`.
fn capture_source(declarations: &str) -> String {
    format!("version: 1\nfrontmatter:\n  captures:\n{declarations}sections: []\n")
}

/// The one error a source is expected to produce, with its anchor's text.
fn single_capture_error(source: &str) -> (String, String) {
    let refused = invalid(source);
    assert!(
        refused.errors.rest.is_empty(),
        "expected one error, got {:#?}",
        refused.errors
    );
    assert_eq!(refused.errors.first.kind, SchemaErrorKind::InvalidCapture);
    assert_eq!(refused.errors.first.range.source, SourceId(0));
    (
        refused.errors.first.message.clone(),
        source_slice(source, refused.errors.first.range).to_owned(),
    )
}

/// §2.3: "A frontmatter `captures` value MUST be a non-empty mapping." Every
/// way of writing something else is one fault of the collection, so §6.3
/// anchors it at the `captures` key rather than at an entry there is none of.
#[test]
fn rejects_invalid_capture_collections() {
    for (spelling, anchor) in [
        ("  captures: {}\n", "{}"),
        ("  captures: null\n", "null"),
        ("  captures: version\n", "version"),
        ("  captures: 3\n", "3"),
        ("  captures: [version]\n", "[version]"),
    ] {
        let source = format!("version: 1\nfrontmatter:\n{spelling}sections: []\n");
        let (message, slice) = single_capture_error(&source);
        assert_eq!(
            message, "frontmatter.captures must be a non-empty mapping of capture declarations",
            "for {spelling:?}"
        );
        assert_eq!(slice, anchor, "for {spelling:?}");
    }
}

/// §2.2's capture-name grammar is `[a-z][a-z0-9_]*`, and it is ASCII: an
/// uppercase letter, a hyphen, a dot, a leading digit, a leading underscore,
/// and the empty name are each outside it. §6.3 anchors the refusal at the
/// declaration, which is the entry an author would rename.
#[test]
fn rejects_invalid_capture_names() {
    for spelled in ["Version", "my-name", "my.name", "1st", "_lead", "verão"] {
        let source = capture_source(&format!("    {spelled}:\n      type: text\n"));
        let (message, slice) = single_capture_error(&source);
        assert_eq!(
            message,
            format!("capture name `{spelled}` must match `[a-z][a-z0-9_]*`")
        );
        assert_eq!(slice, format!("{spelled}:\n      type: text\n"));
    }

    let source = capture_source("    \"\":\n      type: text\n");
    let (message, slice) = single_capture_error(&source);
    assert_eq!(message, "capture name `` must match `[a-z][a-z0-9_]*`");
    assert_eq!(slice, "\"\":\n      type: text\n");
}

/// §2.3 requires each value to be "an object having exactly the required key
/// `type` and optional keys `path` and `required`". A value that is not an
/// object, one missing `type`, and one carrying a key outside that set are
/// the three ways of failing that sentence.
#[test]
fn rejects_malformed_capture_objects() {
    for spelled in [
        "    version:\n",
        "    version: text\n",
        "    version: [text]\n",
    ] {
        let source = capture_source(spelled);
        let (message, _) = single_capture_error(&source);
        assert_eq!(
            message,
            "frontmatter capture `version` must be a mapping declaring a `type`"
        );
    }

    let source = capture_source("    version:\n      path: \"$.version\"\n");
    let (message, slice) = single_capture_error(&source);
    assert_eq!(
        message,
        "frontmatter capture `version` is missing required field `type`"
    );
    assert_eq!(slice, "version:\n      path: \"$.version\"\n");

    let source = capture_source("    version:\n      type: text\n      kind: header\n");
    let (message, slice) = single_capture_error(&source);
    assert_eq!(
        message,
        "unknown field `kind` in frontmatter capture `version`"
    );
    assert_eq!(slice, "version:\n      type: text\n      kind: header\n");
}

/// §2.4's type set is closed and its spellings are exact, so a type that is
/// not a string, not one of the six, or one of the six in another case is
/// refused. The message lists the set, because "unknown" is only actionable
/// beside what is known.
#[test]
fn rejects_invalid_capture_types() {
    for spelled in ["      type:\n", "      type: 3\n", "      type: [text]\n"] {
        let source = capture_source(&format!("    version:\n{spelled}"));
        let (message, _) = single_capture_error(&source);
        assert_eq!(
            message,
            "frontmatter capture `version` `type` must be a string and cannot be null"
        );
    }

    for spelled in ["bogus", "Text", "INT", "string"] {
        let source = capture_source(&format!("    version:\n      type: {spelled}\n"));
        let (message, slice) = single_capture_error(&source);
        assert_eq!(
            message,
            format!(
                "unknown capture type `{spelled}` in frontmatter capture `version`; \
                 expected one of `int`, `bool`, `date`, `semver`, `dotted`, `text`"
            )
        );
        assert_eq!(slice, format!("version:\n      type: {spelled}\n"));
    }
}

/// §2.3: "`path` MUST be a string containing an absolute, `$`-rooted RFC 9535
/// JSONPath singular query." A value that is not a string never reaches the
/// query parser, so it is refused for what it is.
#[test]
fn rejects_non_string_capture_paths() {
    for spelled in [
        "      path:\n",
        "      path: 3\n",
        "      path: [\"$.a\"]\n",
    ] {
        let source = capture_source(&format!("    version:\n      type: text\n{spelled}"));
        let (message, _) = single_capture_error(&source);
        assert_eq!(
            message,
            "frontmatter capture `version` `path` must be a string and cannot be null"
        );
    }
}

/// §2.3 gives `required` the type `bool`. YAML 1.2's core resolver makes
/// `yes` a string rather than a boolean, so the common spelling is refused
/// here too rather than silently reading as true.
#[test]
fn rejects_invalid_capture_required_flags() {
    for spelled in [
        "      required:\n",
        "      required: 1\n",
        "      required: yes\n",
        "      required: [true]\n",
    ] {
        let source = capture_source(&format!("    version:\n      type: text\n{spelled}"));
        let (message, _) = single_capture_error(&source);
        assert_eq!(
            message,
            "frontmatter capture `version` `required` must be a bool and cannot be null"
        );
    }
}

/// §6.3: "Independent schema errors MUST be collected together." One bad
/// declaration therefore does not hide the next, and each keeps its own
/// anchor rather than sharing the collection's.
#[test]
fn collects_independent_capture_declaration_errors() {
    let source = capture_source(concat!(
        "    good:\n      type: text\n",
        "    bad:\n      type: bogus\n",
        "    worse:\n      type: 3\n",
    ));
    let refused = invalid(&source);
    let errors = refused.errors.iter().collect::<Vec<_>>();
    assert_eq!(errors.len(), 2);
    assert!(errors
        .iter()
        .all(|error| error.kind == SchemaErrorKind::InvalidCapture));
    assert_eq!(
        errors
            .iter()
            .map(|error| source_slice(&source, error.range))
            .collect::<Vec<_>>(),
        // A declaration's range runs from its key to the end of its value,
        // so a non-final entry carries the indentation the next key sits on.
        vec!["bad:\n      type: bogus\n    ", "worse:\n      type: 3\n"]
    );
}

/// The same rule inside one declaration: its faults are independent of each
/// other, so all four are reported, and §6.3 anchors every one of them at the
/// declaration they belong to.
#[test]
fn collects_independent_defects_within_one_capture_declaration() {
    let source = capture_source(concat!(
        "    version:\n",
        "      type: bogus\n",
        "      path: 3\n",
        "      required: 4\n",
        "      extra: yes\n",
    ));
    let refused = invalid(&source);
    let errors = refused.errors.iter().collect::<Vec<_>>();
    assert_eq!(errors.len(), 4);
    assert!(errors
        .iter()
        .all(|error| error.kind == SchemaErrorKind::InvalidCapture));
    let declaration = source_slice(
        &source,
        errors.first().expect("the declaration was refused").range,
    );
    assert_eq!(
        declaration,
        "version:\n      type: bogus\n      path: 3\n      required: 4\n      extra: yes\n"
    );
    assert!(errors
        .iter()
        .all(|error| source_slice(&source, error.range) == declaration));
}

/// A collection with one unusable entry is unusable as a whole: the load
/// fails, so neither the refused entry nor its valid neighbour can be
/// observed in a normalized collection or in the node map.
#[test]
fn a_rejected_capture_never_reaches_a_loaded_schema() {
    let source = capture_source(concat!(
        "    good:\n      type: text\n",
        "    bad:\n      type: bogus\n",
    ));
    assert_eq!(
        error_kinds(&source),
        vec![SchemaErrorKind::InvalidCapture],
        "only the unusable entry is refused"
    );
    assert!(load_schema(&source).is_err());
}

/// §2.1's special classification is for keys *inside* a `captures` mapping.
/// The `captures` key itself is an ordinary key of the `frontmatter` mapping,
/// so repeating it stays `syntax`, anchored at the later spelling.
#[test]
fn frontmatter_field_duplicates_remain_syntax() {
    let source = concat!(
        "version: 1\n",
        "frontmatter:\n",
        "  captures:\n",
        "    version:\n",
        "      type: semver\n",
        "  captures:\n",
        "    build:\n",
        "      type: int\n",
        "sections: []\n",
    );
    let refused = invalid(source);
    assert!(refused.errors.rest.is_empty());
    assert_eq!(refused.errors.first.kind, SchemaErrorKind::Syntax);
    assert_eq!(
        refused.errors.first.message,
        "invalid YAML: duplicate mapping key `captures`"
    );
    assert_eq!(refused.errors.first.range.source, SourceId(0));
    assert_eq!(source_slice(source, refused.errors.first.range), "captures");
    let later = source
        .rfind("  captures:")
        .expect("the frontmatter mapping spells the key twice")
        + "  ".len();
    assert_eq!(
        refused.errors.first.range.range,
        TextRange {
            start: ByteOffset(later),
            end: ByteOffset(later + "captures".len()),
        }
    );
}

// ---------------------------------------------------------------------------
// Capture paths and the forbidden-policy conflict
// ---------------------------------------------------------------------------

/// A schema whose one capture declares `path`, spelled so that YAML delivers
/// the query verbatim however many backslashes and quotes it contains.
fn capture_path_source(path: &str) -> String {
    format!(
        concat!(
            "version: 1\n",
            "frontmatter:\n",
            "  captures:\n",
            "    version:\n",
            "      type: text\n",
            "      path: {}\n",
            "sections: []\n",
        ),
        serde_json::to_string(path).expect("a path serializes as a quoted scalar")
    )
}

/// §2.3 admits exactly RFC 9535 §2.3.5.1's name and index segments, and the
/// declaration keeps the spelling the author used rather than a rewritten
/// one: diagnostics quote `path` back.
#[test]
fn accepts_absolute_singular_capture_paths() {
    for path in [
        "$",
        "$.version",
        "$['version']",
        r#"$["version"]"#,
        "$.release[0]['date']",
        "$[-1]",
        "$ .release ['date'] [0]",
        // §4.6's I-JSON exact range, at both ends.
        "$[9007199254740991]",
        "$[-9007199254740991]",
        // A BMP escape and a literal astral character; see the provider
        // boundary pinned below for the spelling that is refused.
        r"$['\u00e4']",
        "$['ä']",
        "$['😀']",
    ] {
        let source = capture_path_source(path);
        let schema = valid(&source);
        let captures = schema.frontmatter.captures();
        let (_, capture) = captures
            .iter()
            .next()
            .expect("the declaration normalized its path");
        assert_eq!(capture.path_source(), path, "for `{path}`");
    }
}

/// The one error a capture path is expected to produce, with its anchor.
fn capture_path_error(path: &str) -> (String, String) {
    let source = capture_path_source(path);
    let refused = invalid(&source);
    assert!(
        refused.errors.rest.is_empty(),
        "expected one error for `{path}`, got {:#?}",
        refused.errors
    );
    assert_eq!(
        refused.errors.first.kind,
        SchemaErrorKind::InvalidCapture,
        "for `{path}`"
    );
    assert_eq!(refused.errors.first.range.source, SourceId(0));
    assert_eq!(
        source_slice(&source, refused.errors.first.range),
        format!(
            "version:\n      type: text\n      path: {}\n",
            serde_json::to_string(path).expect("a path serializes")
        ),
        "for `{path}`"
    );
    (
        refused.errors.first.message.clone(),
        source_slice(&source, refused.errors.first.range).to_owned(),
    )
}

/// §2.3: "A relative, `@`-rooted query is `invalid-capture` because this
/// binding site supplies no current node." A bare or dot-led name is refused
/// for the same reason, and the message says which reason it was.
#[test]
fn rejects_relative_and_at_rooted_capture_paths() {
    for path in ["@", "@.version", ".version", "version", ""] {
        let (message, _) = capture_path_error(path);
        assert_eq!(
            message,
            format!(
                "frontmatter capture `version` path `{path}` is not an absolute singular \
                 JSONPath query: a capture path must be `$`-rooted at offset 0"
            )
        );
    }
}

/// Everything RFC 9535 §2.3.5.1 leaves out of a singular query. §2.3 gives a
/// capture "one absolute singular query", so a construct that could select
/// more than one node is refused at the declaration rather than resolved at
/// evaluation time.
#[test]
fn rejects_plural_capture_paths() {
    for path in [
        "$.*",
        "$[*]",
        "$['a','b']",
        "$[0,1]",
        "$[1:3]",
        "$..a",
        "$[?@.enabled]",
        "$.a[*].b",
    ] {
        let (message, _) = capture_path_error(path);
        assert!(
            message.starts_with(&format!(
                "frontmatter capture `version` path `{path}` is not an absolute singular \
                 JSONPath query: a capture path takes only name and index segments at offset "
            )),
            "for `{path}`: {message}"
        );
    }
}

/// A path that is not a JSONPath query at all is refused as one, so a
/// malformed spelling is never reported as "not singular".
#[test]
fn rejects_malformed_capture_paths() {
    for path in ["$.", "$[", "$['a", r"$['\q']", r"$['\u00']", "$.a extra"] {
        let (message, _) = capture_path_error(path);
        assert!(
            message.starts_with(&format!(
                "frontmatter capture `version` path `{path}` is not an absolute singular \
                 JSONPath query: not a valid JSONPath query"
            )),
            "for `{path}`: {message}"
        );
    }
}

/// §4.6 caps an index selector at the I-JSON exact range. One past either end
/// is refused while parsing the declaration, not deferred to evaluation.
#[test]
fn rejects_out_of_range_capture_indexes() {
    for path in ["$[9007199254740992]", "$[-9007199254740992]"] {
        let (message, _) = capture_path_error(path);
        assert!(
            message.contains("not a valid JSONPath query"),
            "for `{path}`: {message}"
        );
    }
}

/// A provider-boundary pin, not a portable conformance requirement.
///
/// RFC 9535's `hexchar` is case-insensitive, so a surrogate-pair escape names
/// one astral character however its hex digits are cased, and this crate's own
/// recognizer decodes either. The pinned `serde_json_path = 0.7.2` does not:
/// it admits a pair whose **high** surrogate is spelled in uppercase and
/// refuses the same pair spelled in lowercase, which is why `\uD83D\ude00`
/// loads here and `\ud83d\ude00` does not.
///
/// The loader inherits both answers deliberately. Decoding around the provider
/// to close the gap would leave a path admitted here that the evaluator — the
/// same provider — could not run. §2.3 admits the full RFC grammar, so this is
/// a gap between the spec and the provider rather than a choice Outlint gets
/// to make; a provider bump that closes it should update this test and the
/// loader behaviour it describes together. See the locator's own boundary
/// test, which pins the same limitation one layer down.
#[test]
fn the_provider_decides_which_surrogate_pair_escapes_a_capture_path_may_use() {
    let (message, _) = capture_path_error(r"$['\ud83d\ude00']");
    assert!(
        message.contains("not a valid JSONPath query"),
        "a lowercase high surrogate is refused by the provider: {message}"
    );

    // The literal character, an ordinary BMP escape, and the spellings whose
    // high surrogate is uppercase all reach the recognizer and load.
    for path in [
        "$['😀']",
        r"$['\u0041']",
        r"$['\uD83D\uDE00']",
        r"$['\uD83D\ude00']",
    ] {
        let schema = valid(&capture_path_source(path));
        let captures = schema.frontmatter.captures();
        let (_, capture) = captures.iter().next().expect("the path loaded");
        assert_eq!(capture.path_source(), path, "for `{path}`");
    }
}

/// §6.3: when `frontmatter.captures` conflicts with `frontmatter.allow:
/// false`, `conflicting-frontmatter` "anchors at whichever of those keys
/// occurs second". Both orders are spelled here, because an anchor that
/// always picked the same key would pass one of them by accident.
#[test]
fn capture_conflict_anchors_the_later_field() {
    let allow_first = concat!(
        "version: 1\n",
        "frontmatter:\n",
        "  allow: false\n",
        "  captures:\n",
        "    version:\n",
        "      type: semver\n",
        "sections: []\n",
    );
    let refused = invalid(allow_first);
    assert!(refused.errors.rest.is_empty());
    assert_eq!(
        refused.errors.first.kind,
        SchemaErrorKind::ConflictingFrontmatter
    );
    assert_eq!(
        refused.errors.first.message,
        "`frontmatter.captures` cannot be declared together with `frontmatter.allow`"
    );
    assert_eq!(
        source_slice(allow_first, refused.errors.first.range),
        "version:\n      type: semver\n"
    );
    let related = &refused.errors.first.related;
    assert_eq!(related.len(), 1);
    assert_eq!(related[0].message, "`frontmatter.allow` declared here");
    assert_eq!(source_slice(allow_first, related[0].range), "false");

    let captures_first = concat!(
        "version: 1\n",
        "frontmatter:\n",
        "  captures:\n",
        "    version:\n",
        "      type: semver\n",
        "  allow: false\n",
        "sections: []\n",
    );
    let refused = invalid(captures_first);
    assert!(refused.errors.rest.is_empty());
    assert_eq!(
        refused.errors.first.kind,
        SchemaErrorKind::ConflictingFrontmatter
    );
    assert_eq!(
        refused.errors.first.message,
        "`frontmatter.allow` cannot be declared together with `frontmatter.captures`"
    );
    assert_eq!(
        source_slice(captures_first, refused.errors.first.range),
        "false"
    );
    let related = &refused.errors.first.related;
    assert_eq!(related.len(), 1);
    assert_eq!(related[0].message, "`frontmatter.captures` declared here");
    // A mapping value runs to the start of the key that follows it, so the
    // captures mapping carries the indentation `allow` sits on.
    assert_eq!(
        source_slice(captures_first, related[0].range),
        "version:\n      type: semver\n  "
    );
}

/// The conflict is decided from the presence of the two keys, so it is
/// reported even when the declaration is also malformed — and the
/// declaration's own fault is reported alongside it, as §6.3 requires of
/// independent errors.
#[test]
fn collects_capture_conflict_and_declaration_errors() {
    let source = concat!(
        "version: 1\n",
        "frontmatter:\n",
        "  allow: false\n",
        "  captures:\n",
        "    version:\n",
        "      type: bogus\n",
        "sections: []\n",
    );
    let refused = invalid(source);
    assert_eq!(
        refused
            .errors
            .iter()
            .map(|error| error.kind)
            .collect::<Vec<_>>(),
        vec![
            SchemaErrorKind::ConflictingFrontmatter,
            SchemaErrorKind::InvalidCapture,
        ]
    );
    assert_eq!(
        source_slice(source, refused.errors.rest[0].range),
        "version:\n      type: bogus\n"
    );
}

/// §2.3: "`frontmatter.schema` and `frontmatter.captures` are complementary."
/// A normalized capture collection leaves inline and linked JSON Schema
/// preparation, its source attribution, and its document node untouched.
#[test]
fn json_schema_preparation_is_unchanged_by_normalized_captures() {
    let inline_source = concat!(
        "version: 1\n",
        "frontmatter:\n",
        "  captures:\n",
        "    version:\n",
        "      type: semver\n",
        "  schema:\n",
        "    type: object\n",
        "title: null\n",
        "sections: []\n",
    );
    let loaded = load_schema(inline_source).expect("an inline schema loads beside captures");
    let FrontmatterPolicy::OptionalWithCaptures {
        schema: Some(_),
        captures,
    } = &loaded.schema.frontmatter
    else {
        panic!("expected an optional policy carrying both a schema and captures")
    };
    assert_eq!(captures.len(), 1);
    assert_eq!(
        loaded.locations.nodes[&SchemaNode::FrontmatterSchemaDocument],
        loaded.locations.nodes[&SchemaNode::FrontmatterSchemaDeclaration]
    );

    let linked_source = concat!(
        "version: 1\n",
        "frontmatter:\n",
        "  required: true\n",
        "  captures:\n",
        "    version:\n",
        "      type: semver\n",
        "  schema: root.json\n",
        "title: null\n",
        "sections: []\n",
    );
    let loaded = load_schema_with_resources(
        linked_source,
        Some(SourceLabel("schema.yml".into())),
        Some(LinkedJsonSchemaInput {
            root_uri: "https://outlint.invalid/root.json".into(),
            resources: vec![resource(
                "https://outlint.invalid/root.json",
                r#"{"type":"object"}"#,
            )],
        }),
    )
    .expect("a linked schema loads beside captures");
    assert!(matches!(
        loaded.schema.frontmatter,
        FrontmatterPolicy::RequiredWithCaptures {
            schema: Some(_),
            ..
        }
    ));
    assert!(loaded.sources.documents.contains_key(&SourceId(1)));
    assert_eq!(
        loaded.locations.nodes[&SchemaNode::FrontmatterSchemaDocument].source,
        SourceId(1)
    );
}

/// The model has no forbidden-with-captures variant, and the loader has no
/// path to one: every spelling that would need it fails the load, while a
/// forbidden policy without captures still loads exactly as before.
#[test]
fn no_forbidden_policy_carries_captures() {
    for declaration in [
        "    version:\n      type: semver\n",
        "    version:\n      type: bogus\n",
        "    Version:\n      type: semver\n",
    ] {
        let source = format!(
            "version: 1\nfrontmatter:\n  allow: false\n  captures:\n{declaration}sections: []\n"
        );
        assert!(
            load_schema(&source).is_err(),
            "forbidden frontmatter must not carry captures: {declaration}"
        );
    }
    // An empty or null collection is refused for being one, and still never
    // produces a forbidden policy carrying captures.
    assert!(load_schema(
        "version: 1\nfrontmatter:\n  allow: false\n  captures: {}\nsections: []\n"
    )
    .is_err());

    let schema = valid("version: 1\nfrontmatter:\n  allow: false\nsections: []\n");
    assert_eq!(
        schema.frontmatter,
        FrontmatterPolicy::Forbidden { schema: None }
    );
    assert!(schema.frontmatter.is_forbidden());
    assert!(schema.frontmatter.captures().is_empty());
}
