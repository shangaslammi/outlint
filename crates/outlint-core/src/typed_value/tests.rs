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

// ---------------------------------------------------------------------------
// Normalized-value accessors
//
// These reach into the private representation so a test can state what a
// parse produced rather than only that it succeeded. The panics are on
// invariants the test itself authored, never on parser input.
// ---------------------------------------------------------------------------

fn expect_int(parsed: &TypedValue) -> i64 {
    match &parsed.value {
        NormalizedValue::Int(value) => *value,
        other => panic!("expected an int, got {other:?}"),
    }
}

fn expect_bool(parsed: &TypedValue) -> bool {
    match &parsed.value {
        NormalizedValue::Bool(value) => *value,
        other => panic!("expected a bool, got {other:?}"),
    }
}

fn expect_date(parsed: &TypedValue) -> DateValue {
    match &parsed.value {
        NormalizedValue::Date(value) => *value,
        other => panic!("expected a date, got {other:?}"),
    }
}

fn expect_semver(parsed: &TypedValue) -> semver::Version {
    match &parsed.value {
        NormalizedValue::Semver(value) => value.version().clone(),
        other => panic!("expected a semver, got {other:?}"),
    }
}

fn expect_dotted(parsed: &TypedValue) -> Vec<u32> {
    match &parsed.value {
        NormalizedValue::Dotted(value) => value.components().to_vec(),
        other => panic!("expected a dotted, got {other:?}"),
    }
}

fn expect_text(parsed: &TypedValue) -> String {
    match &parsed.value {
        NormalizedValue::Text(value) => value.clone(),
        other => panic!("expected a text, got {other:?}"),
    }
}

/// The outcome of a header parse with the successful value discarded.
///
/// A test compares failures through this rather than through the `Result`
/// itself, so that `TypedValue` never acquires a `PartialEq` of its own: the
/// only equality it has is the §2.4 relation, and a derived one alongside it
/// would be a second answer to the same question.
fn header_outcome(value_type: ValueType, source: &str) -> Result<(), ParseFailure> {
    parse_header(value_type, source).map(|_| ())
}

/// The outcome of a frontmatter parse, with the successful value discarded
/// for the reason [`header_outcome`] gives.
fn frontmatter_outcome(
    value_type: ValueType,
    supplied: FrontmatterValue<'_>,
) -> Result<(), ParseFailure> {
    parse_frontmatter(value_type, supplied).map(|_| ())
}

fn frontmatter_string(value_type: ValueType, source: &str) -> Result<TypedValue, ParseFailure> {
    let json = Value::String(source.to_owned());
    parse_frontmatter(
        value_type,
        FrontmatterValue::new(&json, ResolvedYamlKind::String),
    )
}

fn exact_json_number(spelling: &str) -> Value {
    serde_json::from_str(spelling).expect("the test supplies a valid JSON number")
}

// ---------------------------------------------------------------------------
// `semver`
// ---------------------------------------------------------------------------

#[test]
fn semver_accepts_releases_and_prereleases_without_build_metadata() {
    for source in [
        "0.0.0",
        "1.0.0",
        "1.2.3",
        "1.0.0-rc.1",
        "1.0.0-alpha",
        "1.0.0-alpha.beta",
        "1.0.0-0.3.7",
        "1.0.0-x.7.z.92",
        "18446744073709551615.18446744073709551615.18446744073709551615",
        "1.0.0-18446744073709551615",
    ] {
        let parsed = parse_semver(source).unwrap_or_else(|failure| {
            panic!("semver {source} should parse, got {failure:?}");
        });
        assert!(
            parsed.version().build.is_empty(),
            "an admitted semver always has empty build metadata"
        );
        assert_eq!(parsed.version().to_string(), source, "semver {source}");
    }
}

#[test]
fn semver_preserves_prerelease_identifier_case() {
    let parsed = parse_semver("1.0.0-RC.Alpha1").expect("the spelling is valid");
    assert_eq!(parsed.version().pre.as_str(), "RC.Alpha1");
    assert_ne!(parsed.version().pre.as_str(), "rc.alpha1");
}

#[test]
fn semver_rejects_build_metadata_with_the_suffix_attached() {
    for (source, suffix) in [
        ("1.0.0+build", "+build"),
        ("1.0.0+build.7", "+build.7"),
        ("1.2.3+21AF26D3", "+21AF26D3"),
        ("1.0.0-rc.1+exp.sha.5114f85", "+exp.sha.5114f85"),
    ] {
        assert_eq!(
            parse_semver(source),
            Err(ParseFailure::BuildMetadata {
                suffix: suffix.to_owned()
            }),
            "semver {source}"
        );
    }
}

#[test]
fn semver_reports_malformed_build_syntax_as_lexical() {
    // An empty or malformed build suffix is not valid SemVer at all, so it
    // is not the specific build-metadata rejection.
    for source in ["1.0.0+", "1.0.0+build..7", "1.0.0+bu!ld", "1.0.0++build"] {
        assert_eq!(
            parse_semver(source),
            Err(ParseFailure::Lexical),
            "semver {source:?}"
        );
    }
}

#[test]
fn semver_rejects_spellings_outside_the_grammar() {
    for source in [
        "",
        "1",
        "1.2",
        "1.2.3.4",
        "v1.2.3",
        "1.2.3-",
        "1.2.3-01",
        "01.2.3",
        "1.02.3",
        "-1.2.3",
        "1.2.-3",
        "1.2.3 ",
        " 1.2.3",
        "1.2.x",
        "1.0.0-rc..1",
    ] {
        assert_eq!(
            parse_semver(source),
            Err(ParseFailure::Lexical),
            "semver {source:?}"
        );
    }
}

#[test]
fn semver_bounds_every_numeric_position_to_unsigned_64_bits() {
    let past_u64 = "18446744073709551616";
    for (source, component) in [
        (format!("{past_u64}.0.0"), BoundComponent::SemverMajor),
        (format!("0.{past_u64}.0"), BoundComponent::SemverMinor),
        (format!("0.0.{past_u64}"), BoundComponent::SemverPatch),
        (
            format!("1.0.0-{past_u64}"),
            BoundComponent::SemverPrerelease { index: 0 },
        ),
        (
            format!("1.0.0-rc.{past_u64}"),
            BoundComponent::SemverPrerelease { index: 1 },
        ),
        (
            format!("1.0.0-a.b.{past_u64}.c"),
            BoundComponent::SemverPrerelease { index: 2 },
        ),
        (
            format!("1.0.0-{past_u64}+build"),
            BoundComponent::SemverPrerelease { index: 0 },
        ),
    ] {
        assert_eq!(
            parse_semver(&source),
            Err(ParseFailure::BoundOverflow { component }),
            "semver {source}"
        );
    }
}

#[test]
fn semver_leaves_alphanumeric_prerelease_identifiers_unbounded() {
    // The bound applies to numeric identifiers; a long alphanumeric one is
    // compared as text and has no numeric value to exceed.
    let source = format!("1.0.0-a{}", "9".repeat(40));
    assert!(parse_semver(&source).is_ok(), "semver {source}");
}

// ---------------------------------------------------------------------------
// Entry points: header
// ---------------------------------------------------------------------------

#[test]
fn parse_header_dispatches_each_type_to_its_own_grammar() {
    assert_eq!(
        expect_int(&parse_header(ValueType::Int, "-01").expect("valid int")),
        -1
    );
    assert!(expect_bool(
        &parse_header(ValueType::Bool, "true").expect("valid bool")
    ));
    assert_eq!(
        expect_date(&parse_header(ValueType::Date, "2024-02-29").expect("valid date")),
        DateValue {
            year: 2024,
            month: 2,
            day: 29
        }
    );
    assert_eq!(
        expect_semver(&parse_header(ValueType::Semver, "1.0.0-rc.1").expect("valid semver"))
            .to_string(),
        "1.0.0-rc.1"
    );
    assert_eq!(
        expect_dotted(&parse_header(ValueType::Dotted, "1.02.0").expect("valid dotted")),
        vec![1, 2, 0]
    );
    assert_eq!(
        expect_text(&parse_header(ValueType::Text, " 1.02.0 ").expect("valid text")),
        " 1.02.0 "
    );
}

#[test]
fn parse_header_reports_the_value_type_it_parsed() {
    for (value_type, source) in [
        (ValueType::Int, "1"),
        (ValueType::Bool, "false"),
        (ValueType::Date, "2024-01-01"),
        (ValueType::Semver, "1.0.0"),
        (ValueType::Dotted, "1.2"),
        (ValueType::Text, "anything"),
    ] {
        let parsed = parse_header(value_type, source).expect("the spelling is valid");
        assert_eq!(parsed.value_type(), value_type);
    }
}

#[test]
fn parse_header_accepts_any_string_only_as_text() {
    // A source that fails every other grammar is still a valid `text`.
    let source = "not a value";
    for value_type in [
        ValueType::Int,
        ValueType::Bool,
        ValueType::Date,
        ValueType::Semver,
        ValueType::Dotted,
    ] {
        assert!(
            parse_header(value_type, source).is_err(),
            "{} accepted {source:?}",
            value_type.as_str()
        );
    }
    assert_eq!(
        expect_text(&parse_header(ValueType::Text, source).expect("text takes any string")),
        source
    );
}

// ---------------------------------------------------------------------------
// Entry points: frontmatter
// ---------------------------------------------------------------------------

#[test]
fn frontmatter_int_reads_the_exact_arbitrary_precision_spelling() {
    for (spelling, expected) in [
        ("0", 0),
        ("-0", 0),
        ("42", 42),
        ("-42", -42),
        ("9223372036854775807", i64::MAX),
        ("-9223372036854775808", i64::MIN),
    ] {
        let json = exact_json_number(spelling);
        let parsed = parse_frontmatter(
            ValueType::Int,
            FrontmatterValue::new(&json, ResolvedYamlKind::Integer),
        )
        .unwrap_or_else(|failure| panic!("frontmatter int {spelling}: {failure:?}"));
        assert_eq!(expect_int(&parsed), expected, "frontmatter int {spelling}");
    }
}

#[test]
fn frontmatter_int_beyond_machine_bounds_is_a_bound_failure_not_a_lexical_one() {
    // §1.6 preserves the exact mathematical value, so the kernel sees the
    // real magnitude rather than a saturated or truncated one.
    for spelling in [
        "9223372036854775808",
        "-9223372036854775809",
        "170141183460469231731687303715884105728",
    ] {
        let json = exact_json_number(spelling);
        assert_eq!(
            frontmatter_outcome(
                ValueType::Int,
                FrontmatterValue::new(&json, ResolvedYamlKind::Integer),
            ),
            Err(ParseFailure::BoundOverflow {
                component: BoundComponent::Int
            }),
            "frontmatter int {spelling}"
        );
        assert_eq!(json.as_i64(), None, "as_i64 would have erased {spelling}");
    }
}

#[test]
fn frontmatter_string_types_use_the_same_grammar_as_a_header() {
    assert_eq!(
        expect_date(&frontmatter_string(ValueType::Date, "0000-02-29").expect("valid date")),
        DateValue {
            year: 0,
            month: 2,
            day: 29
        }
    );
    assert_eq!(
        expect_semver(&frontmatter_string(ValueType::Semver, "1.0.0-rc.1").expect("valid semver"))
            .to_string(),
        "1.0.0-rc.1"
    );
    assert_eq!(
        expect_dotted(&frontmatter_string(ValueType::Dotted, "1.02.0").expect("valid dotted")),
        vec![1, 2, 0]
    );
    assert_eq!(
        expect_text(&frontmatter_string(ValueType::Text, "  kept  ").expect("valid text")),
        "  kept  "
    );
    assert_eq!(
        frontmatter_string(ValueType::Semver, "1.0.0+build").map(|_| ()),
        Err(ParseFailure::BuildMetadata {
            suffix: "+build".to_owned()
        })
    );
}

#[test]
fn frontmatter_rejects_every_kind_but_the_one_the_type_accepts() {
    let null = Value::Null;
    let boolean = Value::Bool(true);
    let number = exact_json_number("1.20");
    let integer = exact_json_number("1");
    let string = Value::String("1.2.0".to_owned());
    let sequence = Value::Array(vec![Value::String("1.2.0".to_owned())]);
    let mapping = Value::Object(serde_json::Map::new());

    let supplied = [
        (&null, ResolvedYamlKind::Null),
        (&boolean, ResolvedYamlKind::Boolean),
        (&integer, ResolvedYamlKind::Integer),
        (&number, ResolvedYamlKind::Float),
        (&string, ResolvedYamlKind::String),
        (&sequence, ResolvedYamlKind::Sequence),
        (&mapping, ResolvedYamlKind::Mapping),
    ];

    for value_type in ValueType::ALL {
        let expected = value_type.frontmatter_kind();
        for (value, actual) in supplied {
            if actual == expected {
                continue;
            }
            assert_eq!(
                frontmatter_outcome(value_type, FrontmatterValue::new(value, actual)),
                Err(ParseFailure::KindMismatch { expected, actual }),
                "{} should reject a {actual:?}",
                value_type.as_str()
            );
        }
    }
}

#[test]
fn frontmatter_never_coerces_a_nonmatching_value_by_rendering_it() {
    // `1.2` renders as a plausible `dotted` and `1` as a plausible `int`,
    // yet neither is admitted, because the kind decides before the grammar.
    let float = exact_json_number("1.2");
    assert_eq!(
        frontmatter_outcome(
            ValueType::Dotted,
            FrontmatterValue::new(&float, ResolvedYamlKind::Float)
        ),
        Err(ParseFailure::KindMismatch {
            expected: ResolvedYamlKind::String,
            actual: ResolvedYamlKind::Float
        })
    );
    let integer = exact_json_number("1");
    assert_eq!(
        frontmatter_outcome(
            ValueType::Semver,
            FrontmatterValue::new(&integer, ResolvedYamlKind::Integer)
        ),
        Err(ParseFailure::KindMismatch {
            expected: ResolvedYamlKind::String,
            actual: ResolvedYamlKind::Integer
        })
    );
}

#[test]
fn frontmatter_reports_a_disagreeing_value_and_kind_without_panicking() {
    // A producer bug: the kind says integer while the node is a string.
    let string = Value::String("42".to_owned());
    assert_eq!(
        frontmatter_outcome(
            ValueType::Int,
            FrontmatterValue::new(&string, ResolvedYamlKind::Integer)
        ),
        Err(ParseFailure::KindMismatch {
            expected: ResolvedYamlKind::Integer,
            actual: ResolvedYamlKind::String
        })
    );
    let sequence = Value::Array(Vec::new());
    assert_eq!(
        frontmatter_outcome(
            ValueType::Text,
            FrontmatterValue::new(&sequence, ResolvedYamlKind::String)
        ),
        Err(ParseFailure::KindMismatch {
            expected: ResolvedYamlKind::String,
            actual: ResolvedYamlKind::Sequence
        })
    );
    let boolean = Value::Bool(false);
    assert_eq!(
        parse_frontmatter(
            ValueType::Bool,
            FrontmatterValue::new(&boolean, ResolvedYamlKind::Boolean)
        )
        .map(|parsed| expect_bool(&parsed)),
        Ok(false)
    );
}

// ---------------------------------------------------------------------------
// The §2.4 boundary table
// ---------------------------------------------------------------------------

#[test]
fn boundary_header_int_negative_leading_zero_equals_negative_one() {
    let padded = parse_header(ValueType::Int, "-01").expect("`-01` is a valid int");
    let plain = parse_header(ValueType::Int, "-1").expect("`-1` is a valid int");
    assert_eq!(expect_int(&padded), -1);
    assert_eq!(expect_int(&padded), expect_int(&plain));
}

#[test]
fn boundary_int_above_i64_max_is_bound_overflow() {
    let overflow = Err(ParseFailure::BoundOverflow {
        component: BoundComponent::Int,
    });
    assert_eq!(
        header_outcome(ValueType::Int, "9223372036854775808"),
        overflow
    );

    let json = exact_json_number("9223372036854775808");
    assert_eq!(
        frontmatter_outcome(
            ValueType::Int,
            FrontmatterValue::new(&json, ResolvedYamlKind::Integer)
        ),
        overflow
    );
}

#[test]
fn boundary_header_bool_titlecase_is_lexical_error() {
    assert_eq!(
        header_outcome(ValueType::Bool, "True"),
        Err(ParseFailure::Lexical)
    );
}

#[test]
fn boundary_frontmatter_bool_core_resolved_true_is_valid() {
    // The document spelled `True`; the YAML 1.2 core resolver turned it into
    // a boolean before the kernel ever saw it, so the kernel sees `true`.
    let resolved = Value::Bool(true);
    let parsed = parse_frontmatter(
        ValueType::Bool,
        FrontmatterValue::new(&resolved, ResolvedYamlKind::Boolean),
    )
    .expect("a resolved YAML boolean is a valid frontmatter bool");
    assert!(expect_bool(&parsed));

    // The same spelling reaching a header capture is still invalid.
    assert_eq!(
        header_outcome(ValueType::Bool, "True"),
        Err(ParseFailure::Lexical)
    );
}

#[test]
fn boundary_date_leap_day_valid_and_common_year_invalid() {
    assert_eq!(
        expect_date(&parse_header(ValueType::Date, "2024-02-29").expect("2024 is a leap year")),
        DateValue {
            year: 2024,
            month: 2,
            day: 29
        }
    );
    assert_eq!(
        header_outcome(ValueType::Date, "2023-02-29"),
        Err(ParseFailure::InvalidDate)
    );
}

#[test]
fn boundary_semver_prerelease_valid_and_build_metadata_rejected() {
    let parsed =
        parse_header(ValueType::Semver, "1.0.0-rc.1").expect("a pre-release is a valid semver");
    assert_eq!(expect_semver(&parsed).to_string(), "1.0.0-rc.1");

    assert_eq!(
        header_outcome(ValueType::Semver, "1.0.0+build"),
        Err(ParseFailure::BuildMetadata {
            suffix: "+build".to_owned()
        })
    );
}

#[test]
fn boundary_dotted_leading_zero_components_are_equal() {
    let padded = parse_header(ValueType::Dotted, "1.02.0").expect("leading zeros are allowed");
    let plain = parse_header(ValueType::Dotted, "1.2.0").expect("`1.2.0` is a valid dotted");
    assert_eq!(expect_dotted(&padded), vec![1, 2, 0]);
    assert_eq!(expect_dotted(&padded), expect_dotted(&plain));
}

#[test]
fn boundary_dotted_component_above_u32_max_is_bound_overflow() {
    assert_eq!(
        header_outcome(ValueType::Dotted, "4294967296"),
        Err(ParseFailure::BoundOverflow {
            component: BoundComponent::DottedComponent { index: 0 }
        })
    );
}

#[test]
fn boundary_frontmatter_text_does_not_coerce_integer_kind() {
    let integer = exact_json_number("42");
    assert_eq!(
        frontmatter_outcome(
            ValueType::Text,
            FrontmatterValue::new(&integer, ResolvedYamlKind::Integer)
        ),
        Err(ParseFailure::KindMismatch {
            expected: ResolvedYamlKind::String,
            actual: ResolvedYamlKind::Integer
        })
    );
}
