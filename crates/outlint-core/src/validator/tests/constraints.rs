use crate::validator::constraints::frontmatter_satisfied;
use crate::validator::{validate, DiagnosticId, DiagnosticReference, DiagnosticTarget};
use crate::yaml::parse_frontmatter_scalar;
use crate::{load_schema, parse_markdown, DocumentFrontmatter, MarkdownOptions};

/// Builds an `fm.` reference the way the loader normalizes one: the
/// equality literal resolves through the shared core-schema resolver.
fn fm_reference(path: &[&str], equals: Option<&str>) -> crate::FrontmatterRef {
    let mut keys = path.iter();
    crate::FrontmatterRef {
        path: crate::NonEmpty {
            first: crate::FrontmatterKey(
                (*keys.next().expect("test paths are non-empty")).to_owned(),
            ),
            rest: keys
                .map(|key| crate::FrontmatterKey((*key).to_owned()))
                .collect(),
        },
        equals: equals.map(parse_frontmatter_scalar),
    }
}

/// Evaluates one `fm.` proposition against a Markdown document's parsed
/// frontmatter, typed by the real reader.
fn fm_satisfied(markdown: &str, path: &[&str], equals: Option<&str>, match_case: bool) -> bool {
    let document = parse_markdown(markdown, MarkdownOptions::default());
    let frontmatter = match &document.frontmatter {
        DocumentFrontmatter::Mapping { value, .. } => Some(value),
        DocumentFrontmatter::Absent | DocumentFrontmatter::Invalid { .. } => None,
    };
    frontmatter_satisfied(frontmatter, &fm_reference(path, equals), match_case)
}

#[test]
fn bare_frontmatter_refs_are_presence_of_a_non_null_value() {
    let document = "---\npresent: 1\nempty: null\nnested:\n  inner: yes\n---\n";
    assert!(fm_satisfied(document, &["present"], None, false));
    // A null value does not satisfy the bare form, and neither does a key
    // the frontmatter lacks.
    assert!(!fm_satisfied(document, &["empty"], None, false));
    assert!(!fm_satisfied(document, &["absent"], None, false));
    // Nested steps address nested mappings.
    assert!(fm_satisfied(document, &["nested", "inner"], None, false));
    assert!(!fm_satisfied(document, &["nested", "missing"], None, false));
    // A step into a non-mapping is unsatisfied, whatever the value is.
    assert!(!fm_satisfied(document, &["present", "deeper"], None, false));
    // A document with no frontmatter at all satisfies nothing.
    assert!(!fm_satisfied("# Title\n", &["present"], None, false));
}

#[test]
fn bare_refs_accept_collections_but_equality_refuses_them() {
    let document = "---\nitems:\n  - one\ntable:\n  key: value\n---\n";
    // The bare form is satisfied by any non-null value, collections
    // included; the `=` form compares scalars only.
    assert!(fm_satisfied(document, &["items"], None, false));
    assert!(fm_satisfied(document, &["table"], None, false));
    assert!(!fm_satisfied(document, &["items"], Some("one"), false));
    assert!(!fm_satisfied(document, &["table"], Some("value"), false));
    // Stepping through a sequence is unsatisfied: only mappings nest.
    assert!(!fm_satisfied(document, &["items", "one"], None, false));
}

#[test]
fn equality_is_typed_by_the_core_schema_resolver() {
    let document = "---\ncount: 1\nspelled: \"1\"\ndraft: true\nquoted: \"true\"\n---\n";
    assert!(fm_satisfied(document, &["count"], Some("1"), false));
    assert!(fm_satisfied(document, &["draft"], Some("true"), false));
    // There is no quoting in the ref literal: the quotes are characters
    // of the string, which the value `"1"` does not contain.
    assert!(!fm_satisfied(document, &["spelled"], Some("\"1\""), false));
    // The spec's three negative examples: no cross-type coercion.
    assert!(!fm_satisfied(document, &["spelled"], Some("1"), false));
    assert!(!fm_satisfied(document, &["quoted"], Some("true"), false));
    assert!(!fm_satisfied(document, &["count"], Some("1.0"), false));
    // Both sides canonicalize before comparing: spelling is irrelevant
    // within a type.
    let spellings = "---\nhex: 0x10\nfloat: 12.5\n---\n";
    assert!(fm_satisfied(spellings, &["hex"], Some("16"), false));
    assert!(fm_satisfied(spellings, &["float"], Some("1.25e1"), false));
    assert!(!fm_satisfied(spellings, &["hex"], Some("16.0"), false));
    // `=null` can never hold: a null value already fails the bare form.
    assert!(!fm_satisfied(
        "---\nempty: null\n---\n",
        &["empty"],
        Some("null"),
        false
    ));
}

#[test]
fn string_equality_follows_match_case_with_simple_folding() {
    let document = "---\nstatus: Deprecated\nfold: \u{17f}\n---\n";
    assert!(fm_satisfied(
        document,
        &["status"],
        Some("deprecated"),
        false
    ));
    assert!(!fm_satisfied(
        document,
        &["status"],
        Some("deprecated"),
        true
    ));
    assert!(fm_satisfied(
        document,
        &["status"],
        Some("Deprecated"),
        true
    ));
    // Unicode simple folding: `ſ` matches `S` only case-insensitively.
    assert!(fm_satisfied(document, &["fold"], Some("S"), false));
    assert!(!fm_satisfied(document, &["fold"], Some("S"), true));
}

#[test]
fn deep_nesting_resolves_one_mapping_per_step() {
    let document = "---\na:\n  b:\n    c:\n      d: leaf\n---\n";
    assert!(fm_satisfied(document, &["a", "b", "c", "d"], None, false));
    assert!(fm_satisfied(
        document,
        &["a", "b", "c", "d"],
        Some("leaf"),
        false
    ));
    assert!(!fm_satisfied(
        document,
        &["a", "b", "c", "d", "e"],
        None,
        false
    ));
    assert!(!fm_satisfied(
        document,
        &["a", "b", "c"],
        Some("leaf"),
        false
    ));
}

#[test]
fn frontmatter_constraints_fire_and_release_through_validation() {
    let loaded = load_schema(
        "version: 1\nsections:\n  - id: migration\n    match: Migration\n    \
         required: false\nconstraints:\n  - requires: { if: \"fm[$.status]=deprecated\", \
         then: migration }\n",
    )
    .expect("test schema is valid");

    let firing = parse_markdown(
        "---\nstatus: deprecated\n---\n# Doc\n",
        MarkdownOptions::default(),
    );
    let diagnostics = validate(&loaded.schema, &firing).expect("schema prepares");
    assert_eq!(diagnostics.len(), 1);
    let diagnostic = &diagnostics[0];
    assert_eq!(diagnostic.id, DiagnosticId::Requires);
    // The top-level sugar constraint binds the title's sections scope but
    // uses the single-h1 document voice; the frontmatter side is named
    // among the references.
    assert_eq!(diagnostic.target, DiagnosticTarget::Document);
    let DiagnosticReference::FrontmatterQuery(reference) = &diagnostic.references[0] else {
        panic!("expected a frontmatter query reference")
    };
    assert_eq!(reference.locator(), "fm[$.status]=deprecated");
    assert_eq!(reference.query(), "$.status");
    assert_eq!(
        reference.equals(),
        Some(&crate::FrontmatterScalar::String("deprecated".into()))
    );

    // Unsatisfied condition: nothing fires.
    let inert = parse_markdown(
        "---\nstatus: current\n---\n# Doc\n",
        MarkdownOptions::default(),
    );
    assert!(validate(&loaded.schema, &inert)
        .expect("schema prepares")
        .is_empty());

    // Satisfied consequence: nothing fires either.
    let satisfied = parse_markdown(
        "---\nstatus: deprecated\n---\n# Doc\n## Migration\n",
        MarkdownOptions::default(),
    );
    assert!(validate(&loaded.schema, &satisfied)
        .expect("schema prepares")
        .is_empty());
}

#[test]
fn frontmatter_queries_route_past_a_nested_rule_addressable_as_fm_x() {
    // A nested rule id `fm` with child `x` would make the rule path
    // `fm.x` spellable — but §4.1 reserves the leading name `fm` for §4.6's
    // frontmatter forms, so `fm[$.x]` reads the frontmatter and never the
    // rule forest, and the headers below cannot satisfy the condition.
    let loaded = load_schema(
        "version: 1\nsections:\n  - id: outer\n    match: Outer\n    required: false\n    \
         sections:\n      - id: fm\n        match: FM\n        required: false\n        \
         sections:\n          - id: x\n            match: X\n            required: false\n    \
         constraints:\n      - requires: { if: \"fm[$.x]=1\", then: \"fm[$.present]=1\" }\n",
    )
    .expect("only a top-level `fm` rule id is reserved");

    // Headers satisfy the rule path fm -> x in the constraint's scope; the
    // frontmatter key `x` is absent. Were the locator a rule locator, the
    // condition would hold and the unsatisfiable consequence would fire.
    let headers_only = parse_markdown(
        "# Doc\n## Outer\n### FM\n#### X\n",
        MarkdownOptions::default(),
    );
    assert!(validate(&loaded.schema, &headers_only)
        .expect("schema prepares")
        .is_empty());

    // The frontmatter key alone fires it, with no matching header in sight.
    let frontmatter_only = parse_markdown(
        "---\nx: 1\n---\n# Doc\n## Outer\n",
        MarkdownOptions::default(),
    );
    let diagnostics = validate(&loaded.schema, &frontmatter_only).expect("schema prepares");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].id, DiagnosticId::Requires);
}
