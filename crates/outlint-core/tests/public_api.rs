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

/// Pins the typed-value declaration surface a schema without `captures` or
/// `order` must expose: present, inspectable, and empty. The loader does not
/// normalize either declaration yet, so a schema that spells neither is the
/// only one that can be asserted about — which is exactly the invariant this
/// test exists to hold while the loader lanes land.
#[test]
fn schemas_without_typed_values_normalize_to_empty_capture_and_order_defaults() {
    let loaded = load_schema(
        r#"
version: 1
title: "*"
sections:
  - id: guide
    match: /(?<version>.+)/
"#,
    )
    .expect("schema is valid");

    let guide = &loaded.schema.outline[0].sections[0];
    assert!(guide.captures.is_empty());
    assert!(guide.order.is_empty());
    // The synthesized title rule has no source declaration to carry either.
    assert!(loaded.schema.outline[0].captures.is_empty());
    assert!(loaded.schema.outline[0].order.is_empty());
}

/// Pins the frontmatter policy's capture inspection: every variant answers,
/// and a policy with no declarations answers with an empty view rather than
/// forcing callers to match five ways.
#[test]
fn the_frontmatter_policy_answers_capture_questions_for_every_variant() {
    let loaded = load_schema("version: 1\nfrontmatter:\n  required: true\nsections: []\n")
        .expect("schema is valid");
    let policy = &loaded.schema.frontmatter;

    assert!(policy.is_required());
    assert!(!policy.is_forbidden());
    assert!(policy.schema().is_none());

    let captures = policy.captures();
    assert!(captures.is_empty());
    assert_eq!(captures.len(), 0);
    assert!(captures.declared().is_none());
    assert_eq!(captures.iter().count(), 0);

    let forbidden = load_schema("version: 1\nfrontmatter:\n  allow: false\nsections: []\n")
        .expect("schema is valid");
    assert!(forbidden.schema.frontmatter.is_forbidden());
    assert!(!forbidden.schema.frontmatter.is_required());
    assert!(forbidden.schema.frontmatter.captures().is_empty());
}

/// Pins §2.1's duplicate classification, which is observable only through the
/// public error kind. A repeat among a `captures` mapping's own keys is
/// `invalid-capture`; a repeat anywhere else — including inside one capture
/// declaration — stays `syntax`. Independent duplicates are collected rather
/// than stopping at the first.
#[test]
fn repeated_capture_keys_are_classified_apart_from_other_duplicate_keys() {
    let invalid = load_schema(
        "version: 1\nsections:\n  - match: /(?<a>.)(?<b>.)/\n    captures:\n      a: text\n      a: int\n",
    )
    .expect_err("a repeated capture key is refused");
    assert_eq!(invalid.errors.first.kind.to_string(), "invalid-capture");
    assert_eq!(invalid.errors.first.message, "duplicate capture name `a`");
    assert!(invalid.errors.rest.is_empty());

    // Inside one frontmatter capture declaration the general rule applies.
    let invalid = load_schema(
        "version: 1\nfrontmatter:\n  captures:\n    v:\n      type: text\n      type: int\nsections: []\n",
    )
    .expect_err("a repeated declaration key is refused");
    assert_eq!(invalid.errors.first.kind.to_string(), "syntax");

    // Two independent repeats are reported together, not one at a time.
    let invalid = load_schema(
        "version: 1\nfrontmatter:\n  captures:\n    v: {type: text}\n    v: {type: int}\n    w: {type: text}\n    w: {type: int}\nsections: []\n",
    )
    .expect_err("repeated capture keys are refused");
    let kinds = invalid
        .errors
        .iter()
        .map(|error| error.kind.to_string())
        .collect::<Vec<_>>();
    assert_eq!(kinds, ["invalid-capture", "invalid-capture"]);
}

/// Pins that `captures` and `order` are admitted as rule and frontmatter
/// fields rather than refused as unknown keys. Their contents are still
/// unvalidated and unnormalized here: the loader lanes that read them land
/// later, and this test exists so that admission is a deliberate state rather
/// than an accident nobody noticed.
#[test]
fn capture_and_order_declarations_are_admitted_without_being_normalized() {
    let loaded = load_schema(
        r#"
version: 1
frontmatter:
  captures:
    version:
      type: semver
sections:
  - match: /Release (?<version>.+)/
    captures:
      version: semver
    order:
      - by: version
        dir: desc
"#,
    )
    .expect("captures and order are known fields");

    assert!(loaded.schema.outline[0].sections[0].captures.is_empty());
    assert!(loaded.schema.outline[0].sections[0].order.is_empty());
    assert!(loaded.schema.frontmatter.captures().is_empty());
}

/// Pins the resolved-locator inspection surface by coercing each accessor to
/// a function pointer. Nothing is constructed: the current loader produces no
/// resolved locator, and pretending otherwise would test a binder that does
/// not exist. What this does hold is the shape — that the terminal kinds are
/// distinct types, that every form yields its original spelling, and that a
/// position leaves the model as arbitrary-precision decimal text rather than
/// a machine integer.
#[test]
fn the_resolved_locator_inspection_surface_is_pinned() {
    use outlint_core::{
        BoundRuleStep, CaptureName, NonEmpty, RefAnchor, ResolvedFrontmatterCapture,
        ResolvedFrontmatterQuery, ResolvedIntrinsicTextLocator, ResolvedOutlineLocator,
        ResolvedRuleCaptureLocator, ResolvedRuleLocator, RuleId, RuleIndex,
    };

    let _: fn(&ResolvedOutlineLocator) -> &str = ResolvedOutlineLocator::locator;
    let _: fn(&ResolvedOutlineLocator) -> RefAnchor = ResolvedOutlineLocator::anchor;

    let _: fn(&ResolvedRuleLocator) -> &str = ResolvedRuleLocator::locator;
    let _: fn(&ResolvedRuleLocator) -> RefAnchor = ResolvedRuleLocator::anchor;
    let _: fn(&ResolvedRuleLocator) -> &NonEmpty<BoundRuleStep> = ResolvedRuleLocator::steps;

    let _: fn(&BoundRuleStep) -> &RuleId = BoundRuleStep::id;
    let _: fn(&BoundRuleStep) -> RuleIndex = BoundRuleStep::index;
    // Decimal text, never `u64`: §4.4 gives `[i]` no upper bound.
    let _: fn(&BoundRuleStep) -> Option<String> = BoundRuleStep::position_digits;

    let _: fn(&ResolvedRuleCaptureLocator) -> &str = ResolvedRuleCaptureLocator::locator;
    let _: fn(&ResolvedRuleCaptureLocator) -> &[BoundRuleStep] =
        ResolvedRuleCaptureLocator::rule_steps;
    let _: fn(&ResolvedRuleCaptureLocator) -> &CaptureName = ResolvedRuleCaptureLocator::name;
    let _: fn(&ResolvedRuleCaptureLocator) -> &'static str = ResolvedRuleCaptureLocator::type_name;
    let _: fn(&ResolvedRuleCaptureLocator) -> Option<String> =
        ResolvedRuleCaptureLocator::position_digits;

    let _: fn(&ResolvedIntrinsicTextLocator) -> &str = ResolvedIntrinsicTextLocator::locator;
    let _: fn(&ResolvedIntrinsicTextLocator) -> &NonEmpty<BoundRuleStep> =
        ResolvedIntrinsicTextLocator::rule_steps;
    let _: fn(&ResolvedIntrinsicTextLocator) -> Option<String> =
        ResolvedIntrinsicTextLocator::position_digits;

    let _: fn(&ResolvedFrontmatterQuery) -> &str = ResolvedFrontmatterQuery::locator;
    let _: fn(&ResolvedFrontmatterQuery) -> &str = ResolvedFrontmatterQuery::query;
    let _: fn(&ResolvedFrontmatterQuery) -> Option<&outlint_core::FrontmatterScalar> =
        ResolvedFrontmatterQuery::equals;

    let _: fn(&ResolvedFrontmatterCapture) -> &str = ResolvedFrontmatterCapture::locator;
    let _: fn(&ResolvedFrontmatterCapture) -> &CaptureName = ResolvedFrontmatterCapture::name;
    let _: fn(&ResolvedFrontmatterCapture) -> &'static str = ResolvedFrontmatterCapture::type_name;
}

/// Pins the intended opacity of the kernel-backed model. None of these types
/// can be built from outside the crate, so no caller can construct a schema
/// that skips the loader's checks — and none of them exposes a JSONPath
/// provider type or an arbitrary-precision integer type in a signature.
#[test]
fn kernel_backed_values_stay_opaque_to_callers() {
    fn assert_inspectable<T: std::fmt::Debug + Clone + PartialEq>() {}

    assert_inspectable::<outlint_core::CaptureName>();
    assert_inspectable::<outlint_core::RuleCapture>();
    assert_inspectable::<outlint_core::FrontmatterCapture>();
    assert_inspectable::<outlint_core::FrontmatterCaptures>();
    assert_inspectable::<outlint_core::ValueOrderEntry>();
    assert_inspectable::<outlint_core::BoundRuleStep>();
    assert_inspectable::<outlint_core::ResolvedOutlineLocator>();
    assert_inspectable::<outlint_core::ResolvedFrontmatterQuery>();
    assert_inspectable::<outlint_core::ResolvedFrontmatterCapture>();
}

/// Pins the stable ids Typed Values adds, on both channels. §6.3 fixes the
/// spellings, and `outlint-disable` matches diagnostic ids as text, so a
/// spelling is a compatibility contract even while nothing emits it.
#[test]
fn the_typed_value_ids_have_their_specified_spellings() {
    use outlint_core::SchemaErrorKind;

    assert_eq!(DiagnosticId::InvalidValue.as_str(), "invalid-value");
    assert_eq!(DiagnosticId::MissingValue.as_str(), "missing-value");
    assert_eq!(DiagnosticId::OrderViolation.as_str(), "order-violation");
    assert_eq!(DiagnosticId::InvalidValue.to_string(), "invalid-value");
    assert_eq!(DiagnosticId::MissingValue.to_string(), "missing-value");
    assert_eq!(DiagnosticId::OrderViolation.to_string(), "order-violation");

    assert_eq!(SchemaErrorKind::InvalidCapture.as_str(), "invalid-capture");
    assert_eq!(SchemaErrorKind::InvalidOrder.as_str(), "invalid-order");
    assert_eq!(
        SchemaErrorKind::InvalidCapture.to_string(),
        "invalid-capture"
    );
    assert_eq!(SchemaErrorKind::InvalidOrder.to_string(), "invalid-order");
}

/// Pins the new schema-node addresses: that a capture is addressed by its
/// owning rule plus a name, an order entry by its owning rule plus a
/// position, and a frontmatter capture by a name alone — its named scope is
/// rooted at `fm`, not at any rule. The `Ord` these derive is what orders the
/// side-car node map, so the addresses are compared here too.
#[test]
fn the_new_schema_node_addresses_are_constructible_and_ordered() {
    use outlint_core::{
        CaptureName, CapturePath, ConstraintIndex, ConstraintPath, OrderEntryPath, OrderIndex,
        RuleIndex, RulePath, SchemaNode, ScopePath,
    };

    let rule = RulePath {
        scope: ScopePath(vec![RuleIndex(0)]),
        index: RuleIndex(1),
    };
    // §11.3 declaration order: rule, capture, frontmatter_capture,
    // order_entry, constraint. The derived `Ord` follows it, and a variant
    // appended rather than inserted would fail here.
    assert!(
        SchemaNode::Rule(rule.clone())
            < SchemaNode::OrderEntry(OrderEntryPath {
                rule: rule.clone(),
                order_index: OrderIndex(0),
            })
    );
    assert!(
        SchemaNode::OrderEntry(OrderEntryPath {
            rule,
            order_index: OrderIndex(0),
        }) < SchemaNode::Constraint(ConstraintPath {
            scope: ScopePath(vec![RuleIndex(0)]),
            index: ConstraintIndex(0),
        })
    );

    // A `CapturePath` and a `SchemaNode::FrontmatterCapture` both need a
    // validated `CaptureName`, which only the loader can make, so their field
    // types are pinned by coercion rather than by construction.
    let _: fn(&CapturePath) -> &RulePath = |path| &path.rule;
    let _: fn(&CapturePath) -> &CaptureName = |path| &path.name;
    let _: fn(&OrderEntryPath) -> OrderIndex = |path| path.order_index;
    let _: fn(CaptureName) -> SchemaNode = SchemaNode::FrontmatterCapture;
    let _: fn(CapturePath) -> SchemaNode = SchemaNode::Capture;
}
