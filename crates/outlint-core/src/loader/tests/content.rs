use super::{invalid, source_slice, valid};
use crate::{
    BlockMatcher, ContentRule, ContentScope, DocumentShape, ItemScope, ListKind, Matcher,
    SchemaErrorKind,
};

fn document_content(source: &str) -> ContentScope {
    let schema = valid(source);
    match schema.document {
        DocumentShape::Outline { content, .. } => content,
        DocumentShape::Title(title) => title.content().clone(),
    }
}

#[test]
fn content_and_item_forms_normalize() {
    let source = "version: 1\ncontent:\n  - block: p\n  - block: list\n    list_kind: any\n    required: false\n    items:\n      - match: Exact\n      - match: '*'\n        repeat: 0..n\n  - one_of:\n      - block: p\n      - block: list\n        list_kind: ordered\n    repeat: 2..3\noutline: []\n";
    let ContentScope::Declared(rules) = document_content(source) else {
        panic!("content is declared");
    };
    assert_eq!(rules.len(), 3);
    assert!(matches!(
        &rules[0],
        ContentRule::Paragraph { cardinality, .. }
            if cardinality.min() == 1 && cardinality.max() == crate::UpperBound::Bounded(1)
    ));
    let ContentRule::List {
        list_kind,
        items,
        cardinality,
        ..
    } = &rules[1]
    else {
        panic!("second rule is a list");
    };
    assert_eq!(*list_kind, None);
    assert_eq!(cardinality.min(), 0);
    let ItemScope::Declared(items) = items else {
        panic!("items are declared");
    };
    assert!(matches!(items[0].matcher, Matcher::Exact(_)));
    assert_eq!(items[0].cardinality.min(), 1);
    assert!(matches!(items[1].matcher, Matcher::Any));
    let ContentRule::OneOf {
        alternatives,
        cardinality,
        ..
    } = &rules[2]
    else {
        panic!("third rule is a choice");
    };
    assert_eq!(cardinality.min(), 2);
    assert!(matches!(alternatives.first, BlockMatcher::Paragraph));
    assert!(matches!(
        alternatives.second,
        BlockMatcher::List {
            list_kind: Some(ListKind::Ordered)
        }
    ));

    let nested = valid("version: 1\nsections:\n  - match: A\n    content:\n      - block: p\n");
    let rule = nested
        .outline()
        .first()
        .unwrap_or_else(|| panic!("one section rule was declared"));
    assert!(matches!(rule.content, ContentScope::Declared(ref rules) if rules.len() == 1));
}

#[test]
fn invalid_outer_form_skips_five_secondary_members() {
    let source = "version: 1\ncontent:\n  - block: p\n    one_of: nope\n    list_kind: 7\n    items: nope\n    id: BAD\n    required: nope\n    repeat: 3..1\noutline: []\n";
    let invalid = invalid(source);
    assert_eq!(invalid.errors.iter().count(), 1);
    assert_eq!(
        invalid.errors.first.kind,
        SchemaErrorKind::InvalidContentRule
    );
    assert_eq!(source_slice(source, invalid.errors.first.range), "block");
}

#[test]
fn invalid_outer_form_keeps_unknown_key_errors_independent() {
    let source = "version: 1\ncontent:\n  - block: p\n    one_of: []\n    deferred: false\n    items: nope\noutline: []\n";
    let invalid = invalid(source);
    let errors = invalid.errors.iter().collect::<Vec<_>>();
    assert_eq!(errors.len(), 2);
    assert_eq!(errors[0].kind, SchemaErrorKind::InvalidDocumentShape);
    assert_eq!(source_slice(source, errors[0].range), "deferred");
    assert_eq!(errors[1].kind, SchemaErrorKind::InvalidContentRule);
    assert_eq!(source_slice(source, errors[1].range), "block");
}

#[test]
fn content_rejection_lattice_has_one_error_per_member_occurrence() {
    let source = "version: 1\ncontent:\n  - block: p\n    list_kind: [bad]\n    items: {bad: value}\n    required: yes\n    repeat: 2..1\n    allow: true\n  - one_of:\n      - block: list\n        list_kind: [bad]\n      - block: p\n        items: ignored\n      - block: p\noutline: []\n";
    let rejected = invalid(source);
    let kinds = rejected
        .errors
        .iter()
        .map(|error| error.kind)
        .collect::<Vec<_>>();
    assert_eq!(
        kinds,
        vec![
            SchemaErrorKind::InvalidDocumentShape,
            SchemaErrorKind::InvalidContentRule,
            SchemaErrorKind::InvalidContentRule,
            SchemaErrorKind::InvalidDocumentShape,
            SchemaErrorKind::InvalidContentRule,
            SchemaErrorKind::InvalidContentRule,
        ]
    );
    let slices = rejected
        .errors
        .iter()
        .map(|error| source_slice(source, error.range))
        .collect::<Vec<_>>();
    assert_eq!(
        slices,
        vec!["allow", "list_kind", "items", "yes", "[bad]", "items",]
    );

    let cases = [
        (
            "version: 1\ncontent: nope\noutline: []\n",
            SchemaErrorKind::InvalidContentRule,
            "nope",
        ),
        (
            "version: 1\ncontent: [nope]\noutline: []\n",
            SchemaErrorKind::InvalidContentRule,
            "nope",
        ),
        (
            "version: 1\ncontent: [{block: quote}]\noutline: []\n",
            SchemaErrorKind::InvalidContentRule,
            "block",
        ),
        (
            "version: 1\ncontent: [{one_of: nope}]\noutline: []\n",
            SchemaErrorKind::InvalidContentRule,
            "one_of",
        ),
        (
            "version: 1\ncontent: [{one_of: [{block: p}]}]\noutline: []\n",
            SchemaErrorKind::InvalidContentRule,
            "[{block: p}]",
        ),
        (
            "version: 1\ncontent: [{one_of: [{one_of: []}, {block: p}]}]\noutline: []\n",
            SchemaErrorKind::InvalidContentRule,
            "{one_of: []}",
        ),
        (
            "version: 1\ncontent: [{one_of: [{block: p, id: nope}, {block: list}]}]\noutline: []\n",
            SchemaErrorKind::InvalidContentRule,
            "id",
        ),
        (
            "version: 1\ncontent: [{block: list, items: [{id: ok}]}]\noutline: []\n",
            SchemaErrorKind::InvalidDocumentShape,
            "{id: ok}",
        ),
        (
            "version: 1\ncontent: [{block: list, items: [{match: 7}]}]\noutline: []\n",
            SchemaErrorKind::InvalidDocumentShape,
            "7",
        ),
        (
            "version: 1\ncontent: [{block: list, items: [{match: '/[bad/'}]}]\noutline: []\n",
            SchemaErrorKind::InvalidMatcher,
            "'/[bad/'",
        ),
        (
            "version: 1\ncontent: [{block: p, repeat: 3..1}]\noutline: []\n",
            SchemaErrorKind::InvalidRepeat,
            "3..1",
        ),
        (
            "version: 1\ncontent: [{block: p, id: BAD}]\noutline: []\n",
            SchemaErrorKind::InvalidDocumentShape,
            "BAD",
        ),
        (
            "version: 1\ncontent: [{block: p, id: fm}]\noutline: []\n",
            SchemaErrorKind::ReservedId,
            "fm",
        ),
        (
            "version: 1\ncontent: [{block: p, content_ordered: false}]\noutline: []\n",
            SchemaErrorKind::InvalidDocumentShape,
            "content_ordered",
        ),
    ];
    for (source, kind, anchor) in cases {
        let invalid = invalid(source);
        assert_eq!(invalid.errors.iter().count(), 1, "{source}");
        assert_eq!(invalid.errors.first.kind, kind, "{source}");
        assert_eq!(
            source_slice(source, invalid.errors.first.range),
            anchor,
            "{source}"
        );
    }

    let duplicate = invalid(
        "version: 1\ncontent: [{one_of: [{block: list}, {block: list, list_kind: any}]}]\noutline: []\n",
    );
    assert_eq!(duplicate.errors.iter().count(), 1);
    assert_eq!(duplicate.errors.first.related.len(), 1);
}

#[test]
fn item_pattern_matchers_require_cardinality() {
    for matcher in ["*", "A*", "/A+/"] {
        let source = format!(
            "version: 1\ncontent:\n  - block: list\n    items:\n      - match: '{matcher}'\noutline: []\n"
        );
        let invalid = invalid(&source);
        assert_eq!(invalid.errors.iter().count(), 1);
        assert_eq!(
            invalid.errors.first.kind,
            SchemaErrorKind::MissingCardinality
        );
        assert_eq!(
            source_slice(&source, invalid.errors.first.range),
            format!("'{matcher}'")
        );
    }
}

#[test]
fn document_content_null_is_invalid_at_its_value() {
    let source = "version: 1\ncontent: null\noutline: []\n";
    let rejected = invalid(source);
    assert_eq!(rejected.errors.iter().count(), 1);
    assert_eq!(
        rejected.errors.first.kind,
        SchemaErrorKind::InvalidContentRule
    );
    assert_eq!(source_slice(source, rejected.errors.first.range), "null");
}

#[test]
fn section_content_null_is_invalid_at_its_value() {
    let source = "version: 1\noutline:\n  - match: A\n    content: null\n";
    let rejected = invalid(source);
    assert_eq!(rejected.errors.iter().count(), 1);
    assert_eq!(
        rejected.errors.first.kind,
        SchemaErrorKind::InvalidContentRule
    );
    assert_eq!(source_slice(source, rejected.errors.first.range), "null");
}

#[test]
fn non_mapping_one_of_alternative_is_invalid_at_its_range() {
    let source = "version: 1\ncontent:\n  - one_of:\n      - block: p\n      - nope\noutline: []\n";
    let rejected = invalid(source);
    assert_eq!(rejected.errors.iter().count(), 1);
    assert_eq!(
        rejected.errors.first.kind,
        SchemaErrorKind::InvalidContentRule
    );
    assert_eq!(source_slice(source, rejected.errors.first.range), "nope");
}
