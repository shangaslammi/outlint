use super::{error_kinds, invalid, source_slice, valid};
use crate::CanonicalInteger;
use crate::{
    Constraint, FrontmatterKey, FrontmatterRef, FrontmatterScalar, NonEmpty, Proposition,
    SchemaErrorKind,
};

#[test]
fn resolves_constraints_and_normalizes_frontmatter_scalars() {
    let schema = valid(
        r#"
version: 1
sections:
  - match: Overview
    sections:
      - match: Goals
  - match: Deployment
constraints:
  - requires: { if: deployment, then: [$.overview.goals, fm.count=0x10] }
"#,
    );
    let Constraint::Requires { consequences, .. } = &schema.outline[0].constraints[0] else {
        panic!("expected requires")
    };
    assert_eq!(
        consequences.rest[0],
        Proposition::Frontmatter(FrontmatterRef {
            path: NonEmpty {
                first: FrontmatterKey("count".into()),
                rest: vec![]
            },
            equals: Some(FrontmatterScalar::Integer(CanonicalInteger("16".into())))
        })
    );
}

#[test]
fn frontmatter_ref_identity_uses_simple_case_folding() {
    let duplicate = error_kinds(
        r#"
version: 1
sections: []
constraints:
  - any_of: [fm.key=ſ, fm.key=S]
"#,
    );
    assert_eq!(duplicate, vec![SchemaErrorKind::DuplicateRef]);

    let schema = valid(
        r#"
version: 1
sections: []
constraints:
  - any_of: [fm.key=ß, fm.key=ss]
"#,
    );
    assert_eq!(schema.outline[0].constraints.len(), 1);
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
            SchemaErrorKind::UnresolvedRef,
            SchemaErrorKind::UnresolvedRef
        ]
    );
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
