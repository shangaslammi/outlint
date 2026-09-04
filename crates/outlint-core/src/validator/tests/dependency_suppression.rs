//! §4.4 dependency suppression of constraints whose locator descent depends
//! on a cardinality that did not hold, and §5.3's whole-constraint tri-state.

use crate::validator::{Diagnostic, DiagnosticId};

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
