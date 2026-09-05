use crate::validator::{validate, Diagnostic, DiagnosticId, DiagnosticTarget, HeaderPath};
use crate::{
    load_schema, parse_markdown, ConstraintIndex, ConstraintPath, MarkdownOptions, RuleIndex,
    SchemaNode, ScopePath,
};

use super::ids_and_targets;

fn ordered_diagnostics(schema: &str, markdown: &str) -> Vec<Diagnostic> {
    let loaded = load_schema(schema).expect("test schema is valid");
    let document = parse_markdown(markdown, MarkdownOptions::default());
    validate(&loaded.schema, &document)
        .expect("schema prepares")
        .into_iter()
        .filter(|diagnostic| diagnostic.id == DiagnosticId::Ordered)
        .collect()
}

fn misplaced_diagnostics(schema: &str, markdown: &str) -> Vec<Diagnostic> {
    let loaded = load_schema(schema).expect("test schema is valid");
    let document = parse_markdown(markdown, MarkdownOptions::default());
    validate(&loaded.schema, &document)
        .expect("schema prepares")
        .into_iter()
        .filter(|diagnostic| diagnostic.id == DiagnosticId::MisplacedSection)
        .collect()
}

#[test]
fn a_scope_orders_its_rules_by_default() {
    // §3.2/§3.5: ordered assignment recovers an inverted heading as
    // `misplaced-section`; it does not synthesize v1's `ordered` constraint.
    let schema = "version: 2\nsections:\n  - match: Overview\n    required: false\n  - match: Usage\n    required: false\n  - match: Notes\n    required: false\n";
    assert_eq!(
        ids_and_targets(schema, "# T\n## Overview\n## Usage\n## Notes\n"),
        []
    );
    let diagnostics = misplaced_diagnostics(schema, "# T\n## Usage\n## Overview\n## Notes\n");
    assert_eq!(diagnostics.len(), 1);
    let diagnostic = &diagnostics[0];
    assert_eq!(
        diagnostic.target,
        DiagnosticTarget::Header(HeaderPath(vec!["T".into(), "Usage".into()]))
    );
    assert_eq!(diagnostic.schema_node, Some(SchemaNode::Title));
    assert_eq!(diagnostic.location.line, 2);
    assert!(diagnostic.references.is_empty());
    assert!(diagnostic.involved_headers.is_empty());
}

#[test]
fn implicit_order_reports_each_broken_adjacent_pair() {
    // §3.5 reports each heading left unassigned by canonical recovery.
    let schema = "version: 2\nsections:\n  - match: A\n    required: false\n  - match: B\n    required: false\n  - match: C\n    required: false\n";
    let reversed = misplaced_diagnostics(schema, "# T\n## C\n## B\n## A\n");
    assert_eq!(
        reversed
            .iter()
            .map(|diagnostic| diagnostic.location.line)
            .collect::<Vec<_>>(),
        [2, 3]
    );
    let displaced = misplaced_diagnostics(schema, "# T\n## A\n## C\n## B\n");
    assert_eq!(displaced.len(), 1);
    assert_eq!(displaced[0].location.line, 3);
}

#[test]
fn implicit_order_ignores_unmatched_and_denied_headers_and_absent_rules() {
    // §3.1: extras and guards are removed before ordered assignment.
    let schema = "version: 2\nextras: anywhere\nforbid_sections:\n  - match: X\nsections:\n  - match: A\n  - match: B\n    required: false\n  - match: C\n";
    assert_eq!(
        ids_and_targets(schema, "# T\n## Free\n## A\n## Free\n## C\n## Free\n"),
        []
    );
    assert_eq!(
        ids_and_targets(schema, "# T\n## X\n## A\n## C\n"),
        [(
            DiagnosticId::NotAllowed,
            DiagnosticTarget::Header(HeaderPath(vec!["T".into(), "X".into()])),
        )]
    );
}

#[test]
fn implicit_order_compares_all_occurrences_of_repeated_rules() {
    // §3.2 phases remain contiguous; recovery identifies the straddling
    // heading instead of emitting v1's aggregate order diagnostic.
    let schema = "version: 2\nsections:\n  - match: \"A *\"\n    repeat: 0..n\n  - match: \"B *\"\n    repeat: 0..n\n";
    assert_eq!(
        ids_and_targets(schema, "# T\n## A 1\n## A 2\n## B 1\n## B 2\n"),
        []
    );
    assert_eq!(
        ids_and_targets(schema, "# T\n## A 1\n## B 1\n## A 2\n"),
        [(
            DiagnosticId::MisplacedSection,
            DiagnosticTarget::Header(HeaderPath(vec!["T".into(), "B 1".into()])),
        )]
    );
}

#[test]
fn nested_and_outline_scopes_order_themselves_with_their_own_owners() {
    // §3.5: recovery diagnostics target the unassigned heading and carry the
    // concrete scope owner only as schema attribution.
    let nested = "version: 2\nsections:\n  - match: Steps\n    sections:\n      - match: One\n        required: false\n      - match: Two\n        required: false\n";
    let diagnostics = misplaced_diagnostics(nested, "# T\n## Steps\n### Two\n### One\n");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(
        diagnostics[0].target,
        DiagnosticTarget::Header(HeaderPath(vec!["T".into(), "Steps".into(), "Two".into()]))
    );
    assert_eq!(
        diagnostics[0].schema_node,
        Some(SchemaNode::Rule(crate::RulePath {
            scope: ScopePath(Vec::new()),
            index: RuleIndex(0),
        }))
    );
    assert_eq!(diagnostics[0].location.line, 3);

    let outline = "version: 2\noutline:\n  - match: Intro\n    required: false\n  - match: Part\n    required: false\n";
    let diagnostics = misplaced_diagnostics(outline, "# Part\n# Intro\n");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(
        diagnostics[0].target,
        DiagnosticTarget::Header(HeaderPath(vec!["Part".into()]))
    );
    assert_eq!(diagnostics[0].schema_node, None);
}

#[test]
fn unordered_is_local_and_each_undeclared_child_scope_remains_ordered() {
    // §3.4: `unordered` is local and never inherited.
    let unordered = "version: 2\nunordered: true\nsections:\n  - match: A\n  - match: B\n";
    assert_eq!(ids_and_targets(unordered, "# T\n## B\n## A\n"), []);

    let opted_in = "version: 2\nunordered: true\nsections:\n  - match: S\n    sections:\n      - match: A\n      - match: B\n";
    assert!(ids_and_targets(opted_in, "# T\n## S\n### B\n### A\n")
        .iter()
        .any(|(id, _)| *id == DiagnosticId::MisplacedSection));
    let opted_out = "version: 2\nsections:\n  - match: S\n    unordered: true\n    sections:\n      - match: A\n      - match: B\n";
    assert_eq!(ids_and_targets(opted_out, "# T\n## S\n### B\n### A\n"), []);
}

#[test]
fn implicit_order_binds_per_instance_and_speaks_for_each_owner() {
    // §3.1 evaluates each repeated owner's child scope independently.
    let schema = "version: 2\noutline:\n  - match: Part\n    repeat: 0..n\n    sections:\n      - match: A\n        required: false\n      - match: B\n        required: false\n";
    let diagnostics = misplaced_diagnostics(schema, "# Part\n## A\n## B\n# Part\n## B\n## A\n");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(
        diagnostics[0].target,
        DiagnosticTarget::Header(HeaderPath(vec!["Part".into(), "B".into()]))
    );
    assert_eq!(diagnostics[0].location.line, 5);
}

#[test]
fn implicit_order_is_suppressible_at_the_owning_header() {
    // §6.3: suppression filters the recovery diagnostic, not its assignment.
    let schema = "version: 2\nsections:\n  - match: S\n    sections:\n      - match: A\n        required: false\n      - match: B\n        required: false\n";
    assert_eq!(
        ids_and_targets(
            schema,
            "<!-- outlint-disable-file misplaced-section -->\n# T\n## S\n### B\n### A\n"
        ),
        []
    );
}

#[test]
fn explicit_ordered_compares_all_occurrences_of_repeated_refs() {
    // The constraint path resolves refs by id rather than walking rule
    // indices, so it is tested on its own: `last(A) < first(B)` over
    // every occurrence, in an unordered scope where only the constraint
    // speaks.
    let schema = "version: 2\nunordered: true\nsections:\n  - id: a\n    match: \"A *\"\n    repeat: 0..n\n  - id: b\n    match: \"B *\"\n    repeat: 0..n\nconstraints:\n  - ordered: [a, b]\n";
    assert_eq!(
        ids_and_targets(schema, "# T\n## A 1\n## A 2\n## B 1\n## B 2\n"),
        []
    );
    let diagnostics = ordered_diagnostics(schema, "# T\n## A 1\n## B 1\n## A 2\n");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].target, DiagnosticTarget::Document);
    assert_eq!(
        diagnostics[0].schema_node,
        Some(SchemaNode::Constraint(ConstraintPath {
            scope: ScopePath(Vec::new()),
            index: ConstraintIndex(0),
        }))
    );
    // The constraint form cites its refs, unlike the implicit form.
    assert_eq!(diagnostics[0].references.len(), 2);
    // Unordered scope: the reverse order is legal once the constraint
    // says so, which the implicit form could never express.
    let reversed = "version: 2\nunordered: true\nsections:\n  - id: a\n    match: A\n  - id: b\n    match: B\nconstraints:\n  - ordered: [b, a]\n";
    assert_eq!(ids_and_targets(reversed, "# T\n## B\n## A\n"), []);
    assert_eq!(
        ids_and_targets(reversed, "# T\n## A\n## B\n"),
        [(DiagnosticId::Ordered, DiagnosticTarget::Document)]
    );
}

#[test]
fn explicit_ordered_binds_per_instance_and_never_reaches_across_ancestors() {
    // Attached to the sugar `sections` scope, the constraint is evaluated
    // once per enclosing h1. An inversion inside one part fires and names
    // that part; the same pair split across two parts leaves each
    // instance holding one ref, vacuously satisfied.
    let schema = "version: 2\nunordered: true\nsections:\n  - id: intro\n    match: Intro\n  - id: body\n    match: Body\nconstraints:\n  - ordered: [intro, body]\n";
    let diagnostics = ordered_diagnostics(
        schema,
        "# One\n## Intro\n## Body\n# Two\n## Body\n## Intro\n",
    );
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(
        diagnostics[0].target,
        DiagnosticTarget::Header(HeaderPath(vec!["Two".into()]))
    );
    assert!(ordered_diagnostics(schema, "# Alpha\n## Body\n# Beta\n## Intro\n").is_empty());
}

#[test]
fn explicit_ordered_on_the_outline_root_targets_the_document() {
    let schema = "version: 2\nunordered: true\noutline:\n  - id: guide\n    match: Guide\n    required: true\n  - id: appendix\n    match: Appendix\n    repeat: \"0..1\"\nconstraints:\n  - ordered: [guide, appendix]\n";
    assert_eq!(ids_and_targets(schema, "# Guide\n# Appendix\n"), []);
    let diagnostics = ordered_diagnostics(schema, "# Appendix\n# Guide\n");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].target, DiagnosticTarget::Document);
    assert_eq!(diagnostics[0].location.line, 1);
    assert_eq!(
        diagnostics[0].schema_node,
        Some(SchemaNode::Constraint(ConstraintPath {
            scope: ScopePath(Vec::new()),
            index: ConstraintIndex(0),
        }))
    );
}
