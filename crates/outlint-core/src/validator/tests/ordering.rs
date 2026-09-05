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

#[test]
fn a_scope_orders_its_rules_by_default() {
    // No constraint spelled: the `sections` list is the order. Under the
    // sugar's document voice the violation targets the document and is
    // attributed to the title node that owns the sections scope; the
    // message names the pair that broke.
    let schema = "version: 2\nsections:\n  - match: Overview\n  - match: Usage\n  - match: Notes\n";
    assert_eq!(
        ids_and_targets(schema, "# T\n## Overview\n## Usage\n## Notes\n"),
        []
    );
    let diagnostics = ordered_diagnostics(schema, "# T\n## Usage\n## Overview\n## Notes\n");
    assert_eq!(diagnostics.len(), 1);
    let diagnostic = &diagnostics[0];
    assert_eq!(diagnostic.target, DiagnosticTarget::Document);
    assert_eq!(diagnostic.schema_node, Some(SchemaNode::Title));
    assert_eq!(diagnostic.location.line, 1);
    assert!(diagnostic.references.is_empty());
    assert_eq!(
        diagnostic.message,
        "sections are out of the declared order: `Overview` must precede `Usage`"
    );
    // Involved headers are the two rules' occurrences in document order.
    assert_eq!(
        diagnostic
            .involved_headers
            .iter()
            .map(|header| header.path.clone())
            .collect::<Vec<_>>(),
        [
            HeaderPath(vec!["T".into(), "Usage".into()]),
            HeaderPath(vec!["T".into(), "Overview".into()]),
        ]
    );
}

#[test]
fn implicit_order_reports_each_broken_adjacent_pair() {
    // `last(A) < first(B)` over adjacent present rules, one diagnostic per
    // broken pair: a fully reversed list breaks every pair, while a
    // single displaced section breaks only the pairs around it.
    let schema = "version: 2\nsections:\n  - match: A\n  - match: B\n  - match: C\n";
    let reversed = ordered_diagnostics(schema, "# T\n## C\n## B\n## A\n");
    assert_eq!(
        reversed
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .collect::<Vec<_>>(),
        [
            "sections are out of the declared order: `A` must precede `B`",
            "sections are out of the declared order: `B` must precede `C`",
        ]
    );
    let displaced = ordered_diagnostics(schema, "# T\n## A\n## C\n## B\n");
    assert_eq!(displaced.len(), 1);
    assert_eq!(
        displaced[0].message,
        "sections are out of the declared order: `B` must precede `C`"
    );
}

#[test]
fn implicit_order_ignores_unmatched_and_denied_headers_and_absent_rules() {
    // Unmatched headers in an open scope are unconstrained by ordering; a
    // denied rule contributes no accepted occurrence to the order; an
    // absent optional rule is simply not among the present pairs.
    let schema = "version: 2\nsections:\n  - match: A\n  - match: B\n    required: false\n  - match: C\n  - match: X\n    allow: false\n";
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
    // Repeats of one rule may sit together but not straddle the next
    // rule's occurrences: every A precedes every B.
    let schema = "version: 2\nsections:\n  - match: \"A *\"\n  - match: \"B *\"\n";
    assert_eq!(
        ids_and_targets(schema, "# T\n## A 1\n## A 2\n## B 1\n## B 2\n"),
        []
    );
    assert_eq!(
        ids_and_targets(schema, "# T\n## A 1\n## B 1\n## A 2\n"),
        [(DiagnosticId::Ordered, DiagnosticTarget::Document)]
    );
}

#[test]
fn nested_and_outline_scopes_order_themselves_with_their_own_owners() {
    // A nested scope's violation targets the owning header and is
    // attributed to the owning rule; the general form's outline scope
    // targets the document and has no schema node, the root being
    // nobody's rule.
    let nested = "version: 2\nsections:\n  - match: Steps\n    sections:\n      - match: One\n      - match: Two\n";
    let diagnostics = ordered_diagnostics(nested, "# T\n## Steps\n### Two\n### One\n");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(
        diagnostics[0].target,
        DiagnosticTarget::Header(HeaderPath(vec!["T".into(), "Steps".into()]))
    );
    assert_eq!(
        diagnostics[0].schema_node,
        Some(SchemaNode::Rule(crate::RulePath {
            scope: ScopePath(Vec::new()),
            index: RuleIndex(0),
        }))
    );
    assert_eq!(diagnostics[0].location.line, 2);

    let outline = "version: 2\noutline:\n  - match: Intro\n  - match: Part\n";
    let diagnostics = ordered_diagnostics(outline, "# Part\n# Intro\n");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].target, DiagnosticTarget::Document);
    assert_eq!(diagnostics[0].schema_node, None);
}

#[test]
fn the_option_sets_the_default_and_a_rule_overrides_it_for_its_scope() {
    let unordered =
        "version: 2\noptions:\n  ordered_sections: false\nsections:\n  - match: A\n  - match: B\n";
    assert_eq!(ids_and_targets(unordered, "# T\n## B\n## A\n"), []);

    // The rule's own `ordered` wins in both directions.
    let opted_in = "version: 2\noptions:\n  ordered_sections: false\nsections:\n  - match: S\n    ordered: true\n    sections:\n      - match: A\n      - match: B\n";
    assert_eq!(
        ids_and_targets(opted_in, "# T\n## S\n### B\n### A\n"),
        [(
            DiagnosticId::Ordered,
            DiagnosticTarget::Header(HeaderPath(vec!["T".into(), "S".into()])),
        )]
    );
    let opted_out = "version: 2\nsections:\n  - match: S\n    ordered: false\n    sections:\n      - match: A\n      - match: B\n";
    assert_eq!(ids_and_targets(opted_out, "# T\n## S\n### B\n### A\n"), []);
}

#[test]
fn implicit_order_binds_per_instance_and_speaks_for_each_owner() {
    // Two h1s under the sugar bind two instances; each is compared on
    // its own and names its owning h1, since the document voice would
    // otherwise emit indistinguishable duplicates.
    let schema = "version: 2\nsections:\n  - match: A\n  - match: B\n";
    let diagnostics = ordered_diagnostics(schema, "# One\n## A\n## B\n# Two\n## B\n## A\n");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(
        diagnostics[0].target,
        DiagnosticTarget::Header(HeaderPath(vec!["Two".into()]))
    );
    assert_eq!(diagnostics[0].location.line, 4);
}

#[test]
fn implicit_order_is_suppressible_at_the_owning_header() {
    let schema =
        "version: 2\nsections:\n  - match: S\n    sections:\n      - match: A\n      - match: B\n";
    assert_eq!(
        ids_and_targets(
            schema,
            "# T\n<!-- outlint-disable ordered -->\n## S\n### B\n### A\n"
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
    let schema = "version: 2\noptions:\n  ordered_sections: false\nsections:\n  - id: a\n    match: \"A *\"\n  - id: b\n    match: \"B *\"\nconstraints:\n  - ordered: [a, b]\n";
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
    let reversed = "version: 2\noptions:\n  ordered_sections: false\nsections:\n  - id: a\n    match: A\n  - id: b\n    match: B\nconstraints:\n  - ordered: [b, a]\n";
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
    let schema = "version: 2\noptions:\n  ordered_sections: false\nsections:\n  - id: intro\n    match: Intro\n  - id: body\n    match: Body\nconstraints:\n  - ordered: [intro, body]\n";
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
    let schema = "version: 2\noptions:\n  ordered_sections: false\noutline:\n  - id: guide\n    match: Guide\n    required: true\n  - id: appendix\n    match: Appendix\n    repeat: \"0..1\"\nconstraints:\n  - ordered: [guide, appendix]\n";
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
