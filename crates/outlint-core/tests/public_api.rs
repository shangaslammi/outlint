use outlint_core::{
    load_schema, parse_markdown, validate, ByteOffset, Diagnostic, DiagnosticId, Document,
    DocumentFrontmatter, FrontmatterAnchors, FrontmatterLocation, HeaderPath, MarkdownOptions,
    Options, PrepareValidationError, PreparedValidator, Schema, SchemaError, TextRange,
    ValidationError, ValidationOperationalError,
};

#[test]
fn frontmatter_mapping_value_has_the_public_json_object_type() {
    let frontmatter = DocumentFrontmatter::Mapping {
        value: serde_json::Map::new(),
        location: FrontmatterLocation {
            range: TextRange {
                start: ByteOffset(0),
                end: ByteOffset(0),
            },
            start_line: 1,
            end_line: 1,
        },
        anchors: FrontmatterAnchors::default(),
    };
    let DocumentFrontmatter::Mapping { value, .. } = frontmatter else {
        panic!("constructed the mapping variant")
    };

    let _: serde_json::Map<String, serde_json::Value> = value;
}

#[test]
fn public_errors_implement_the_standard_error_trait() {
    fn assert_error<T: std::error::Error>() {}

    assert_error::<outlint_core::InvalidSchema>();
    assert_error::<SchemaError>();
    assert_error::<PrepareValidationError>();
    assert_error::<ValidationOperationalError>();
    assert_error::<ValidationError>();
}

/// Pins the two validation signatures. If either result type changes, these
/// coercions stop compiling and the change has to be made deliberately.
#[test]
fn the_validation_signatures_are_pinned() {
    let prepared_validate: fn(
        &PreparedValidator,
        &Document,
    ) -> Result<Vec<Diagnostic>, ValidationOperationalError> = PreparedValidator::validate;
    let one_shot_validate: fn(&Schema, &Document) -> Result<Vec<Diagnostic>, ValidationError> =
        validate;
    let prepare: fn(&Schema) -> Result<PreparedValidator, PrepareValidationError> =
        PreparedValidator::new;

    let loaded = load_schema("version: 1\ntitle: '*'\nsections: []\n").expect("schema is valid");
    let document = parse_markdown("# Guide\n", MarkdownOptions::default());

    let validator = prepare(&loaded.schema).expect("the loaded schema compiles");
    assert!(prepared_validate(&validator, &document)
        .expect("validation completes")
        .is_empty());
    assert!(one_shot_validate(&loaded.schema, &document)
        .expect("preparation and validation both succeed")
        .is_empty());
}

#[test]
fn the_validation_error_channel_separates_preparation_from_operation() {
    let preparation = PrepareValidationError {
        message: "schema did not compile".to_owned(),
    };
    let operational = ValidationOperationalError::new("validation did not complete");

    // Both halves reach `ValidationError` through `From`.
    let from_preparation = ValidationError::from(preparation.clone());
    let from_operational = ValidationError::from(operational.clone());
    assert_eq!(
        from_preparation,
        ValidationError::Preparation(preparation.clone())
    );
    assert_eq!(
        from_operational,
        ValidationError::Operational(operational.clone())
    );

    // The wrapper is transparent for display and exposes the contained error
    // as its source.
    assert_eq!(from_preparation.to_string(), "schema did not compile");
    assert_eq!(from_operational.to_string(), "validation did not complete");
    assert_eq!(operational.message, "validation did not complete");

    let source = std::error::Error::source(&from_operational).expect("a source is exposed");
    assert_eq!(source.to_string(), "validation did not complete");
    assert!(std::error::Error::source(&from_preparation).is_some());

    // The `?` operator carries either half into the combined type.
    fn preparation_failure() -> Result<(), ValidationError> {
        Err(PrepareValidationError {
            message: "schema did not compile".to_owned(),
        })?;
        unreachable!()
    }
    fn operational_failure() -> Result<(), ValidationError> {
        Err(ValidationOperationalError::new(
            "validation did not complete",
        ))?;
        unreachable!()
    }
    assert!(matches!(
        preparation_failure(),
        Err(ValidationError::Preparation(_))
    ));
    assert!(matches!(
        operational_failure(),
        Err(ValidationError::Operational(_))
    ));
}

#[test]
fn public_display_implementations_are_concise_and_stable() {
    assert_eq!(DiagnosticId::MissingSection.to_string(), "missing-section");
    assert_eq!(
        HeaderPath(vec!["Guide".into(), "Usage".into()]).to_string(),
        "Guide > Usage"
    );

    let invalid =
        load_schema("version: 99\nsections: []\n").expect_err("version 99 is unsupported");
    assert_eq!(invalid.errors.first.kind.to_string(), "unsupported-version");
    assert_eq!(
        invalid.errors.first.to_string(),
        "unsupported-version: unsupported schema version 99; expected 1"
    );
    assert_eq!(invalid.to_string(), invalid.errors.first.to_string());
}

#[test]
fn normalized_newtypes_are_inspectable_without_exposing_construction() {
    let loaded = load_schema(
        r#"
version: 1
title: "*"
sections:
  - id: guide
    match: "Guide*"
  - match: /Usage/
constraints:
  - one_of: [fm.count=01, guide]
"#,
    )
    .expect("schema is valid");

    let guide = &loaded.schema.outline[0].sections[0];
    assert_eq!(guide.id.as_ref().map(|id| id.as_str()), Some("guide"));
    let outlint_core::Matcher::Glob(glob) = &guide.matcher else {
        panic!("expected a glob matcher")
    };
    assert_eq!(glob.as_str(), "Guide*");

    let outlint_core::Matcher::Regex(regex) = &loaded.schema.outline[0].sections[1].matcher else {
        panic!("expected a regex matcher")
    };
    assert_eq!(regex.as_str(), "Usage");

    let outlint_core::Constraint::OneOf(items) = &loaded.schema.outline[0].constraints[0] else {
        panic!("expected one_of")
    };
    let outlint_core::Proposition::Frontmatter(reference) = &items.first else {
        panic!("expected a frontmatter proposition")
    };
    assert_eq!(reference.path.first.as_str(), "count");
    let Some(outlint_core::FrontmatterScalar::Integer(value)) = &reference.equals else {
        panic!("expected an integer equality")
    };
    assert_eq!(value.as_str(), "1");
}

#[test]
fn semantic_options_default_to_the_specification_values() {
    let defaults = Options::default();
    assert!(!defaults.match_case);
    assert!(defaults.strip_inline_markup);
    assert!(!defaults.allow_skipped_levels);
    assert!(defaults.ordered_sections);

    let customized = defaults
        .with_match_case(true)
        .with_strip_inline_markup(false)
        .with_allow_skipped_levels(true)
        .with_ordered_sections(false);
    assert!(customized.match_case);
    assert!(!customized.strip_inline_markup);
    assert!(customized.allow_skipped_levels);
    assert!(!customized.ordered_sections);
}
