//! §4.4 dependency suppression of constraints whose locator descent depends
//! on a cardinality that did not hold, and §5.3's whole-constraint tri-state.

use crate::validator::{Diagnostic, DiagnosticId, DiagnosticReference, DiagnosticTarget};
use crate::{ConstraintIndex, ConstraintPath, SchemaNode, ScopePath};

use super::diagnostics;

/// An `outline` schema whose `part` rule is statically singular and whose
/// `goal` lives one step below it, plus two optional siblings to build
/// constraints out of. `constraints` is spelled by the caller.
fn descent_schema(constraints: &str) -> String {
    format!(
        "version: 1\noptions:\n  ordered_sections: false\noutline:\n  \
         - id: part\n    match: Part\n    required: true\n    ordered: false\n    \
         sections:\n      - id: goal\n        match: Goal\n        required: false\n      \
         - id: note\n        match: Note\n        required: false\n  \
         - id: extra\n    match: Extra\n    required: false\n  \
         - id: other\n    match: Other\n    required: false\nconstraints:\n{constraints}"
    )
}

fn ids(reported: &[Diagnostic]) -> Vec<DiagnosticId> {
    reported.iter().map(|diagnostic| diagnostic.id).collect()
}

/// The document with `ids` disabled file-wide, for the filtering variant of
/// every row: §6.3 decides dependency suppression before these comments
/// filter diagnostics.
fn file_disabled(ids: &str, markdown: &str) -> String {
    format!("<!-- outlint-disable-file {ids} -->\n\n{markdown}")
}

#[test]
fn an_unnarrowed_singular_step_suppresses_when_its_rule_matched_twice() {
    // §4.4: "an unnarrowed non-terminal locator step may be statically
    // singular because its rule has effective maximum one. If that rule
    // nevertheless matches several headers in a cardinality-violating
    // concrete scope, `too-many-sections` stands and every constraint
    // evaluation that depends on descending through that step is suppressed
    // in that scope; it emits no constraint diagnostic."
    let schema = descent_schema("  - requires: { if: extra, then: \"$.part.goal\" }\n");

    // One `part`, no `goal`: the descent is evaluable and the constraint
    // fires on its own terms.
    assert_eq!(
        ids(&diagnostics(&schema, "# Part\n# Extra\n")),
        [DiagnosticId::Requires]
    );
    // Zero `part`s: an empty terminal list is unsatisfied, not suppressed.
    assert_eq!(
        ids(&diagnostics(&schema, "# Extra\n")),
        [DiagnosticId::MissingSection, DiagnosticId::Requires]
    );
    // Two `part`s: the cardinality primary stands and the constraint that
    // depended on descending through it says nothing.
    let plural = "# Part\n# Part\n# Extra\n";
    assert_eq!(
        ids(&diagnostics(&schema, plural)),
        [DiagnosticId::TooManySections]
    );

    // Filtering variant: §6.3 — "suppressing `too-many-sections` never
    // re-enables a locator descent that depended on singularity".
    assert_eq!(
        ids(&diagnostics(
            &schema,
            &file_disabled("too-many-sections", plural)
        )),
        []
    );
}

#[test]
fn a_positional_step_descends_through_the_same_cardinality_violation() {
    // §4.4: "A step narrowed with `[i]` does not depend on the rule's
    // cardinality holding and remains evaluable."
    let first = descent_schema("  - requires: { if: extra, then: \"$.part[0].goal\" }\n");
    let second = descent_schema("  - requires: { if: extra, then: \"$.part[1].goal\" }\n");
    // The first `Part` has the `Goal`; the second — the occurrence in excess
    // of the bound — does not. Both descents happen all the same.
    let markdown = "# Part\n## Goal\n# Part\n# Extra\n";
    assert_eq!(
        ids(&diagnostics(&first, markdown)),
        [DiagnosticId::TooManySections]
    );
    assert_eq!(
        ids(&diagnostics(&second, markdown)),
        [DiagnosticId::TooManySections, DiagnosticId::Requires]
    );
    // §4.4: "An index beyond the end of a concrete node list selects nothing
    // and produces the empty list" — unsatisfied, and not suppressed.
    let third = descent_schema("  - requires: { if: extra, then: \"$.part[2].goal\" }\n");
    assert_eq!(
        ids(&diagnostics(&third, markdown)),
        [DiagnosticId::TooManySections, DiagnosticId::Requires]
    );
}

#[test]
fn a_plural_terminal_step_is_an_ordinary_presence_proposition() {
    // §4.4: "Only the terminal step may remain plural", and §4.5 makes such a
    // locator "satisfied iff its terminal node list is non-empty".
    let schema = "version: 1\noptions:\n  ordered_sections: false\noutline:\n  \
                  - id: part\n    match: Part\n    repeat: 0..n\n  \
                  - id: extra\n    match: Extra\n    required: false\n\
                  constraints:\n  - requires: { if: extra, then: part }\n";
    // A repeatable rule matched twice is simply present.
    assert_eq!(ids(&diagnostics(schema, "# Part\n# Part\n# Extra\n")), []);
    assert_eq!(
        ids(&diagnostics(schema, "# Extra\n")),
        [DiagnosticId::Requires]
    );
}

#[test]
fn an_unrelated_cardinality_failure_leaves_the_locator_alone() {
    // §4.4: dependency suppression "does not make every cardinality failure
    // suppress every later check". The rule in excess here is not on the
    // locator's path.
    let schema = "version: 1\noptions:\n  ordered_sections: false\noutline:\n  \
                  - id: part\n    match: Part\n    required: true\n    ordered: false\n    \
                  sections:\n      - id: goal\n        match: Goal\n        required: false\n  \
                  - id: extra\n    match: Extra\n    required: false\n  \
                  - id: other\n    match: Other\n    required: false\n\
                  constraints:\n  - requires: { if: extra, then: \"$.part.goal\" }\n";
    // `other` is spelled `required: false`, so a second `Other` breaks its
    // bound while `part` keeps its own.
    assert_eq!(
        ids(&diagnostics(schema, "# Part\n# Extra\n# Other\n# Other\n")),
        [DiagnosticId::TooManySections, DiagnosticId::Requires]
    );
}

#[test]
fn every_constraint_form_propagates_a_suppressed_operand() {
    // §5.3: "If evaluating any proposition is suppressed under §4.4 or §4.6,
    // the containing constraint produces no constraint diagnostic.
    // Suppression applies to the whole boolean constraint without
    // three-valued short-circuiting."
    //
    // Each row pairs a constraint that is violated when its `$.part.goal`
    // operand is evaluable with the same constraint when that operand is
    // suppressed. The document differs only in how many `Part`s it has.
    for (constraints, evaluable, plural, id) in [
        (
            "  - one_of: [extra, \"$.part.goal\"]\n",
            "# Part\n",
            "# Part\n# Part\n",
            DiagnosticId::OneOf,
        ),
        (
            "  - any_of: [extra, \"$.part.goal\"]\n",
            "# Part\n",
            "# Part\n# Part\n",
            DiagnosticId::AnyOf,
        ),
        (
            "  - at_most_one: [extra, other, \"$.part.goal\"]\n",
            "# Part\n# Extra\n# Other\n",
            "# Part\n# Part\n# Extra\n# Other\n",
            DiagnosticId::AtMostOne,
        ),
        (
            "  - all_or_none: [extra, \"$.part.goal\"]\n",
            "# Part\n# Extra\n",
            "# Part\n# Part\n# Extra\n",
            DiagnosticId::AllOrNone,
        ),
        (
            "  - requires: { if: extra, then: \"$.part.goal\" }\n",
            "# Part\n# Extra\n",
            "# Part\n# Part\n# Extra\n",
            DiagnosticId::Requires,
        ),
        (
            "  - conflicts: { if: extra, then_not: [other, \"$.part.goal\"] }\n",
            "# Part\n## Goal\n# Extra\n# Other\n",
            "# Part\n## Goal\n# Part\n# Extra\n# Other\n",
            DiagnosticId::Conflicts,
        ),
    ] {
        let schema = descent_schema(constraints);
        assert!(
            ids(&diagnostics(&schema, evaluable)).contains(&id),
            "{constraints} must fire when the descent is evaluable"
        );
        assert_eq!(
            ids(&diagnostics(&schema, plural)),
            [DiagnosticId::TooManySections],
            "{constraints} must be suppressed by the descent"
        );
        // Filtering variant: hiding the upstream primary changes nothing.
        assert_eq!(
            ids(&diagnostics(
                &schema,
                &file_disabled("too-many-sections", plural)
            )),
            [],
            "{constraints} stays suppressed with its primary hidden"
        );
    }
}

#[test]
fn ordered_suppresses_when_any_of_its_locator_descents_is_suppressed() {
    // §5.1's pairwise `last(A) < first(B)` over locators that each descend
    // through `part`. When that descent is suppressed the constraint has no
    // evaluable operand list at all, even though the first pair would fail.
    let schema = descent_schema("  - ordered: [\"$.part.goal\", \"$.part.note\"]\n");
    let out_of_order = "# Part\n## Note\n## Goal\n";
    assert_eq!(
        ids(&diagnostics(&schema, out_of_order)),
        [DiagnosticId::Ordered]
    );
    let plural = "# Part\n## Note\n## Goal\n# Part\n";
    assert_eq!(
        ids(&diagnostics(&schema, plural)),
        [DiagnosticId::TooManySections]
    );
    assert_eq!(
        ids(&diagnostics(
            &schema,
            &file_disabled("too-many-sections", plural)
        )),
        []
    );
}

// ---------------------------------------------------------------------------
// §4.6's `fm[...]` boolean read
// ---------------------------------------------------------------------------

/// A headless schema whose one constraint reads `query` under a condition the
/// document always satisfies, so the `requires` diagnostic reports the
/// query's truth.
fn query_schema(query: &str, frontmatter: &str) -> String {
    format!(
        "version: 1\ntitle: null\nsections:\n  - id: body\n    match: Body\n    \
         required: true\n{frontmatter}constraints:\n  - requires: {{ if: body, \
         then: \"{query}\" }}\n"
    )
}

fn query_ids(schema: &str, frontmatter: &str) -> Vec<DiagnosticId> {
    ids(&diagnostics(schema, &format!("{frontmatter}## Body\n")))
}

/// The JSON pointer a frontmatter diagnostic named, if it named one.
fn query_pointer(diagnostic: &Diagnostic) -> Option<&str> {
    match &diagnostic.target {
        DiagnosticTarget::Frontmatter { block: Some(block) } => block.json_pointer.as_deref(),
        other => panic!("expected a frontmatter target, got {other:?}"),
    }
}

#[test]
fn a_bare_read_is_satisfied_only_by_a_true_among_boolean_and_null_nodes() {
    // §4.6: "It is satisfied iff at least one result node is the YAML/JSON
    // boolean `true`. Boolean `false`, an empty result, and null are
    // unsatisfied."
    let schema = query_schema("fm[$.flags[*]]", "");
    assert_eq!(query_ids(&schema, "---\nflags: [false, true]\n---\n"), []);
    assert_eq!(
        query_ids(&schema, "---\nflags: [false, false]\n---\n"),
        [DiagnosticId::Requires]
    );
    assert_eq!(
        query_ids(&schema, "---\nflags: []\n---\n"),
        [DiagnosticId::Requires]
    );
    assert_eq!(
        query_ids(&schema, "---\nflags: [null, null]\n---\n"),
        [DiagnosticId::Requires]
    );
    assert_eq!(
        query_ids(&schema, "---\nother: 1\n---\n"),
        [DiagnosticId::Requires]
    );
}

#[test]
fn every_non_boolean_non_null_node_is_invalid_and_suppresses_the_constraint() {
    // §4.6: "Every non-boolean, non-null result node produces `invalid-value`,
    // and the entire constraint containing the proposition is suppressed; a
    // true sibling result or another already-true operand does not
    // short-circuit that suppression."
    let schema = query_schema("fm[$.flags[*]]", "");
    // One offending node, alone.
    assert_eq!(
        query_ids(&schema, "---\nflags: [\"text\"]\n---\n"),
        [DiagnosticId::InvalidValue]
    );
    // A true sibling neither hides it nor rescues the constraint.
    assert_eq!(
        query_ids(&schema, "---\nflags: [true, \"text\"]\n---\n"),
        [DiagnosticId::InvalidValue]
    );
    // One primary per distinct offending node, and each one about the node
    // it is about: §6.1 gives it "the failing value's pointer" and §6.2
    // anchors it at that entry, so three diagnostics that agreed on either
    // would be three copies of one complaint rather than three findings.
    let block = "---\nflags:\n  - 1\n  - \"text\"\n  - a: 1\n---\n";
    let reported = diagnostics(&schema, &format!("{block}## Body\n"));
    assert_eq!(
        ids(&reported),
        [
            DiagnosticId::InvalidValue,
            DiagnosticId::InvalidValue,
            DiagnosticId::InvalidValue
        ]
    );
    assert_eq!(
        reported
            .iter()
            .map(|diagnostic| (query_pointer(diagnostic), diagnostic.location.line))
            .collect::<Vec<_>>(),
        [
            (Some("/flags/0"), 3),
            (Some("/flags/1"), 4),
            (Some("/flags/2"), 5),
        ]
    );

    // Filtering variant: §6.3 — hiding the primary does not re-enable the
    // constraint that depended on it.
    let hidden = "---\nflags: [true, \"text\"]\n---\n<!-- outlint-disable-file invalid-value -->\n\n## Body\n";
    assert_eq!(ids(&diagnostics(&schema, hidden)), []);
}

#[test]
fn one_invalid_diagnostic_names_the_node_it_rejected() {
    let schema = query_schema("fm[$.a.b]", "");
    let reported = diagnostics(&schema, "---\na:\n  b: \"text\"\n---\n## Body\n");
    assert_eq!(ids(&reported), [DiagnosticId::InvalidValue]);
    let diagnostic = &reported[0];
    // §6.1: the target is the frontmatter with the failing value's pointer,
    // and §6.2 anchors it at that entry.
    match &diagnostic.target {
        DiagnosticTarget::Frontmatter { block: Some(block) } => {
            assert_eq!(block.json_pointer.as_deref(), Some("/a/b"));
        }
        other => panic!("expected a frontmatter target, got {other:?}"),
    }
    assert_eq!(diagnostic.location.line, 3);
    // §6.2: "an invalid boolean-read value is attributed to the constraint
    // containing the query".
    assert_eq!(
        diagnostic.schema_node,
        Some(SchemaNode::Constraint(ConstraintPath {
            scope: ScopePath(Vec::new()),
            index: ConstraintIndex(0),
        }))
    );
    assert!(
        diagnostic.message.contains("$.a.b") && diagnostic.message.contains("bool"),
        "{}",
        diagnostic.message
    );
}

#[test]
fn duplicate_references_to_one_node_are_collapsed() {
    // §4.6: "At the `fm[...]` boundary, duplicate references to the same
    // result node are collapsed", so one node is one diagnostic however many
    // selectors reached it.
    let schema = query_schema("fm[$['a','a']]", "");
    assert_eq!(
        query_ids(&schema, "---\na: \"text\"\n---\n"),
        [DiagnosticId::InvalidValue]
    );
}

#[test]
fn an_absent_block_is_unsatisfied_and_an_invalid_one_suppresses() {
    // §4.6: "If the block is `invalid-frontmatter`, the query is unevaluated
    // and the entire containing constraint is suppressed. If the block is
    // absent, the query produces an empty result: a bare boolean read is
    // unsatisfied, and an equality proposition is unsatisfied."
    for query in ["fm[$.draft]", "fm[$.draft]=yes"] {
        let optional = query_schema(query, "");
        assert_eq!(
            query_ids(&optional, ""),
            [DiagnosticId::Requires],
            "{query}"
        );
        // The policy separately requiring a block reports that, and does not
        // change the query's own answer.
        let required = query_schema(query, "frontmatter:\n  required: true\n");
        assert_eq!(
            query_ids(&required, ""),
            [DiagnosticId::MissingFrontmatter, DiagnosticId::Requires],
            "{query}"
        );
        // An invalid block leaves the query unevaluated.
        assert_eq!(
            query_ids(&optional, "---\n- not a mapping\n---\n"),
            [DiagnosticId::InvalidFrontmatter],
            "{query}"
        );
        // Filtering variant: hiding the block primary changes nothing.
        let hidden = "---\n- not a mapping\n---\n<!-- outlint-disable-file invalid-frontmatter -->\n\n## Body\n";
        assert_eq!(ids(&diagnostics(&optional, hidden)), [], "{query}");
    }
}

#[test]
fn equality_never_invalidates_a_node_and_keeps_its_typed_existential_reading() {
    // §4.6 gives the equality form no `invalid-value` at all: "mappings and
    // sequences never equal the literal", and a non-boolean scalar is simply
    // compared.
    let schema = query_schema("fm[$.status]=deprecated", "");
    assert_eq!(query_ids(&schema, "---\nstatus: deprecated\n---\n"), []);
    assert_eq!(
        query_ids(&schema, "---\nstatus: current\n---\n"),
        [DiagnosticId::Requires]
    );
    let collections = query_schema("fm[$.items]=one", "");
    assert_eq!(
        query_ids(&collections, "---\nitems: [one]\n---\n"),
        [DiagnosticId::Requires]
    );
    // §4.6: "String equality follows `options.match_case`".
    let sensitive = format!(
        "version: 1\noptions:\n  match_case: true\n{}",
        query_schema("fm[$.status]=deprecated", "")
            .strip_prefix("version: 1\n")
            .expect("the helper spells the version first")
    );
    assert_eq!(
        query_ids(&sensitive, "---\nstatus: Deprecated\n---\n"),
        [DiagnosticId::Requires]
    );
}

#[test]
fn a_decisive_earlier_operand_does_not_prevent_a_later_query_diagnostic() {
    // §5.3 forbids three-valued short-circuiting, and §4.6 says "another
    // already-true operand does not short-circuit that suppression". Each row
    // puts a decisive operand ahead of an invalid query and asserts both the
    // primary and the suppression.
    for constraints in [
        "  - any_of: [body, \"fm[$.flag]\"]\n",
        "  - at_most_one: [body, \"fm[$.flag]\"]\n",
        "  - one_of: [body, \"fm[$.flag]\"]\n",
        "  - all_or_none: [body, \"fm[$.flag]\"]\n",
        "  - requires: { if: missing, then: \"fm[$.flag]\" }\n",
        "  - conflicts: { if: missing, then_not: \"fm[$.flag]\" }\n",
    ] {
        let schema = format!(
            "version: 1\ntitle: null\nsections:\n  - id: body\n    match: Body\n    \
             required: true\n  - id: missing\n    match: Missing\n    \
             required: false\nconstraints:\n{constraints}"
        );
        assert_eq!(
            ids(&diagnostics(&schema, "---\nflag: \"text\"\n---\n## Body\n")),
            [DiagnosticId::InvalidValue],
            "{constraints}"
        );
    }
}

#[test]
fn an_already_suppressed_operand_does_not_prevent_a_later_query_diagnostic() {
    // §5.3 forbids three-valued short-circuiting in both directions. A
    // suppressed earlier operand already decides the constraint — nothing the
    // later operands say can change a suppressed answer — and it must no more
    // stop them being evaluated than a decisive true one does. §4.6's
    // primaries are what makes that observable: the invalid query node still
    // names itself, even though the constraint's answer was settled before it
    // was reached.
    let schema = descent_schema("  - any_of: [\"$.part.goal\", \"fm[$.flag]\"]\n");
    let markdown = "---\nflag: \"text\"\n---\n# Part\n# Part\n";
    let reported = diagnostics(&schema, markdown);
    assert_eq!(
        ids(&reported),
        [DiagnosticId::TooManySections, DiagnosticId::InvalidValue]
    );
    assert_eq!(query_pointer(&reported[1]), Some("/flag"));

    // The same constraint with its descent evaluable isolates what the case
    // above is about: the query primary is not what the suppression adds.
    assert_eq!(
        ids(&diagnostics(&schema, "---\nflag: \"text\"\n---\n# Part\n")),
        [DiagnosticId::InvalidValue]
    );

    // Filtering variant: §6.3 — hiding the upstream cardinality primary
    // leaves both the query primary and the suppression as they were. The
    // directive follows the block, since frontmatter starts at line one.
    let hidden = "---\nflag: \"text\"\n---\n\
                  <!-- outlint-disable-file too-many-sections -->\n\n# Part\n# Part\n";
    assert_eq!(
        ids(&diagnostics(&schema, hidden)),
        [DiagnosticId::InvalidValue]
    );
}

#[test]
fn an_unsatisfied_constraint_keeps_every_reference_in_declaration_order() {
    // §5.3 reports "the resolved locators with their matchers or frontmatter
    // declarations", and §11.3's three reference kinds all reach the spine.
    let schema = "version: 1\ntitle: null\nsections:\n  - id: body\n    match: Body\n    \
                  required: false\nfrontmatter:\n  captures:\n    v:\n      type: bool\n\
                  constraints:\n  - any_of: [body, \"fm[$.draft]=yes\", \"fm.v\"]\n";
    let reported = diagnostics(schema, "---\nv: false\n---\n");
    assert_eq!(ids(&reported), [DiagnosticId::AnyOf]);
    let references = &reported[0].references;
    assert_eq!(references.len(), 3);
    match &references[0] {
        DiagnosticReference::Rule { locator, .. } => assert_eq!(locator.locator(), "body"),
        other => panic!("expected a rule reference, got {other:?}"),
    }
    match &references[1] {
        DiagnosticReference::FrontmatterQuery(query) => {
            assert_eq!(query.locator(), "fm[$.draft]=yes");
            assert_eq!(query.query(), "$.draft");
        }
        other => panic!("expected a frontmatter query reference, got {other:?}"),
    }
    match &references[2] {
        DiagnosticReference::FrontmatterCapture(capture) => {
            assert_eq!(capture.locator(), "fm.v");
            assert_eq!(capture.name().as_str(), "v");
        }
        other => panic!("expected a frontmatter capture reference, got {other:?}"),
    }
}
