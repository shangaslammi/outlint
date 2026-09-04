//! Tests for the typed-value kernel.

use proptest::prelude::*;

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

#[test]
fn as_bool_projects_only_a_bool_value() {
    // §4.6: "a bound `bool` capture contributes its boolean value: a valid
    // bound `false` is unsatisfied". Both booleans have to be readable, and
    // they have to be readable apart from each other.
    assert_eq!(
        parse_header(ValueType::Bool, "true")
            .expect("`true` is a bool")
            .as_bool(),
        Some(true)
    );
    assert_eq!(
        parse_header(ValueType::Bool, "false")
            .expect("`false` is a bool")
            .as_bool(),
        Some(false)
    );
    // A YAML boolean reaches the same projection through the other source.
    let yes = Value::Bool(true);
    assert_eq!(
        parse_frontmatter(
            ValueType::Bool,
            FrontmatterValue::new(&yes, ResolvedYamlKind::Boolean)
        )
        .expect("a YAML boolean is a bool")
        .as_bool(),
        Some(true)
    );
}

#[test]
fn as_bool_refuses_every_other_type_including_boolean_looking_text() {
    // The projection is a type test, not a reading of the spelling: a `text`
    // capture whose characters are `false` is not a false boolean, and a
    // proposition that treated it as one would let a string decide a
    // constraint.
    for (value_type, source) in [
        (ValueType::Text, "true"),
        (ValueType::Text, "false"),
        (ValueType::Text, "yes"),
        (ValueType::Int, "0"),
        (ValueType::Int, "1"),
        (ValueType::Date, "2024-02-29"),
        (ValueType::Semver, "1.0.0"),
        (ValueType::Dotted, "1.2"),
    ] {
        let parsed = parse_header(value_type, source).expect("the source parses as its type");
        assert_eq!(
            parsed.as_bool(),
            None,
            "{} `{source}` must not project to a boolean",
            value_type.as_str()
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
    assert_eq!(padded.equals(&plain), Some(true));
    assert_eq!(padded.compare(&plain), Some(Ordering::Equal));
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
    assert_eq!(padded.equals(&plain), Some(true));
    assert_eq!(padded.compare(&plain), Some(Ordering::Equal));
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

// ---------------------------------------------------------------------------
// Equality and ordering
// ---------------------------------------------------------------------------

fn header(value_type: ValueType, source: &str) -> TypedValue {
    parse_header(value_type, source)
        .unwrap_or_else(|failure| panic!("{source:?} should parse: {failure:?}"))
}

/// Asserts that every source in `ascending` compares strictly below the ones
/// after it, and equal only to itself.
fn assert_strictly_ascending(value_type: ValueType, ascending: &[&str]) {
    for (left_index, left_source) in ascending.iter().enumerate() {
        for (right_index, right_source) in ascending.iter().enumerate() {
            let left = header(value_type, left_source);
            let right = header(value_type, right_source);
            let expected = left_index.cmp(&right_index);
            assert_eq!(
                left.compare(&right),
                Some(expected),
                "{left_source:?} against {right_source:?}"
            );
            assert_eq!(
                left.equals(&right),
                Some(expected == Ordering::Equal),
                "{left_source:?} against {right_source:?}"
            );
        }
    }
}

#[test]
fn int_orders_mathematically() {
    assert_strictly_ascending(
        ValueType::Int,
        &[
            "-9223372036854775808",
            "-100",
            "-2",
            "-1",
            "0",
            "1",
            "2",
            "100",
            "9223372036854775807",
        ],
    );
}

#[test]
fn int_ignores_redundant_zeros_and_the_sign_of_zero() {
    let padded = header(ValueType::Int, "-01");
    let plain = header(ValueType::Int, "-1");
    assert_eq!(padded.equals(&plain), Some(true));

    let negative_zero = header(ValueType::Int, "-0");
    let zero = header(ValueType::Int, "0");
    let padded_zero = header(ValueType::Int, "0000");
    assert_eq!(negative_zero.equals(&zero), Some(true));
    assert_eq!(padded_zero.equals(&zero), Some(true));
}

#[test]
fn bool_orders_false_below_true() {
    assert_strictly_ascending(ValueType::Bool, &["false", "true"]);
}

#[test]
fn date_orders_chronologically() {
    assert_strictly_ascending(
        ValueType::Date,
        &[
            "0000-01-01",
            "0000-02-29",
            "0001-01-01",
            "1900-02-28",
            "1900-03-01",
            "2000-02-29",
            "2024-01-31",
            "2024-02-01",
            "2024-02-29",
            "9999-12-31",
        ],
    );
}

#[test]
fn semver_orders_by_precedence() {
    // The precedence chain from the SemVer 2.0.0 specification, including
    // numeric identifiers comparing as numbers rather than as text.
    assert_strictly_ascending(
        ValueType::Semver,
        &[
            "1.0.0-alpha",
            "1.0.0-alpha.1",
            "1.0.0-alpha.beta",
            "1.0.0-beta",
            "1.0.0-beta.2",
            "1.0.0-beta.11",
            "1.0.0-rc.1",
            "1.0.0",
            "1.0.1",
            "1.1.0",
            "2.0.0",
        ],
    );
}

#[test]
fn semver_prerelease_case_is_retained_in_comparison() {
    let upper = header(ValueType::Semver, "1.0.0-RC");
    let lower = header(ValueType::Semver, "1.0.0-rc");
    assert_eq!(upper.equals(&lower), Some(false));
    // ASCII order, which is what SemVer specifies for alphanumeric
    // identifiers; nothing here folds case.
    assert_eq!(upper.compare(&lower), Some(Ordering::Less));
}

#[test]
fn semver_build_metadata_cannot_reach_a_comparison() {
    // §2.4 rejects build metadata outright, so there is no value carrying it
    // for a comparison to have to ignore.
    for source in ["1.0.0+build", "1.0.0+build.7", "1.0.0-rc.1+build"] {
        assert!(
            parse_header(ValueType::Semver, source).is_err(),
            "{source} should not become a value at all"
        );
    }
}

#[test]
fn dotted_orders_components_numerically_with_shorter_prefixes_first() {
    assert_strictly_ascending(
        ValueType::Dotted,
        &["0", "1", "1.0", "1.2", "1.2.0", "1.2.1", "1.10", "2", "10"],
    );
}

#[test]
fn dotted_equality_ignores_redundant_zeros() {
    let padded = header(ValueType::Dotted, "1.02");
    let plain = header(ValueType::Dotted, "1.2");
    assert_eq!(padded.equals(&plain), Some(true));
    assert_eq!(padded.compare(&plain), Some(Ordering::Equal));

    let heavily_padded = header(ValueType::Dotted, "0001.0000002.000");
    let terse = header(ValueType::Dotted, "1.2.0");
    assert_eq!(heavily_padded.equals(&terse), Some(true));

    // And the spelling still does not make `1.2` and `1.2.0` the same value.
    assert_eq!(plain.compare(&terse), Some(Ordering::Less));
}

#[test]
fn text_orders_by_code_point_and_distinguishes_case() {
    assert_strictly_ascending(ValueType::Text, &["", "A", "B", "a", "b", "é", "😀"]);

    let upper = header(ValueType::Text, "Version");
    let lower = header(ValueType::Text, "version");
    assert_eq!(upper.equals(&lower), Some(false));
    assert_eq!(upper.compare(&lower), Some(Ordering::Less));
}

#[test]
fn text_distinguishes_canonically_equivalent_spellings() {
    // Composed and decomposed e-acute mean the same to a reader and are
    // different values here, because §2.4 compares code points and nothing
    // in this module normalizes them.
    let composed = header(ValueType::Text, "\u{00e9}");
    let decomposed = header(ValueType::Text, "e\u{0301}");
    assert_eq!(composed.equals(&decomposed), Some(false));
    assert_eq!(composed.compare(&decomposed), Some(Ordering::Greater));

    // Code points crossing UTF-8 length boundaries still order by code
    // point, which is where a byte comparison could have diverged.
    assert_strictly_ascending(
        ValueType::Text,
        &[
            "\u{7f}",
            "\u{80}",
            "\u{7ff}",
            "\u{800}",
            "\u{ffff}",
            "\u{10000}",
        ],
    );
}

#[test]
fn values_of_different_types_are_never_compared() {
    let values = [
        header(ValueType::Int, "1"),
        header(ValueType::Bool, "true"),
        header(ValueType::Date, "2024-01-01"),
        header(ValueType::Semver, "1.0.0"),
        header(ValueType::Dotted, "1"),
        header(ValueType::Text, "1"),
    ];
    for left in &values {
        for right in &values {
            if left.value_type() == right.value_type() {
                assert!(left.compare(right).is_some());
                continue;
            }
            assert_eq!(
                left.compare(right),
                None,
                "{} against {}",
                left.value_type().as_str(),
                right.value_type().as_str()
            );
            assert_eq!(left.equals(right), None);
        }
    }
}

#[test]
fn a_frontmatter_value_compares_with_a_header_value_of_the_same_type() {
    // Both sources normalize to the same value, so a capture from one can be
    // ordered against a capture from the other.
    let from_header = header(ValueType::Dotted, "1.02.0");
    let from_frontmatter =
        frontmatter_string(ValueType::Dotted, "1.2.0").expect("a valid dotted string");
    assert_eq!(from_header.equals(&from_frontmatter), Some(true));

    let json = exact_json_number("-1");
    let integer = parse_frontmatter(
        ValueType::Int,
        FrontmatterValue::new(&json, ResolvedYamlKind::Integer),
    )
    .expect("a valid frontmatter integer");
    assert_eq!(header(ValueType::Int, "-01").equals(&integer), Some(true));
}

#[test]
fn canonical_spelling_drops_what_normalization_dropped() {
    for (value_type, source, expected) in [
        (ValueType::Int, "-01", "-1"),
        (ValueType::Int, "-0", "0"),
        (ValueType::Int, "0000042", "42"),
        (ValueType::Bool, "true", "true"),
        (ValueType::Bool, "false", "false"),
        (ValueType::Date, "0000-02-29", "0000-02-29"),
        (ValueType::Date, "9999-12-31", "9999-12-31"),
        (ValueType::Semver, "1.0.0-RC.1", "1.0.0-RC.1"),
        (ValueType::Dotted, "0001.0002.000", "1.2.0"),
        (ValueType::Dotted, "7", "7"),
        (ValueType::Text, "  0001  ", "  0001  "),
        (ValueType::Text, "MiXeD", "MiXeD"),
    ] {
        assert_eq!(
            header(value_type, source).canonical(),
            expected,
            "{} {source:?}",
            value_type.as_str()
        );
    }
}

#[test]
fn canonically_equal_spellings_reparse_to_the_same_value() {
    for (value_type, source) in [
        (ValueType::Int, "-01"),
        (ValueType::Int, "-0"),
        (ValueType::Date, "0000-02-29"),
        (ValueType::Semver, "1.0.0-RC.1"),
        (ValueType::Dotted, "0001.0002.000"),
        (ValueType::Text, "  kept  "),
    ] {
        let parsed = header(value_type, source);
        let reparsed = header(value_type, &parsed.canonical());
        assert_eq!(
            parsed.equals(&reparsed),
            Some(true),
            "{} {source:?}",
            value_type.as_str()
        );
    }
}

// ---------------------------------------------------------------------------
// Properties
//
// The deterministic tests above pin the cases §2.4 names. These state what
// must hold for every value: that no input can panic a parser, and that the
// single comparison relation really is an ordering rather than six ad-hoc
// answers that happen to agree on the examples.
// ---------------------------------------------------------------------------

/// Parses generated source strings, keeping only what the type admits.
///
/// Values are generated through the parsers rather than assembled from
/// normalized parts, so every generated value is one the kernel can actually
/// produce.
fn parsed_as(
    value_type: ValueType,
    sources: impl Strategy<Value = String>,
) -> impl Strategy<Value = TypedValue> {
    sources.prop_filter_map("the source parses", move |source| {
        parse_header(value_type, &source).ok()
    })
}

fn arbitrary_int() -> impl Strategy<Value = TypedValue> {
    let sources = prop_oneof![
        // A narrow range so independent draws collide, which is what makes
        // equality and transitivity worth checking.
        (-4i64..=4).prop_map(|value| value.to_string()),
        any::<i64>().prop_map(|value| value.to_string()),
        // Redundant spellings of values the narrow range also produces.
        (0usize..=6, -4i64..=4).prop_map(|(zeros, value)| {
            let sign = if value < 0 { "-" } else { "" };
            format!("{sign}{}{}", "0".repeat(zeros), value.abs())
        }),
        Just(i64::MIN.to_string()),
        Just(i64::MAX.to_string()),
    ];
    parsed_as(ValueType::Int, sources)
}

fn arbitrary_bool() -> impl Strategy<Value = TypedValue> {
    let sources = any::<bool>().prop_map(|value| value.to_string());
    parsed_as(ValueType::Bool, sources)
}

fn arbitrary_date() -> impl Strategy<Value = TypedValue> {
    // The whole range including year zero, with impossible days filtered by
    // the parser rather than by a second calendar written here.
    let sources = (0u16..=9999, 1u8..=12, 1u8..=31)
        .prop_map(|(year, month, day)| format!("{year:04}-{month:02}-{day:02}"));
    parsed_as(ValueType::Date, sources)
}

fn arbitrary_semver() -> impl Strategy<Value = TypedValue> {
    let identifier = prop_oneof![
        Just("alpha".to_owned()),
        Just("beta".to_owned()),
        Just("rc".to_owned()),
        Just("RC".to_owned()),
        Just("0".to_owned()),
        Just("1".to_owned()),
        Just("2".to_owned()),
        Just("11".to_owned()),
        Just(u64::MAX.to_string()),
    ];
    let prerelease = prop::option::of(prop::collection::vec(identifier, 1..=3));
    let sources =
        (0u64..=3, 0u64..=3, 0u64..=3, prerelease).prop_map(|(major, minor, patch, prerelease)| {
            let version = format!("{major}.{minor}.{patch}");
            match prerelease {
                Some(identifiers) => format!("{version}-{}", identifiers.join(".")),
                None => version,
            }
        });
    parsed_as(ValueType::Semver, sources)
}

fn arbitrary_dotted() -> impl Strategy<Value = TypedValue> {
    let component = prop_oneof![
        (0u32..=3, 0usize..=3).prop_map(|(value, zeros)| format!("{}{value}", "0".repeat(zeros))),
        any::<u32>().prop_map(|value| value.to_string()),
    ];
    let sources =
        prop::collection::vec(component, 1..=4).prop_map(|components| components.join("."));
    parsed_as(ValueType::Dotted, sources)
}

fn arbitrary_text() -> impl Strategy<Value = TypedValue> {
    let sources = prop_oneof![
        // A small alphabet, so distinct draws are sometimes equal and
        // sometimes differ only by case or by combining marks.
        "[aAbBé\u{0301}e]{0,4}".prop_map(String::from),
        any::<String>(),
    ];
    parsed_as(ValueType::Text, sources)
}

fn values_of_type(value_type: ValueType) -> BoxedStrategy<TypedValue> {
    match value_type {
        ValueType::Int => arbitrary_int().boxed(),
        ValueType::Bool => arbitrary_bool().boxed(),
        ValueType::Date => arbitrary_date().boxed(),
        ValueType::Semver => arbitrary_semver().boxed(),
        ValueType::Dotted => arbitrary_dotted().boxed(),
        ValueType::Text => arbitrary_text().boxed(),
    }
}

fn arbitrary_value() -> impl Strategy<Value = TypedValue> {
    prop::sample::select(ValueType::ALL.to_vec()).prop_flat_map(values_of_type)
}

fn same_type_pair() -> impl Strategy<Value = (TypedValue, TypedValue)> {
    prop::sample::select(ValueType::ALL.to_vec())
        .prop_flat_map(|value_type| (values_of_type(value_type), values_of_type(value_type)))
}

fn same_type_triple() -> impl Strategy<Value = (TypedValue, TypedValue, TypedValue)> {
    prop::sample::select(ValueType::ALL.to_vec()).prop_flat_map(|value_type| {
        (
            values_of_type(value_type),
            values_of_type(value_type),
            values_of_type(value_type),
        )
    })
}

proptest! {
    /// No string, however malformed, can panic a header parser: every one
    /// either becomes a value of the requested type or a structured failure.
    #[test]
    fn header_parsers_are_total_for_arbitrary_strings(source in any::<String>()) {
        for value_type in ValueType::ALL {
            if let Ok(parsed) = parse_header(value_type, &source) {
                prop_assert_eq!(parsed.value_type(), value_type);
            }
        }
    }

    /// The same holds through the frontmatter entry point, which also has to
    /// agree with the header one: given the same string, the two either both
    /// succeed on the same value or both fail the same way.
    #[test]
    fn frontmatter_string_parsers_are_total_for_arbitrary_strings(source in any::<String>()) {
        let json = Value::String(source.clone());
        for value_type in ValueType::ALL {
            let supplied = FrontmatterValue::new(&json, ResolvedYamlKind::String);
            let from_frontmatter = parse_frontmatter(value_type, supplied);
            match value_type {
                // A YAML string is not an integer or a boolean, whatever it
                // happens to spell.
                ValueType::Int | ValueType::Bool => {
                    prop_assert_eq!(
                        from_frontmatter.map(|_| ()),
                        Err(ParseFailure::KindMismatch {
                            expected: value_type.frontmatter_kind(),
                            actual: ResolvedYamlKind::String,
                        })
                    );
                }
                _ => match (from_frontmatter, parse_header(value_type, &source)) {
                    (Ok(frontmatter_value), Ok(header_value)) => {
                        prop_assert_eq!(frontmatter_value.equals(&header_value), Some(true));
                    }
                    (Err(frontmatter_failure), Err(header_failure)) => {
                        prop_assert_eq!(frontmatter_failure, header_failure);
                    }
                    _ => prop_assert!(false, "the two sources disagreed on {:?}", source),
                },
            }
        }
    }

    /// Reversing the arguments reverses the answer, so no pair has two
    /// stories about which of them is larger.
    #[test]
    fn comparison_is_antisymmetric_on_normalized_values((left, right) in same_type_pair()) {
        prop_assert_eq!(
            left.compare(&right),
            right.compare(&left).map(Ordering::reverse)
        );
    }

    /// A chain of comparisons cannot fold back on itself.
    #[test]
    fn comparison_is_transitive_on_normalized_values(
        (first, second, third) in same_type_triple()
    ) {
        let first_to_second = first.compare(&second);
        let second_to_third = second.compare(&third);
        let ascending = |ordering: Option<Ordering>| {
            matches!(ordering, Some(Ordering::Less | Ordering::Equal))
        };
        if ascending(first_to_second) && ascending(second_to_third) {
            prop_assert!(
                ascending(first.compare(&third)),
                "{:?} <= {:?} <= {:?} did not order the ends",
                first.canonical(),
                second.canonical(),
                third.canonical()
            );
        }
    }

    /// Equality is the comparison relation reaching `Equal`, and nothing
    /// else: there is no pair the two disagree about.
    #[test]
    fn equality_is_exactly_comparison_equality(
        (left, right) in (arbitrary_value(), arbitrary_value())
    ) {
        prop_assert_eq!(
            left.equals(&right),
            left.compare(&right).map(|ordering| ordering == Ordering::Equal)
        );
        if left.value_type() != right.value_type() {
            prop_assert_eq!(left.compare(&right), None);
            prop_assert_eq!(left.equals(&right), None);
        }
    }

    /// Within a type the relation is total and every value equals itself, so
    /// a `None` from `compare` always means the types differed.
    #[test]
    fn comparison_is_reflexive_and_total_within_each_type(
        (left, right) in same_type_pair()
    ) {
        prop_assert_eq!(left.compare(&left), Some(Ordering::Equal));
        prop_assert_eq!(left.equals(&left), Some(true));
        prop_assert!(left.compare(&right).is_some());
        prop_assert!(left.equals(&right).is_some());
        prop_assert_eq!(left.value_type(), right.value_type());
    }
}

#[test]
fn frontmatter_integer_kind_with_a_non_integral_number_is_a_kind_mismatch() {
    // The handoff contract says a disagreeing `value`/`yaml_kind` pair comes
    // back as a kind failure. A number carrying a fraction or an exponent
    // was never a whole number, so the disagreement is real and is reported
    // against the float the node was actually written as, rather than
    // arriving as a malformed integer.
    for spelling in [
        "1.2", "1.0", "-0.5", "0.0", "1e3", "1.5e2", "1E+3", "-2.5e-3",
    ] {
        let json = exact_json_number(spelling);
        assert_eq!(
            frontmatter_outcome(
                ValueType::Int,
                FrontmatterValue::new(&json, ResolvedYamlKind::Integer)
            ),
            Err(ParseFailure::KindMismatch {
                expected: ResolvedYamlKind::Integer,
                actual: ResolvedYamlKind::Float
            }),
            "frontmatter int {spelling}"
        );
    }
}

#[test]
fn frontmatter_integer_kind_still_separates_the_bound_from_a_kind_failure() {
    // A whole-number spelling past the bound is a bound failure, not a kind
    // one: the node really was an integer, just not one that fits.
    let json = exact_json_number("9223372036854775808");
    assert_eq!(
        frontmatter_outcome(
            ValueType::Int,
            FrontmatterValue::new(&json, ResolvedYamlKind::Integer)
        ),
        Err(ParseFailure::BoundOverflow {
            component: BoundComponent::Int
        })
    );
}

#[test]
fn a_disagreeing_number_is_reported_by_its_spelling_for_every_type() {
    // The same reading applies wherever a number reaches the shape check,
    // not only on the `int` path: `bool` sees the kind it was handed.
    let integral = exact_json_number("42");
    let fractional = exact_json_number("4.2");
    assert_eq!(
        frontmatter_outcome(
            ValueType::Bool,
            FrontmatterValue::new(&integral, ResolvedYamlKind::Boolean)
        ),
        Err(ParseFailure::KindMismatch {
            expected: ResolvedYamlKind::Boolean,
            actual: ResolvedYamlKind::Integer
        })
    );
    assert_eq!(
        frontmatter_outcome(
            ValueType::Bool,
            FrontmatterValue::new(&fractional, ResolvedYamlKind::Boolean)
        ),
        Err(ParseFailure::KindMismatch {
            expected: ResolvedYamlKind::Boolean,
            actual: ResolvedYamlKind::Float
        })
    );
    // A string type reached with a number reports the same distinction.
    assert_eq!(
        frontmatter_outcome(
            ValueType::Semver,
            FrontmatterValue::new(&fractional, ResolvedYamlKind::String)
        ),
        Err(ParseFailure::KindMismatch {
            expected: ResolvedYamlKind::String,
            actual: ResolvedYamlKind::Float
        })
    );
}

#[test]
fn the_contract_operations_are_reachable_from_the_rest_of_the_crate() {
    // The kernel is consumed by sibling modules, so the entry points and the
    // operations on a parsed value are crate-visible rather than private to
    // this module. This test exists to state that: it uses each of them the
    // way a caller outside the module would.
    let value_type = ValueType::from_name("dotted").expect("`dotted` is a capture type");
    assert_eq!(value_type.as_str(), "dotted");
    assert_eq!(value_type.frontmatter_kind(), ResolvedYamlKind::String);

    let from_header = parse_header(value_type, "1.02").expect("a valid dotted header capture");
    let json = Value::String("1.2".to_owned());
    let from_frontmatter = parse_frontmatter(
        value_type,
        FrontmatterValue::new(&json, ResolvedYamlKind::String),
    )
    .expect("a valid dotted frontmatter capture");

    assert_eq!(from_header.value_type(), ValueType::Dotted);
    assert_eq!(from_header.equals(&from_frontmatter), Some(true));
    assert_eq!(
        from_header.compare(&from_frontmatter),
        Some(Ordering::Equal)
    );
    assert_eq!(from_header.canonical(), "1.2");
}
