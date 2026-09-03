//! Tests for the typed-value kernel.

use super::*;

#[test]
fn value_type_names_round_trip() {
    for value_type in ValueType::ALL {
        let name = value_type.as_str();
        assert_eq!(
            ValueType::from_name(name),
            Some(value_type),
            "{name} did not round-trip"
        );
    }
}

#[test]
fn value_type_names_are_exactly_the_closed_set() {
    let names: Vec<&str> = ValueType::ALL.iter().map(|t| t.as_str()).collect();
    assert_eq!(names, ["int", "bool", "date", "semver", "dotted", "text"]);
}

#[test]
fn value_type_names_are_lowercase_only_and_otherwise_unknown() {
    for name in [
        "Int", "INT", "integer", "boolean", "Bool", "Date", "SemVer", "semver ", " semver",
        "string", "float", "null", "", "dotted.",
    ] {
        assert_eq!(ValueType::from_name(name), None, "{name} was accepted");
    }
}

#[test]
fn frontmatter_kind_is_integer_for_int_and_boolean_for_bool() {
    assert_eq!(ValueType::Int.frontmatter_kind(), ResolvedYamlKind::Integer);
    assert_eq!(
        ValueType::Bool.frontmatter_kind(),
        ResolvedYamlKind::Boolean
    );
}

#[test]
fn frontmatter_kind_is_string_for_every_other_type() {
    for value_type in [
        ValueType::Date,
        ValueType::Semver,
        ValueType::Dotted,
        ValueType::Text,
    ] {
        assert_eq!(
            value_type.frontmatter_kind(),
            ResolvedYamlKind::String,
            "{} does not take a YAML string",
            value_type.as_str()
        );
    }
}

#[test]
fn every_type_accepts_exactly_one_frontmatter_kind() {
    // The mapping is total and lands only in scalar kinds: no capture type
    // ever admits a null, a sequence, or a mapping.
    for value_type in ValueType::ALL {
        let kind = value_type.frontmatter_kind();
        assert!(
            matches!(
                kind,
                ResolvedYamlKind::Integer | ResolvedYamlKind::Boolean | ResolvedYamlKind::String
            ),
            "{} admits {kind:?}",
            value_type.as_str()
        );
    }
}

#[test]
fn frontmatter_value_borrows_its_json_value() {
    let json = Value::String("1.0.0".to_owned());
    let supplied = FrontmatterValue::new(&json, ResolvedYamlKind::String);
    assert!(
        std::ptr::eq(supplied.value(), &json),
        "the selected node was copied instead of borrowed"
    );
}

#[test]
fn frontmatter_value_retains_the_separately_supplied_kind() {
    // The same JSON number stands for either a YAML integer or a YAML float;
    // only the supplied kind separates them.
    let json: Value = serde_json::from_str("1.20").expect("1.20 is valid JSON");
    let as_float = FrontmatterValue::new(&json, ResolvedYamlKind::Float);
    let as_integer = FrontmatterValue::new(&json, ResolvedYamlKind::Integer);

    assert_eq!(as_float.yaml_kind(), ResolvedYamlKind::Float);
    assert_eq!(as_integer.yaml_kind(), ResolvedYamlKind::Integer);
    assert!(std::ptr::eq(as_float.value(), as_integer.value()));
}

#[test]
fn parse_failures_carry_facts_rather_than_prose() {
    let mismatch = ParseFailure::KindMismatch {
        expected: ResolvedYamlKind::Integer,
        actual: ResolvedYamlKind::Float,
    };
    assert_ne!(mismatch, ParseFailure::Lexical);

    let overflow = ParseFailure::BoundOverflow {
        component: BoundComponent::SemverPrerelease { index: 1 },
    };
    assert_ne!(
        overflow,
        ParseFailure::BoundOverflow {
            component: BoundComponent::SemverPrerelease { index: 0 }
        }
    );
    assert_ne!(
        ParseFailure::BoundOverflow {
            component: BoundComponent::Int
        },
        ParseFailure::InvalidDate
    );

    // The rejected build-metadata suffix keeps its leading `+`.
    let build = ParseFailure::BuildMetadata {
        suffix: "+build.7".to_owned(),
    };
    assert_eq!(
        build,
        ParseFailure::BuildMetadata {
            suffix: "+build.7".to_owned()
        }
    );
}

// ---------------------------------------------------------------------------
// `int`
// ---------------------------------------------------------------------------

#[test]
fn int_accepts_signed_decimal_with_redundant_leading_zeros() {
    for (source, expected) in [
        ("0", 0),
        ("7", 7),
        ("-7", -7),
        ("007", 7),
        ("-01", -1),
        ("00000000000000000000042", 42),
        ("9223372036854775807", i64::MAX),
        ("-9223372036854775808", i64::MIN),
    ] {
        assert_eq!(parse_int(source), Ok(expected), "int {source}");
    }
}

#[test]
fn int_normalizes_negative_zero_to_zero() {
    assert_eq!(parse_int("-0"), Ok(0));
    assert_eq!(parse_int("-0000"), Ok(0));
    assert_eq!(parse_int("-0"), parse_int("0"));
}

#[test]
fn int_rejects_every_spelling_outside_the_grammar() {
    for source in [
        "",
        "-",
        "+1",
        "+0",
        " 1",
        "1 ",
        "1_000",
        "1,000",
        "1.0",
        "1e3",
        "0x1f",
        "--1",
        "1-",
        "abc",
        "١٢٣",       // Arabic-Indic digits are not ASCII digits.
        "\u{ff11}",  // Fullwidth digit one.
        "1\u{0000}", // A NUL is not a digit either.
    ] {
        assert_eq!(
            parse_int(source),
            Err(ParseFailure::Lexical),
            "int {source:?} should be lexically invalid"
        );
    }
}

#[test]
fn int_separates_the_signed_64_bit_bound_from_lexical_failure() {
    let overflow = Err(ParseFailure::BoundOverflow {
        component: BoundComponent::Int,
    });
    for source in [
        "9223372036854775808",
        "-9223372036854775809",
        "99999999999999999999999999",
        "-99999999999999999999999999",
        "0000000009223372036854775808",
    ] {
        assert_eq!(parse_int(source), overflow, "int {source} should overflow");
    }
}

// ---------------------------------------------------------------------------
// `bool`
// ---------------------------------------------------------------------------

#[test]
fn bool_header_spelling_is_lowercase_true_or_false_only() {
    assert_eq!(parse_bool("true"), Ok(true));
    assert_eq!(parse_bool("false"), Ok(false));
    for source in [
        "True", "TRUE", "False", "FALSE", "yes", "no", "on", "off", "y", "n", "1", "0", "",
        " true", "true ",
    ] {
        assert_eq!(
            parse_bool(source),
            Err(ParseFailure::Lexical),
            "bool {source:?} should be lexically invalid"
        );
    }
}

// ---------------------------------------------------------------------------
// `date`
// ---------------------------------------------------------------------------

#[test]
fn date_accepts_the_full_proleptic_gregorian_range() {
    for (source, expected) in [
        ("0000-01-01", (0, 1, 1)),
        ("0000-02-29", (0, 2, 29)),
        ("2000-02-29", (2000, 2, 29)),
        ("2024-02-29", (2024, 2, 29)),
        ("2023-12-31", (2023, 12, 31)),
        ("9999-12-31", (9999, 12, 31)),
    ] {
        let (year, month, day) = expected;
        assert_eq!(
            parse_date(source),
            Ok(DateValue { year, month, day }),
            "date {source}"
        );
    }
}

#[test]
fn date_applies_the_century_and_four_century_leap_rules() {
    // Divisible by four is not enough; divisible by four hundred restores it.
    assert_eq!(parse_date("1900-02-29"), Err(ParseFailure::InvalidDate));
    assert_eq!(parse_date("2100-02-29"), Err(ParseFailure::InvalidDate));
    assert_eq!(parse_date("2023-02-29"), Err(ParseFailure::InvalidDate));
    assert!(parse_date("2000-02-29").is_ok());
    assert!(parse_date("0000-02-29").is_ok());
    assert!(parse_date("1600-02-29").is_ok());
    assert!(parse_date("2024-02-29").is_ok());
    assert!(parse_date("1900-02-28").is_ok());
}

#[test]
fn date_rejects_days_the_calendar_does_not_have() {
    for source in [
        "2024-00-10",
        "2024-13-01",
        "2024-99-01",
        "2024-01-00",
        "2024-01-32",
        "2024-04-31",
        "2024-06-31",
        "2024-09-31",
        "2024-11-31",
        "2023-02-30",
    ] {
        assert_eq!(
            parse_date(source),
            Err(ParseFailure::InvalidDate),
            "date {source} should fail the calendar"
        );
    }
}

#[test]
fn date_requires_the_fixed_ten_character_shape() {
    for source in [
        "",
        "2024-1-01",
        "2024-01-1",
        "24-01-01",
        "20240101",
        "2024/01/01",
        "2024.01.01",
        "2024-01-011",
        " 2024-01-01",
        "2024-01-01 ",
        "2024-01-0a",
        "+024-01-01",
        "-024-01-01",
        "٢٠٢٤-٠١-٠١",
        "2024\u{2013}01-01", // An en dash occupies three bytes, not one.
    ] {
        assert_eq!(
            parse_date(source),
            Err(ParseFailure::Lexical),
            "date {source:?} should be lexically invalid"
        );
    }
}

#[test]
fn date_cannot_be_normalized_outside_the_calendar() {
    assert_eq!(DateValue::new(2023, 2, 29), None);
    assert_eq!(DateValue::new(2024, 0, 1), None);
    assert_eq!(DateValue::new(2024, 13, 1), None);
    assert_eq!(DateValue::new(2024, 1, 0), None);
    assert_eq!(DateValue::new(10000, 1, 1), None);
    assert!(DateValue::new(2024, 2, 29).is_some());
}

// ---------------------------------------------------------------------------
// `dotted`
// ---------------------------------------------------------------------------

#[test]
fn dotted_accepts_one_or_more_components_with_redundant_leading_zeros() {
    for (source, expected) in [
        ("0", vec![0]),
        ("1", vec![1]),
        ("1.2", vec![1, 2]),
        ("1.02.0", vec![1, 2, 0]),
        ("0001.0002", vec![1, 2]),
        ("1.2.3.4.5", vec![1, 2, 3, 4, 5]),
        ("4294967295", vec![u32::MAX]),
    ] {
        assert_eq!(
            parse_dotted(source).map(|value| value.components().to_vec()),
            Ok(expected),
            "dotted {source}"
        );
    }
}

#[test]
fn dotted_rejects_empty_components_at_any_position() {
    for source in [
        "", ".", ".1", "1.", "1..2", "1.2.", "..", "-1", "1.-2", "1.a", "1 .2", "1. 2",
    ] {
        assert_eq!(
            parse_dotted(source),
            Err(ParseFailure::Lexical),
            "dotted {source:?} should be lexically invalid"
        );
    }
}

#[test]
fn dotted_reports_the_overflowing_component_by_position() {
    assert_eq!(
        parse_dotted("4294967296"),
        Err(ParseFailure::BoundOverflow {
            component: BoundComponent::DottedComponent { index: 0 }
        })
    );
    assert_eq!(
        parse_dotted("1.4294967296"),
        Err(ParseFailure::BoundOverflow {
            component: BoundComponent::DottedComponent { index: 1 }
        })
    );
    assert_eq!(
        parse_dotted("1.2.99999999999999999999"),
        Err(ParseFailure::BoundOverflow {
            component: BoundComponent::DottedComponent { index: 2 }
        })
    );
}

#[test]
fn dotted_cannot_be_normalized_to_an_empty_sequence() {
    assert_eq!(DottedValue::new(Vec::new()), None);
    assert!(DottedValue::new(vec![0]).is_some());
    // No accepted spelling produces one either: the parser never returns a
    // value whose component list is empty.
    for source in ["0", "1.2", "0001.0002", "4294967295"] {
        let parsed = parse_dotted(source).expect("the spelling is valid");
        assert!(!parsed.components().is_empty(), "dotted {source}");
    }
}

// ---------------------------------------------------------------------------
// `text`
// ---------------------------------------------------------------------------

#[test]
fn text_preserves_its_source_scalar_for_scalar() {
    for source in [
        "",
        " ",
        "  padded  ",
        "Mixed CASE",
        "*not markdown*",
        "line\nbreak",
        "tab\there",
        "e\u{0301}",  // Decomposed e-acute.
        "\u{00e9}",   // Composed e-acute.
        "\u{1f600}",  // Four UTF-8 bytes.
        "\u{7ff}",    // Two UTF-8 bytes.
        "\u{800}",    // Three UTF-8 bytes.
        "\u{200b}",   // Zero-width space.
        "İstanbul",   // Case folding would change the length.
        "ß",          // And so would upcasing this one.
        "0123",       // A `text` capture never becomes a number.
        "2024-02-30", // Nor is it checked against another type's grammar.
    ] {
        let parsed = parse_text(source);
        assert_eq!(parsed, source);
        assert_eq!(parsed.as_bytes(), source.as_bytes(), "text {source:?}");
        assert_eq!(
            parsed.chars().collect::<Vec<char>>(),
            source.chars().collect::<Vec<char>>()
        );
    }
}
