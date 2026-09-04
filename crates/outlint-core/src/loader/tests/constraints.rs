use super::{error_kinds, invalid, source_slice, valid};
use crate::CanonicalInteger;
use crate::{
    Constraint, FrontmatterScalar, Proposition, RefAnchor, ResolvedRuleLocator, Schema,
    SchemaErrorKind,
};

/// The bound rule locator a root `any_of`'s first operand holds.
fn first_rule_locator(schema: &Schema) -> &ResolvedRuleLocator {
    let Constraint::AnyOf(items) = &schema.outline[0].constraints[0] else {
        panic!("expected any_of")
    };
    let Proposition::ResolvedRule(locator) = &items.first else {
        panic!("expected a bound rule proposition")
    };
    locator
}

#[test]
fn resolves_constraints_and_normalizes_frontmatter_scalars() {
    let schema = valid(
        r#"
version: 1
sections:
  - match: Overview
    required: false
    sections:
      - match: Goals
  - match: Deployment
constraints:
  - requires: { if: deployment, then: [$.overview.goals, "fm[$.count]=0x10"] }
"#,
    );
    let Constraint::Requires {
        condition,
        consequences,
    } = &schema.outline[0].constraints[0]
    else {
        panic!("expected requires")
    };
    // The outline operands are bound locators: each id resolved to a
    // structural index, and the spelling the author wrote is retained.
    let Proposition::ResolvedRule(locator) = condition else {
        panic!("expected a bound rule proposition")
    };
    assert_eq!(locator.locator(), "deployment");
    assert_eq!(locator.anchor(), RefAnchor::CurrentScope);
    assert_eq!(locator.steps().first.index().0, 1);
    let Proposition::ResolvedRule(nested) = &consequences.first else {
        panic!("expected a bound rule proposition")
    };
    assert_eq!(nested.locator(), "$.overview.goals");
    assert_eq!(nested.anchor(), RefAnchor::SchemaRoot);
    assert_eq!(
        nested
            .steps()
            .iter()
            .map(|step| step.id().as_str().to_owned())
            .collect::<Vec<_>>(),
        ["overview", "goals"]
    );
    // §4.6: the wrapper is retained whole for diagnostics, the query is kept
    // exactly as written, and the equality literal resolves as one YAML 1.2
    // core-schema scalar.
    let Proposition::FrontmatterQuery(query) = &consequences.rest[0] else {
        panic!("expected a frontmatter query proposition")
    };
    assert_eq!(query.locator(), "fm[$.count]=0x10");
    assert_eq!(query.query(), "$.count");
    assert_eq!(
        query.equals(),
        Some(&FrontmatterScalar::Integer(CanonicalInteger("16".into())))
    );
}

#[test]
fn query_equality_identity_uses_simple_case_folding() {
    // §5.4 compares equality literals "under §4.6", where string equality
    // follows `options.match_case`, so two literals that a document cannot
    // tell apart are one proposition.
    let duplicate = error_kinds(
        r#"
version: 1
sections: []
constraints:
  - any_of: ["fm[$.key]=ſ", "fm[$.key]=S"]
"#,
    );
    assert_eq!(duplicate, vec![SchemaErrorKind::DuplicateRef]);

    let schema = valid(
        r#"
version: 1
sections: []
constraints:
  - any_of: ["fm[$.key]=ß", "fm[$.key]=ss"]
"#,
    );
    assert_eq!(schema.outline[0].constraints.len(), 1);

    // With `match_case` on, the two spellings stay apart.
    let sensitive = valid(
        r#"
version: 1
options:
  match_case: true
sections: []
constraints:
  - any_of: ["fm[$.key]=ſ", "fm[$.key]=S"]
"#,
    );
    assert_eq!(sensitive.outline[0].constraints.len(), 1);
}

#[test]
fn query_sources_are_retained_and_compared_as_written() {
    // §4.6: "The wrapper ends after parsing that complete query, not at the
    // first `]` or `=` occurring inside it", so a quoted `]` or `=` reaches
    // the provider intact.
    let schema = valid(
        r#"
version: 1
sections: []
constraints:
  - any_of: ["fm[$['a]b']]", "fm[$['c=d']]=x"]
"#,
    );
    let Constraint::AnyOf(items) = &schema.outline[0].constraints[0] else {
        panic!("expected any_of")
    };
    let Proposition::FrontmatterQuery(bracketed) = &items.first else {
        panic!("expected a frontmatter query proposition")
    };
    assert_eq!(bracketed.query(), "$['a]b']");
    assert_eq!(bracketed.equals(), None);
    let Proposition::FrontmatterQuery(equality) = &items.second else {
        panic!("expected a frontmatter query proposition")
    };
    assert_eq!(equality.query(), "$['c=d']");
    assert_eq!(
        equality.equals(),
        Some(&FrontmatterScalar::String("x".into()))
    );

    // §5.4: "Syntactically different JSONPath queries are not treated as
    // duplicates merely because they may select the same nodes."
    valid(
        r#"
version: 1
sections: []
constraints:
  - any_of: ["fm[$.a]", "fm[$['a']]"]
"#,
    );
    // The same source twice is one proposition, though.
    assert_eq!(
        error_kinds(
            r#"
version: 1
sections: []
constraints:
  - any_of: ["fm[$.a]", "fm[$.a]"]
"#,
        ),
        vec![SchemaErrorKind::DuplicateRef]
    );
    // Canonically equal numeric literals duplicate; a differently typed one
    // does not.
    assert_eq!(
        error_kinds(
            r#"
version: 1
sections: []
constraints:
  - any_of: ["fm[$.a]=0x10", "fm[$.a]=16"]
"#,
        ),
        vec![SchemaErrorKind::DuplicateRef]
    );
    valid(
        r#"
version: 1
sections: []
constraints:
  - any_of: ["fm[$.a]=16", "fm[$.a]=16.0"]
"#,
    );
}

#[test]
fn a_bare_query_is_not_an_equality_against_the_empty_scalar() {
    // §5.4 duplicates two queries only when they "either both lack equality
    // or their equality literals resolve to values equal under §4.6", so a
    // bare read never duplicates an equality form.
    valid(
        r#"
version: 1
sections: []
constraints:
  - any_of: ["fm[$.a]", "fm[$.a]="]
"#,
    );
    // YAML 1.2's core schema resolves the empty scalar to null, so `=` and
    // `=null` are the same literal and do duplicate.
    assert_eq!(
        error_kinds(
            r#"
version: 1
sections: []
constraints:
  - any_of: ["fm[$.a]=", "fm[$.a]=null"]
"#,
        ),
        vec![SchemaErrorKind::DuplicateRef]
    );
}

#[test]
fn the_retired_dotted_frontmatter_spelling_is_invalid_syntax() {
    // §10 migrates `fm.status=deprecated` to `fm[$.status]=deprecated`; the
    // old spelling is now `fm.` followed by something that is not one capture
    // name, which §4.4 makes `invalid-document-shape`.
    assert_eq!(
        error_kinds(
            "version: 1\nsections:\n  - id: a\n    match: A\nconstraints:\n  \
             - any_of: [fm.status=deprecated, a]\n",
        ),
        vec![SchemaErrorKind::InvalidDocumentShape]
    );
    // A dotted key path is not a capture name either.
    assert_eq!(
        error_kinds(
            "version: 1\nsections:\n  - id: a\n    match: A\nconstraints:\n  \
             - any_of: [fm.outer.inner, a]\n",
        ),
        vec![SchemaErrorKind::InvalidDocumentShape]
    );
}

#[test]
fn an_undeclared_frontmatter_capture_does_not_resolve() {
    // §4.6: "Unknown capture names are `unresolved-ref`, even if a YAML key
    // of the same name exists."
    assert_eq!(
        error_kinds(
            "version: 1\nsections:\n  - id: a\n    match: A\nconstraints:\n  \
             - any_of: [fm.version, a]\n",
        ),
        vec![SchemaErrorKind::UnresolvedRef]
    );
}

#[test]
fn rejects_dangling_forbidden_duplicate_and_mis_scoped_ordered_refs() {
    let kinds = error_kinds(
        r#"
version: 1
sections:
  - id: repeated
    match: Repeated
    sections:
      - match: Child
  - id: denied
    match: Denied
    allow: false
constraints:
  - any_of: [missing, missing]
  - requires: { if: denied, then: denied }
  - ordered: [repeated.child, denied]
"#,
    );
    assert!(kinds.contains(&SchemaErrorKind::UnresolvedRef));
    assert!(kinds.contains(&SchemaErrorKind::ForbiddenRef));
    assert!(kinds.contains(&SchemaErrorKind::OrderedScopeMismatch));
}

#[test]
fn checks_constraint_lexemes_even_when_a_rule_cannot_be_built() {
    // §4.4 makes invalid locator syntax `invalid-document-shape`, and syntax
    // is the only question answerable with no schema to bind against.
    let kinds = error_kinds(
        r#"
version: 1
sections:
  - match: /(?=invalid)/
constraints:
  - any_of: [bad..ref, also..bad]
"#,
    );
    assert_eq!(
        kinds,
        vec![
            SchemaErrorKind::InvalidMatcher,
            SchemaErrorKind::InvalidDocumentShape,
            SchemaErrorKind::InvalidDocumentShape
        ]
    );
    // Neither an unbindable name nor a duplicate identity is decidable
    // without a schema, so the lexical pass claims neither: §4.4 puts both
    // behind binding, and the only error left here is the matcher's.
    let quiet = error_kinds(
        r#"
version: 1
sections:
  - match: /(?=invalid)/
constraints:
  - any_of: [nowhere, nowhere]
"#,
    );
    assert_eq!(quiet, vec![SchemaErrorKind::InvalidMatcher]);
}

#[test]
fn rejects_implication_objects_with_the_wrong_keys() {
    let kinds = error_kinds(
        r#"
version: 1
sections: []
constraints:
  - requires: { condition: foo, consequence: bar }
"#,
    );
    assert!(kinds.contains(&SchemaErrorKind::InvalidDocumentShape));
}

#[test]
fn unknown_and_reserved_keywords_are_refused_as_shape() {
    // §5.5: reserving a word does not activate syntax, so every reserved
    // spelling arrives as an ordinary unknown keyword.
    for keyword in [
        "equal-values",
        "subset-values",
        "select",
        "sequence",
        "numbered",
    ] {
        let kinds = error_kinds(&format!(
            "version: 1\nsections:\n  - id: a\n    match: A\n  - id: b\n    match: B\n\
             constraints:\n  - {keyword}: [a, b]\n"
        ));
        assert_eq!(
            kinds,
            vec![SchemaErrorKind::InvalidDocumentShape],
            "`{keyword}` must be an unknown keyword"
        );
    }
}

#[test]
fn relative_and_absolute_spellings_of_one_rule_duplicate() {
    // §5.4: outline locators "duplicate when they resolve to the same
    // declared rule steps with the same positional subscripts", whichever
    // anchor reached them.
    let kinds = error_kinds(
        "version: 1\nsections:\n  - id: a\n    match: A\nconstraints:\n  - any_of: [a, \"$.a\"]\n",
    );
    assert_eq!(kinds, vec![SchemaErrorKind::DuplicateRef]);
    // A subscript is part of that identity, so two different ones do not
    // duplicate, and a subscripted spelling does not duplicate a bare one.
    valid(
        "version: 1\nsections:\n  - id: a\n    match: A\nconstraints:\n  \
         - any_of: [\"a[0]\", \"a[1]\"]\n",
    );
    valid(
        "version: 1\nsections:\n  - id: a\n    match: A\nconstraints:\n  \
         - any_of: [a, \"a[0]\"]\n",
    );
    // Duplicate detection spans a whole implication, `if` included.
    let across = error_kinds(
        "version: 1\nsections:\n  - id: a\n    match: A\n  - id: b\n    match: B\nconstraints:\n  \
         - requires: { if: a, then: [b, \"$.a\"] }\n",
    );
    assert_eq!(across, vec![SchemaErrorKind::DuplicateRef]);
}

#[test]
fn a_name_step_resolves_only_in_its_own_named_scope() {
    // §4.4: "A name step resolves only in the current named scope. There is
    // no implicit upward or downward search."
    let upward = error_kinds(
        "version: 1\nsections:\n  - id: outer\n    match: Outer\n    required: false\n    \
         sections:\n      - id: inner\n        match: Inner\n    constraints:\n      \
         - any_of: [inner, outer]\n",
    );
    assert_eq!(upward, vec![SchemaErrorKind::UnresolvedRef]);
    let downward = error_kinds(
        "version: 1\nsections:\n  - id: outer\n    match: Outer\n    required: false\n    \
         sections:\n      - id: inner\n        match: Inner\nconstraints:\n  \
         - any_of: [inner, outer]\n",
    );
    assert_eq!(downward, vec![SchemaErrorKind::UnresolvedRef]);
}

#[test]
fn a_position_keeps_arbitrary_precision() {
    // §4.4 gives `i` "no upper bound" and forbids work proportional to its
    // value, so the digits survive binding without meeting a machine integer.
    let digits = "1".repeat(400);
    let schema = valid(&format!(
        "version: 1\nsections:\n  - id: a\n    match: A\n  - id: b\n    match: B\nconstraints:\n  \
         - any_of: [\"a[{digits}]\", b]\n"
    ));
    assert_eq!(
        first_rule_locator(&schema).steps().first.position_digits(),
        Some(digits)
    );
}

#[test]
fn only_the_terminal_step_may_stay_plural() {
    // §4.4: "Every non-terminal step MUST be singular [...] Only the terminal
    // step may remain plural"; `[i]` makes any step singular.
    let source = "version: 1\nsections:\n  - id: many\n    match: M\n    sections:\n      \
                  - id: kid\n        match: K\n  - id: other\n    match: O\nconstraints:\n  \
                  - any_of: [{locator}, other]\n";
    assert_eq!(
        error_kinds(&source.replace("{locator}", "many.kid")),
        vec![SchemaErrorKind::InvalidDocumentShape]
    );
    valid(&source.replace("{locator}", "\"many[0].kid\""));
    // A terminal repeated rule is legal on its own.
    valid(&source.replace("{locator}", "many"));
    // A singular ancestor needs no subscript.
    valid(
        "version: 1\nsections:\n  - id: one\n    match: M\n    required: false\n    sections:\n   \
         \x20  - id: kid\n        match: K\n  - id: other\n    match: O\nconstraints:\n  \
         - any_of: [one.kid, other]\n",
    );
}

#[test]
fn value_terminals_bind_but_are_not_propositions() {
    // §4.5: "Locators ending in a capture or intrinsic value are value
    // locators and are not propositions in this version." Reaching the
    // context error is itself proof that `/text` bound; `/label` never does.
    let bound = invalid(
        "version: 1\nsections:\n  - id: a\n    match: A\n    required: false\n  - id: b\n    \
         match: B\nconstraints:\n  - any_of: [a/text, b]\n",
    );
    assert_eq!(bound.errors.rest.len(), 0);
    assert_eq!(
        bound.errors.first.kind,
        SchemaErrorKind::InvalidDocumentShape
    );
    assert!(bound.errors.first.message.contains("`/text` intrinsic"));
    // §4.4: other structural kinds "remain unallocated", so they never bind.
    let label = error_kinds(
        "version: 1\nsections:\n  - id: a\n    match: A\n    required: false\n  - id: b\n    \
         match: B\nconstraints:\n  - any_of: [a/label, b]\n",
    );
    assert_eq!(label, vec![SchemaErrorKind::UnresolvedRef]);
    // The rule in front of `/text` is non-terminal and takes the singularity
    // check like any other, so a repeatable one is refused before the context.
    let plural = invalid(
        "version: 1\nsections:\n  - id: a\n    match: A\n  - id: b\n    match: B\n\
         constraints:\n  - any_of: [a/text, b]\n",
    );
    assert_eq!(
        plural.errors.first.kind,
        SchemaErrorKind::InvalidDocumentShape
    );
    assert!(plural.errors.first.message.contains("repeatable rule `a`"));
}

#[test]
fn ordered_refuses_value_terminals_with_its_own_error() {
    // §5.1: "Mixing scopes, terminating in a frontmatter or typed value, or
    // otherwise lacking header position is schema error
    // `ordered-scope-mismatch`."
    let text = error_kinds(
        "version: 1\noptions:\n  ordered_sections: false\nsections:\n  - id: a\n    match: A\n  \
         - id: b\n    match: B\nconstraints:\n  - ordered: [a/text, b]\n",
    );
    assert_eq!(text, vec![SchemaErrorKind::OrderedScopeMismatch]);
    let frontmatter = error_kinds(
        "version: 1\noptions:\n  ordered_sections: false\nsections:\n  - id: a\n    match: A\n  \
         - id: b\n    match: B\nconstraints:\n  - ordered: [\"fm[$.draft]\", b]\n",
    );
    assert_eq!(frontmatter, vec![SchemaErrorKind::OrderedScopeMismatch]);
}

#[test]
fn ordered_compares_concrete_parent_scopes() {
    let source = "version: 1\nsections:\n  - id: part\n    match: Part\n    ordered: false\n    \
                  sections:\n      - id: x\n        match: X\n      - id: y\n        match: Y\n\
                  constraints:\n  - ordered: [{first}, {second}]\n";
    // One concrete scope: the same occurrence of a repeatable ancestor.
    valid(
        &source
            .replace("{first}", "\"part[0].x\"")
            .replace("{second}", "\"part[0].y\""),
    );
    // Two occurrences are two scopes, and "before" has no meaning across them.
    assert_eq!(
        error_kinds(
            &source
                .replace("{first}", "\"part[0].x\"")
                .replace("{second}", "\"part[1].y\"")
        ),
        vec![SchemaErrorKind::OrderedScopeMismatch]
    );
    // §5.1: "a bare terminal rule MAY remain plural", and a terminal
    // subscript is welcome in an unordered scope.
    valid(
        "version: 1\nsections:\n  - id: part\n    match: Part\n    required: true\n    \
         ordered: false\n    sections:\n      - id: x\n        match: X\n      - id: y\n        \
         match: Y\n    constraints:\n      - ordered: [\"x[0]\", y]\n",
    );
    // The terminal subscript is part of the ordered identity too.
    assert_eq!(
        error_kinds(
            "version: 1\nsections:\n  - id: part\n    match: Part\n    required: true\n    \
             ordered: false\n    sections:\n      - id: x\n        match: X\n      - id: y\n      \
             \x20 match: Y\n    constraints:\n      - ordered: [\"x[0]\", \"x[0]\"]\n",
        ),
        vec![SchemaErrorKind::DuplicateRef]
    );
}

#[test]
fn an_explicit_ordered_constraint_over_an_ordered_scope_is_refused() {
    // Redundant or contradictory, the fix is the same: the message says
    // which knob to turn.
    let redundant = "version: 1\nsections:\n  - id: a\n    match: A\n  - id: b\n    match: B\nconstraints:\n  - ordered: [a, b]\n";
    let refused = invalid(redundant);
    let error = refused
        .errors
        .iter()
        .find(|error| error.kind == SchemaErrorKind::OrderedScopeMismatch)
        .expect("the ordered scope refuses the constraint");
    assert!(error.message.contains("already ordered by its rule list"));
    assert!(error.message.contains("`ordered: false`"));
    assert_eq!(
        source_slice(redundant, error.range).trim_end(),
        "ordered: [a, b]"
    );

    // The same refs are welcome once the scope is unordered — by the
    // option at the root, or by the owning rule one level down, whether
    // reached by bare ids or by a path from the root.
    valid("version: 1\noptions:\n  ordered_sections: false\nsections:\n  - id: a\n    match: A\n  - id: b\n    match: B\nconstraints:\n  - ordered: [b, a]\n");
    valid("version: 1\nsections:\n  - id: s\n    match: S\n    ordered: false\n    sections:\n      - id: a\n        match: A\n      - id: b\n        match: B\n    constraints:\n      - ordered: [b, a]\n");
    valid("version: 1\nsections:\n  - id: s\n    match: S\n    required: true\n    ordered: false\n    sections:\n      - id: a\n        match: A\n      - id: b\n        match: B\nconstraints:\n  - ordered: [s.b, s.a]\n");
    // A path into an ordered nested scope is refused like a bare ref.
    let nested = invalid("version: 1\noptions:\n  ordered_sections: false\nsections:\n  - id: s\n    match: S\n    required: true\n    ordered: true\n    sections:\n      - id: a\n        match: A\n      - id: b\n        match: B\nconstraints:\n  - ordered: [s.a, s.b]\n");
    assert!(nested
        .errors
        .iter()
        .any(|error| error.kind == SchemaErrorKind::OrderedScopeMismatch));
    // Mixed scopes are already refused; the redundancy check stays quiet.
    let mixed = invalid("version: 1\nsections:\n  - id: s\n    match: S\n    required: true\n    sections:\n      - id: a\n        match: A\n  - id: b\n    match: B\nconstraints:\n  - ordered: [s.a, b]\n");
    assert_eq!(
        mixed
            .errors
            .iter()
            .filter(|error| error.kind == SchemaErrorKind::OrderedScopeMismatch)
            .count(),
        1
    );
}
