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
