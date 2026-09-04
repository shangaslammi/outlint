use crate::locator::{parse_locator, ParsedLocator};
use crate::validator::constraints::frontmatter_query_satisfied;
use crate::validator::{validate, DiagnosticId, DiagnosticReference, DiagnosticTarget};
use crate::yaml::parse_frontmatter_scalar;
use crate::{
    load_schema, parse_markdown, DocumentFrontmatter, MarkdownOptions, ResolvedFrontmatterQuery,
};

/// Normalizes one `fm[...]` locator the way the loader does: the query source
/// is retained as written, and the equality remainder, when there is one,
/// resolves through the shared YAML core-schema resolver.
fn fm_query(locator: &str) -> ResolvedFrontmatterQuery {
    let Ok(ParsedLocator::FrontmatterQuery(parsed)) = parse_locator(locator) else {
        panic!("`{locator}` must parse as a frontmatter query")
    };
    let equals = parsed.equality().map(parse_frontmatter_scalar);
    ResolvedFrontmatterQuery::new(parsed, equals)
}

/// Evaluates one `fm[...]` proposition against a Markdown document's parsed
/// frontmatter, typed by the real reader.
fn fm_satisfied(markdown: &str, locator: &str, match_case: bool) -> bool {
    let document = parse_markdown(markdown, MarkdownOptions::default());
    let frontmatter = match &document.frontmatter {
        DocumentFrontmatter::Mapping { value, .. } => Some(value),
        DocumentFrontmatter::Absent | DocumentFrontmatter::Invalid { .. } => None,
    };
    frontmatter_query_satisfied(frontmatter, &fm_query(locator), match_case)
}

#[test]
fn a_bare_query_is_a_typed_boolean_read() {
    // §4.6: "A bare `fm[...]` is a typed boolean read, not a presence test. It
    // is satisfied iff at least one result node is the YAML/JSON boolean
    // `true`. Boolean `false`, an empty result, and null are unsatisfied."
    let document = "---\nyes: true\nno: false\nempty: null\nnested:\n  inner: true\n---\n";
    assert!(fm_satisfied(document, "fm[$.yes]", false));
    assert!(!fm_satisfied(document, "fm[$.no]", false));
    assert!(!fm_satisfied(document, "fm[$.empty]", false));
    assert!(!fm_satisfied(document, "fm[$.absent]", false));
    assert!(fm_satisfied(document, "fm[$.nested.inner]", false));
    // A document with no frontmatter at all satisfies nothing.
    assert!(!fm_satisfied("# Title\n", "fm[$.yes]", false));
    // Existential over the node set: one `true` among several is enough.
    let several = "---\nflags:\n  - false\n  - true\n---\n";
    assert!(fm_satisfied(several, "fm[$.flags[*]]", false));
    // PHASE 4A DEBT: §4.6 says a non-boolean, non-null result node "produces
    // `invalid-value`, and the entire constraint containing the proposition is
    // suppressed". Neither exists yet, so such a node reads as unsatisfied
    // rather than suppressing anything. These two assertions pin the interim
    // answer, not the specified one, and must be rewritten when suppression
    // lands.
    let text = "---\nname: outlint\ncount: 1\n---\n";
    assert!(!fm_satisfied(text, "fm[$.name]", false));
    assert!(!fm_satisfied(text, "fm[$.count]", false));
}

#[test]
fn equality_refuses_collections_and_never_holds_against_null() {
    let document = "---\nitems:\n  - one\ntable:\n  key: value\nempty: null\n---\n";
    // §4.6: "mappings and sequences never equal the literal".
    assert!(!fm_satisfied(document, "fm[$.items]=one", false));
    assert!(!fm_satisfied(document, "fm[$.table]=value", false));
    // A selector applied to the wrong container kind selects nothing.
    assert!(!fm_satisfied(document, "fm[$.items.one]=x", false));
    // Equality reaches inside them by naming the member.
    assert!(fm_satisfied(document, "fm[$.items[0]]=one", false));
    assert!(fm_satisfied(document, "fm[$.table.key]=value", false));
    // §4.6: "`fm[query]=null` is always false", and equality is existential
    // over *non-null* nodes, so a null node satisfies nothing.
    assert!(!fm_satisfied(document, "fm[$.empty]=null", false));
    assert!(!fm_satisfied(document, "fm[$.table.key]=null", false));
    // §4.6: "a result set `[null, \"x\"]` satisfies `=\"x\"`".
    let mixed = "---\nvalues:\n  - null\n  - x\n---\n";
    assert!(fm_satisfied(mixed, "fm[$.values[*]]=x", false));
}

#[test]
fn equality_is_typed_by_the_core_schema_resolver() {
    let document = "---\ncount: 1\nspelled: \"1\"\ndraft: true\nquoted: \"true\"\n---\n";
    assert!(fm_satisfied(document, "fm[$.count]=1", false));
    assert!(fm_satisfied(document, "fm[$.draft]=true", false));
    // There is no quoting in the literal: the quotes are characters of the
    // string, which the value `"1"` does not contain.
    assert!(!fm_satisfied(document, "fm[$.spelled]=\"1\"", false));
    // The spec's three negative examples: no cross-type coercion.
    assert!(!fm_satisfied(document, "fm[$.spelled]=1", false));
    assert!(!fm_satisfied(document, "fm[$.quoted]=true", false));
    assert!(!fm_satisfied(document, "fm[$.count]=1.0", false));
    // Both sides canonicalize before comparing: spelling is irrelevant within
    // a type.
    let spellings = "---\nhex: 0x10\nfloat: 12.5\n---\n";
    assert!(fm_satisfied(spellings, "fm[$.hex]=16", false));
    assert!(fm_satisfied(spellings, "fm[$.float]=1.25e1", false));
    assert!(!fm_satisfied(spellings, "fm[$.hex]=16.0", false));
}

#[test]
fn string_equality_follows_match_case_with_simple_folding() {
    let document = "---\nstatus: Deprecated\nfold: \u{17f}\n---\n";
    assert!(fm_satisfied(document, "fm[$.status]=deprecated", false));
    assert!(!fm_satisfied(document, "fm[$.status]=deprecated", true));
    assert!(fm_satisfied(document, "fm[$.status]=Deprecated", true));
    // Unicode simple folding: `ſ` matches `S` only case-insensitively.
    assert!(fm_satisfied(document, "fm[$.fold]=S", false));
    assert!(!fm_satisfied(document, "fm[$.fold]=S", true));
}

#[test]
fn a_query_resolves_one_member_per_segment() {
    let document = "---\na:\n  b:\n    c:\n      d: leaf\n---\n";
    assert!(fm_satisfied(document, "fm[$.a.b.c.d]=leaf", false));
    // A missing member selects nothing, and so does a step past a scalar.
    assert!(!fm_satisfied(document, "fm[$.a.b.c.d.e]=leaf", false));
    assert!(!fm_satisfied(document, "fm[$.a.b.missing]=leaf", false));
    // §4.6: a hyphenated member name needs bracket notation.
    let hyphenated = "---\ndecision-makers: ada\n---\n";
    assert!(fm_satisfied(
        hyphenated,
        "fm[$['decision-makers']]=ada",
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
