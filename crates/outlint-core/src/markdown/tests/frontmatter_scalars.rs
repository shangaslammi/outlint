use crate::markdown::frontmatter::yaml::exact_frontmatter_mapping;
use crate::markdown::{parse_markdown, DocumentFrontmatter, MarkdownOptions};

use super::NO_MARK;

#[test]
fn preserves_arbitrary_precision_frontmatter_numbers() {
    let document = parse_markdown(
        "---\nbig: 184467440737095516160\nprecise: 0.123456789012345678901234567890\nquoted: \"184467440737095516160\"\n---\n",
        MarkdownOptions::default(),
    );
    let DocumentFrontmatter::Mapping { value, .. } = document.frontmatter else {
        panic!("expected valid numeric frontmatter: {document:?}")
    };
    assert_eq!(value["big"].to_string(), "184467440737095516160");
    assert_eq!(
        value["precise"].to_string(),
        "0.123456789012345678901234567890"
    );
    assert_eq!(value["quoted"], "184467440737095516160");
}

#[test]
fn preserves_json_compatible_frontmatter_number_spellings_and_typed_identity() {
    let document = parse_markdown(
        concat!(
            "---\n",
            "whole: 100.0\n",
            "integer: 100\n",
            "fraction: 1.5\n",
            "lower_exponent: 1e2\n",
            "upper_exponent: 1E2\n",
            "tagged: !!float 2.50\n",
            "base: &number 3.75\n",
            "alias: *number\n",
            "normalized: +4.50\n",
            "forced_float: !!float 1\n",
            "huge: 1e10000\n",
            "tiny: 1e-10000\n",
            "unrelated: !!str value\n",
            "---\n",
        ),
        MarkdownOptions::default(),
    );
    let DocumentFrontmatter::Mapping { value, .. } = document.frontmatter else {
        panic!("expected valid numeric frontmatter: {document:?}")
    };

    assert_eq!(value["whole"].to_string(), "100.0");
    assert_ne!(value["whole"], value["integer"]);
    assert!(jsonschema::draft202012::is_valid(
        &serde_json::json!({"const": 100}),
        &value["whole"]
    ));
    assert_eq!(value["fraction"].to_string(), "1.5");
    assert_eq!(value["lower_exponent"].to_string(), "1e2");
    assert_eq!(value["upper_exponent"].to_string(), "1E2");
    assert_eq!(value["tagged"].to_string(), "2.50");
    assert_eq!(value["base"].to_string(), "3.75");
    assert_eq!(value["alias"].to_string(), "3.75");
    assert_eq!(value["normalized"].to_string(), "45e-1");
    assert_eq!(value["forced_float"].to_string(), "1e+0");
    assert_ne!(value["forced_float"], serde_json::json!(1));
    assert_eq!(value["huge"].to_string(), "1e10000");
    assert_eq!(value["tiny"].to_string(), "1e-10000");
}

#[test]
fn explicit_tags_resolve_to_their_declared_types() {
    let document = parse_markdown(
        concat!(
            "---\n",
            "string: !!str 123\n",
            "integer: !!int \"42\"\n",
            "boolean: !!bool TRUE\n",
            "custom: !thing 123\n",
            "---\n",
        ),
        MarkdownOptions::default(),
    );
    let DocumentFrontmatter::Mapping { value, .. } = document.frontmatter else {
        panic!("expected tagged frontmatter")
    };
    assert_eq!(value["string"], "123");
    assert_eq!(value["integer"], 42);
    assert_eq!(value["boolean"], true);
    assert_eq!(value["custom"], 123);
}

#[test]
fn explicit_tag_on_a_sibling_does_not_round_a_decimal() {
    let plain = parse_markdown(
        "---\nprecise: 0.1234567890123456789012345\n---\n",
        MarkdownOptions::default(),
    );
    let tagged = parse_markdown(
        "---\nprecise: 0.1234567890123456789012345\ntagged: !!str abc\n---\n",
        MarkdownOptions::default(),
    );
    let DocumentFrontmatter::Mapping {
        value: plain_value, ..
    } = plain.frontmatter
    else {
        panic!("expected untagged frontmatter")
    };
    let DocumentFrontmatter::Mapping {
        value: tagged_value,
        ..
    } = tagged.frontmatter
    else {
        panic!("expected tagged frontmatter")
    };

    assert_eq!(tagged_value["precise"], plain_value["precise"]);
    assert_eq!(tagged_value["tagged"], "abc");
}

#[test]
fn explicit_tags_preserve_oversized_integers_and_forced_number_types() {
    let document = parse_markdown(
        concat!(
            "---\n",
            "big: 184467440737095516160\n",
            "precise: !!float 0.1234567890123456789012345\n",
            "tagged: !!str 123\n",
            "---\n",
        ),
        MarkdownOptions::default(),
    );
    let DocumentFrontmatter::Mapping { value, .. } = document.frontmatter else {
        panic!("expected tagged numeric frontmatter: {document:?}")
    };

    assert_eq!(value["big"].to_string(), "184467440737095516160");
    assert_eq!(value["precise"].to_string(), "0.1234567890123456789012345");
    assert_eq!(value["tagged"], "123");
}

#[test]
fn the_exact_builder_keeps_every_digit_it_was_given() {
    // §1.6's exactness is what this whole reader exists for, and under an
    // event-driven builder it rests on the event's own text being the
    // lexeme rather than on any parser option. Twenty-five and thirty
    // digits are both past what a `f64` can distinguish, so each value is
    // paired with the same spelling differing only in its last digit: a
    // parse that went through a float would make the two members of a pair
    // equal, and comparing spellings alone would not notice.
    for (first, second) in [
        ("1234567890123456789012345", "1234567890123456789012346"),
        (
            "123456789012345678901234567890",
            "123456789012345678901234567891",
        ),
        ("0.1234567890123456789012345", "0.1234567890123456789012346"),
        (
            "1.23456789012345678901234567890e5",
            "1.23456789012345678901234567891e5",
        ),
    ] {
        // The tagged sibling once routed the block through this builder
        // alone; it stays so the tagged spelling keeps its coverage.
        let source = format!("first: {first}\nsecond: {second}\ntagged: !!str x\n");
        let (mapping, _) = exact_frontmatter_mapping(&source, NO_MARK)
            .unwrap_or_else(|error| panic!("{source:?}: {error}"));
        assert_eq!(mapping["first"].to_string(), first);
        assert_eq!(mapping["second"].to_string(), second);
        assert_ne!(mapping["first"], mapping["second"], "{source:?}");
    }
}

#[test]
fn standard_tags_with_mismatched_values_are_rejected() {
    for invalid in [
        "bad: !!int 1.0",
        "bad: !!int 01",
        "bad: !!float 0x2A",
        "bad: !!null nope",
        "bad: !!str [one, two]",
        "bad: !!seq {one: two}",
        "bad: !!map [one, two]",
    ] {
        let source = format!("---\nhuge: 184467440737095516160\n{invalid}\n---\n");
        let document = parse_markdown(&source, MarkdownOptions::default());
        assert!(
            matches!(document.frontmatter, DocumentFrontmatter::Invalid { .. }),
            "invalid tag was accepted: {invalid}"
        );
    }
}

#[test]
fn standard_tags_with_conforming_values_are_accepted() {
    let document = parse_markdown(
        concat!(
            "---\n",
            "huge: 184467440737095516160\n",
            "string: !!str 123\n",
            "null_value: !!null null\n",
            "integer: !!int 42\n",
            "binary: !!int 0b101010\n",
            "float: !!float 1.25\n",
            "integer_float: !!float 1\n",
            "sequence: !!seq [one, two]\n",
            "mapping: !!map {one: two}\n",
            "---\n",
        ),
        MarkdownOptions::default(),
    );
    let DocumentFrontmatter::Mapping { value, .. } = document.frontmatter else {
        panic!("expected valid explicitly tagged frontmatter: {document:?}")
    };

    assert_eq!(value["huge"].to_string(), "184467440737095516160");
    assert_eq!(value["string"], "123");
    assert_eq!(value["null_value"], serde_json::Value::Null);
    assert_eq!(value["integer"], 42);
    assert_eq!(value["binary"], 42);
    assert_eq!(value["float"].to_string(), "1.25");
    assert_eq!(value["integer_float"].to_string(), "1e+0");
    assert_ne!(value["integer_float"], serde_json::json!(1));
    assert!(jsonschema::draft202012::is_valid(
        &serde_json::json!({"const": 1}),
        &value["integer_float"]
    ));
    assert_eq!(value["sequence"], serde_json::json!(["one", "two"]));
    assert_eq!(value["mapping"], serde_json::json!({"one": "two"}));
}

#[test]
fn huge_and_tiny_exponents_keep_their_spelling() {
    let document = parse_markdown(
        concat!(
            "---\n",
            "huge: 1e10000\n",
            "tiny: 1e-10000\n",
            "tagged_huge: !!float 2e10000\n",
            "tagged_tiny: !!float 2e-10000\n",
            "unrelated: !!str value\n",
            "---\n",
        ),
        MarkdownOptions::default(),
    );
    let DocumentFrontmatter::Mapping { value, .. } = document.frontmatter else {
        panic!("expected exact ranged decimals: {document:?}")
    };

    assert_eq!(value["huge"].to_string(), "1e10000");
    assert_eq!(value["tiny"].to_string(), "1e-10000");
    assert_eq!(value["tagged_huge"].to_string(), "2e10000");
    assert_eq!(value["tagged_tiny"].to_string(), "2e-10000");
}

#[test]
fn nonfinite_and_malformed_float_tags_are_rejected() {
    for invalid in ["bad: !!float .inf", "bad: !!float 1e", "bad: !!float nope"] {
        let source = format!("---\nhuge: 184467440737095516160\n{invalid}\n---\n");
        let document = parse_markdown(&source, MarkdownOptions::default());
        assert!(
            matches!(document.frontmatter, DocumentFrontmatter::Invalid { .. }),
            "invalid float was accepted: {invalid}"
        );
    }
}

#[test]
fn preserves_yaml_alias_values() {
    let document = parse_markdown(
        "---\nbase: &base 42\ncopy: *base\n---\n",
        MarkdownOptions::default(),
    );
    let DocumentFrontmatter::Mapping { value, .. } = document.frontmatter else {
        panic!("expected aliased frontmatter: {document:?}")
    };
    assert_eq!(value["base"], 42);
    assert_eq!(value["copy"], value["base"]);
}

#[test]
fn aliases_preserve_exact_numeric_values() {
    let document = parse_markdown(
        "---\nbase: &base 0.1234567890123456789012345\ncopy: *base\n---\n",
        MarkdownOptions::default(),
    );
    let DocumentFrontmatter::Mapping { value, .. } = document.frontmatter else {
        panic!("expected aliased frontmatter: {document:?}")
    };

    assert_eq!(value["base"].to_string(), "0.1234567890123456789012345");
    assert_eq!(value["copy"], value["base"]);
}
