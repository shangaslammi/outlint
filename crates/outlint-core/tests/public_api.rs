use outlint_core::{
    load_schema, parse_markdown, validate, ByteOffset, Diagnostic, DiagnosticId, Document,
    DocumentFrontmatter, FrontmatterAnchors, FrontmatterLocation, HeaderPath, MarkdownOptions,
    Options, PrepareValidationError, PreparedValidator, Schema, SchemaError, TextRange,
    ValidationError, ValidationOperationalError, ValueOrderDirection,
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
  - one_of: ["fm[$.count]=01", guide]
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
    let outlint_core::Proposition::FrontmatterQuery(reference) = &items.first else {
        panic!("expected a frontmatter query proposition")
    };
    // The wrapper and the query are retained exactly as written; only the
    // equality literal is normalized, through the YAML core-schema resolver.
    assert_eq!(reference.locator(), "fm[$.count]=01");
    assert_eq!(reference.query(), "$.count");
    let Some(outlint_core::FrontmatterScalar::Integer(value)) = reference.equals() else {
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
/// `order` must expose: present, inspectable, and empty. §2.1 makes both
/// declarations optional, so the absent case is a shape a caller meets on
/// most rules, and it must answer the same questions the declared case does
/// rather than making the fields optional in the model. The synthesized title
/// rule has no source declaration to carry either, so it answers the same way
/// for a second reason.
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

/// Pins the precedence between the schema's shape rule and §2.1's duplicate
/// classification: shape wins, and duplicate classification is never reached.
///
/// `1` and `01` both resolve to the integer one, so they are two spellings of
/// one key — yet inside a `captures` mapping they produce
/// `invalid-document-shape`, not `invalid-capture`. That is the specified
/// outcome, not an escape. A capture name is `[a-z][a-z0-9_]*` under §2.2, so
/// the duplicate-capture check's input is string keys; §6.3 requires that "a
/// check whose input could not be built MUST NOT be attempted", and a
/// non-string key has already failed the upstream rule that mapping keys are
/// strings. The schema is refused either way and no stable-id contract turns
/// on which refusal it gets, so this test exists to keep the precedence from
/// being "fixed" into a second classification path that could only reword an
/// already-doomed load.
#[test]
fn non_string_capture_keys_fail_the_shape_rule_before_duplicate_classification() {
    let invalid = load_schema(
        "version: 1\nsections:\n  - match: /(?<a>.)/\n    captures:\n      1: text\n      01: int\n",
    )
    .expect_err("a non-string mapping key is refused");

    assert_eq!(
        invalid.errors.first.kind.to_string(),
        "invalid-document-shape"
    );
    assert_eq!(invalid.errors.first.message, "mapping keys must be strings");
    assert!(invalid
        .errors
        .iter()
        .all(|error| error.kind.to_string() != "invalid-capture"));
}

/// Pins that a rule's `captures` and `order` declarations and the
/// frontmatter `captures` declaration all reach the public model
/// normalized — types resolved, directions, paths, and flags defaulted.
#[test]
fn rule_captures_and_order_reach_the_public_model() {
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

    let rule = &loaded.schema.outline[0].sections[0];
    let captures = rule
        .captures
        .iter()
        .map(|(name, capture)| (name.as_str(), capture.type_name()))
        .collect::<Vec<_>>();
    assert_eq!(captures, vec![("version", "semver")]);
    assert_eq!(rule.order.len(), 1);
    assert_eq!(rule.order[0].by.as_str(), "version");
    assert_eq!(rule.order[0].direction, ValueOrderDirection::Descending);
    assert!(!rule.order[0].strict);

    let captures = loaded.schema.frontmatter.captures();
    assert_eq!(captures.len(), 1);
    let (name, capture) = captures
        .iter()
        .next()
        .expect("the frontmatter declaration normalized");
    assert_eq!(name.as_str(), "version");
    assert_eq!(capture.type_name(), "semver");
    assert_eq!(capture.path_source(), "$['version']");
    assert!(!capture.is_required());
}

/// Pins the resolved-locator inspection surface by coercing each accessor to
/// a function pointer. Nothing is constructed here: the loader's own tests
/// build these through real schemas, and what this holds is the shape — that
/// the terminal kinds are distinct types, that every form yields its original
/// spelling, and that a position leaves the model as arbitrary-precision
/// decimal text rather than a machine integer.
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
/// spelling is a contract even before anything emits it.
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

/// Pins a positional rule reference as a caller actually receives one: loaded
/// from a schema, bound by the loader, and carried out of validation on an
/// unsatisfied constraint.
///
/// The coercion test above holds the accessors' shapes; this holds what they
/// answer. Three facts are the contract. The locator keeps the spelling the
/// schema wrote, `$.` and subscript included, rather than one reconstructed
/// from the bound index; the anchor records that the `$.` resolved at the
/// schema root; and §4.4 gives `[i]` "no upper bound", so a subscript wider
/// than `u64` reaches the caller as its exact decimal digits.
#[test]
fn a_positional_rule_reference_survives_binding_and_validation_intact() {
    use outlint_core::{DiagnosticReference, Matcher, RefAnchor, RuleIndex};

    let position = "1".repeat(40);
    let loaded = load_schema(&format!(
        "version: 1\nsections:\n  - id: alpha\n    match: Alpha\n  - id: beta\n    \
         match: Beta\nconstraints:\n  - any_of: [\"$.alpha[{position}]\", beta]\n"
    ))
    .expect("schema is valid");
    let document = parse_markdown("# Guide\n", MarkdownOptions::default());
    let reported = validate(&loaded.schema, &document).expect("validation completes");

    assert_eq!(reported.len(), 1);
    assert_eq!(reported[0].id, DiagnosticId::AnyOf);
    let DiagnosticReference::Rule { locator, matcher } = &reported[0].references[0] else {
        panic!("the first reference is the positional rule locator")
    };
    assert_eq!(locator.locator(), format!("$.alpha[{position}]"));
    assert_eq!(locator.anchor(), RefAnchor::SchemaRoot);
    assert!(locator.steps().rest.is_empty());
    let step = &locator.steps().first;
    assert_eq!(step.id().as_str(), "alpha");
    assert_eq!(step.index(), RuleIndex(0));
    // Decimal text, never a machine integer: 40 ones do not fit in a `u64`.
    assert_eq!(step.position_digits(), Some(position));
    let Matcher::Exact(text) = matcher else {
        panic!("the referenced rule's matcher travels with the reference")
    };
    assert_eq!(text.0, "Alpha");
}

/// Pins the shape of an `invalid-value` raised by a rule capture: §6.2 targets
/// "`header` whose capture is invalid" and attributes it "to that capture
/// declaration", so the schema node is the capture's own address and not the
/// owning rule's. The constraint-only fields stay empty, which is what keeps a
/// value diagnostic distinguishable from a constraint one.
#[test]
fn a_rule_capture_invalid_value_names_its_header_and_capture_declaration() {
    use outlint_core::{DiagnosticTarget, RuleIndex, SchemaNode};

    let loaded = load_schema(
        "version: 1\nsections:\n  - match: \"/Release (?<version>.+)/\"\n    repeat: 0..n\n    \
         captures:\n      version: semver\n",
    )
    .expect("schema is valid");
    let document = parse_markdown(
        "# Guide\n## Release 1.0.0+build.7\n",
        MarkdownOptions::default(),
    );
    let reported = validate(&loaded.schema, &document).expect("validation completes");

    assert_eq!(reported.len(), 1);
    let diagnostic = &reported[0];
    assert_eq!(diagnostic.id, DiagnosticId::InvalidValue);
    assert_eq!(
        diagnostic.target,
        DiagnosticTarget::Header(HeaderPath(vec![
            "Guide".into(),
            "Release 1.0.0+build.7".into()
        ]))
    );
    let Some(SchemaNode::Capture(path)) = &diagnostic.schema_node else {
        panic!("a rule-capture diagnostic is attributed to its capture declaration")
    };
    assert_eq!(path.name.as_str(), "version");
    assert_eq!(path.rule.index, RuleIndex(0));
    assert!(path.rule.scope.0.is_empty());
    // §6.2 requires the message to identify the expected type and the
    // responsible capture; the prose around them is not pinned.
    assert!(
        diagnostic.message.contains("version") && diagnostic.message.contains("semver"),
        "the message names the capture and its type: {}",
        diagnostic.message
    );
    assert!(diagnostic.involved_headers.is_empty());
    assert!(diagnostic.references.is_empty());
}

/// Pins the shape of a `missing-value`: §6.2 targets "`frontmatter` with the
/// absent capture's pointer when one can be normalized", and attributes it to
/// the frontmatter capture declaration, whose named scope is rooted at `fm`
/// and so needs no rule coordinates. The pointer is the normalized default
/// path, built from the capture's name rather than copied from a provider.
#[test]
fn a_frontmatter_missing_value_names_its_block_pointer_and_declaration() {
    use outlint_core::{DiagnosticTarget, FrontmatterLineRange, SchemaNode};

    let loaded = load_schema(
        "version: 1\ntitle: null\nsections: []\nfrontmatter:\n  captures:\n    version:\n      \
         type: semver\n      required: true\n",
    )
    .expect("schema is valid");
    let document = parse_markdown("---\nother: 1\n---\n", MarkdownOptions::default());
    let reported = validate(&loaded.schema, &document).expect("validation completes");

    assert_eq!(reported.len(), 1);
    let diagnostic = &reported[0];
    assert_eq!(diagnostic.id, DiagnosticId::MissingValue);
    let DiagnosticTarget::Frontmatter { block: Some(block) } = &diagnostic.target else {
        panic!("a capture diagnostic targets the frontmatter block")
    };
    assert_eq!(
        block.line_range,
        FrontmatterLineRange {
            start_line: 1,
            end_line: 3
        }
    );
    assert_eq!(block.json_pointer.as_deref(), Some("/version"));
    let Some(SchemaNode::FrontmatterCapture(name)) = &diagnostic.schema_node else {
        panic!("a frontmatter-capture diagnostic is attributed to its declaration")
    };
    assert_eq!(name.as_str(), "version");
    assert!(diagnostic.involved_headers.is_empty());
    assert!(diagnostic.references.is_empty());
}

/// Pins the shape of an `order-violation`: §6.2 targets and anchors "the
/// violating adjacent pair's second header", §6.3 attributes it to the order
/// entry, and §6.2 requires it to list "exactly the first and second headers
/// of its violating adjacent pair, in that order".
#[test]
fn an_order_violation_names_its_entry_and_exactly_its_adjacent_pair() {
    use outlint_core::{DiagnosticTarget, OrderIndex, RuleIndex, SchemaNode};

    let loaded = load_schema(
        "version: 1\nsections:\n  - match: \"/V (?<v>.+)/\"\n    repeat: 0..n\n    \
         captures:\n      v: int\n    order:\n      - by: v\n",
    )
    .expect("schema is valid");
    let document = parse_markdown("# Guide\n## V 2\n## V 1\n", MarkdownOptions::default());
    let reported = validate(&loaded.schema, &document).expect("validation completes");

    assert_eq!(reported.len(), 1);
    let diagnostic = &reported[0];
    assert_eq!(diagnostic.id, DiagnosticId::OrderViolation);
    assert_eq!(
        diagnostic.target,
        DiagnosticTarget::Header(HeaderPath(vec!["Guide".into(), "V 1".into()]))
    );
    let Some(SchemaNode::OrderEntry(path)) = &diagnostic.schema_node else {
        panic!("an order violation is attributed to its order entry")
    };
    assert_eq!(path.order_index, OrderIndex(0));
    assert_eq!(path.rule.index, RuleIndex(0));
    assert!(path.rule.scope.0.is_empty());
    assert_eq!(
        diagnostic
            .involved_headers
            .iter()
            .map(|header| header.path.to_string())
            .collect::<Vec<_>>(),
        ["Guide > V 2", "Guide > V 1"]
    );
    // The anchor is the second header of the pair, not the first.
    assert_eq!(diagnostic.location, diagnostic.involved_headers[1].location);
    assert_eq!(diagnostic.location.line, 3);
}

/// Pins the `fm[...]` half of §11.3's reference kinds on the one diagnostic
/// that carries a single query: an `invalid-value` from a boolean read. §6.2
/// attributes it to the containing constraint, which does not say which of
/// that constraint's queries failed, so the responsible query travels as a
/// reference — keeping both the locator spelling the schema wrote and the
/// query inside the brackets.
#[test]
fn an_invalid_boolean_read_carries_the_query_that_failed() {
    use outlint_core::{DiagnosticReference, DiagnosticTarget};

    let loaded = load_schema(
        "version: 1\ntitle: null\nsections:\n  - id: body\n    match: Body\n    \
         required: true\nconstraints:\n  - requires: { if: body, then: \"fm[$.flag]\" }\n",
    )
    .expect("schema is valid");
    // The rule binds `h2`: with `title: null` the sugar's root still stands at
    // level 1, so an `h1` would be a header the schema denies.
    let document = parse_markdown(
        "---\nflag: \"text\"\n---\n## Body\n",
        MarkdownOptions::default(),
    );
    let reported = validate(&loaded.schema, &document).expect("validation completes");

    assert_eq!(reported.len(), 1);
    let diagnostic = &reported[0];
    assert_eq!(diagnostic.id, DiagnosticId::InvalidValue);
    let DiagnosticTarget::Frontmatter { block: Some(block) } = &diagnostic.target else {
        panic!("a boolean-read failure targets the frontmatter block")
    };
    assert_eq!(block.json_pointer.as_deref(), Some("/flag"));
    let [DiagnosticReference::FrontmatterQuery(query)] = diagnostic.references.as_slice() else {
        panic!("exactly the responsible query travels with the diagnostic")
    };
    assert_eq!(query.locator(), "fm[$.flag]");
    assert_eq!(query.query(), "$.flag");
    // A bare read compares against nothing, so there is no equality literal.
    assert!(query.equals().is_none());
}

/// Pins the two remaining §11.3 reference kinds on one failed constraint, so
/// that `fm.<name>` and a rule locator are matched and inspected side by side
/// and in the order the constraint declared them.
#[test]
fn a_failed_constraint_carries_its_rule_and_frontmatter_capture_references() {
    use outlint_core::{DiagnosticReference, Matcher, RefAnchor};

    let loaded = load_schema(
        "version: 1\ntitle: null\nsections:\n  - id: body\n    match: Body\n    \
         required: false\nfrontmatter:\n  captures:\n    released:\n      \
         type: bool\nconstraints:\n  - any_of: [body, \"fm.released\"]\n",
    )
    .expect("schema is valid");
    let document = parse_markdown("---\nreleased: false\n---\n", MarkdownOptions::default());
    let reported = validate(&loaded.schema, &document).expect("validation completes");

    assert_eq!(reported.len(), 1);
    let references = &reported[0].references;
    assert_eq!(reported[0].id, DiagnosticId::AnyOf);
    assert_eq!(references.len(), 2);

    let DiagnosticReference::Rule { locator, matcher } = &references[0] else {
        panic!("the first reference is the rule the constraint named first")
    };
    assert_eq!(locator.locator(), "body");
    // A bare relative name resolves where the constraint is attached.
    assert_eq!(locator.anchor(), RefAnchor::CurrentScope);
    assert_eq!(locator.steps().first.id().as_str(), "body");
    assert_eq!(locator.steps().first.position_digits(), None);
    let Matcher::Exact(text) = matcher else {
        panic!("the rule's matcher travels with the reference")
    };
    assert_eq!(text.0, "Body");

    let DiagnosticReference::FrontmatterCapture(capture) = &references[1] else {
        panic!("the second reference is the declared frontmatter capture")
    };
    assert_eq!(capture.locator(), "fm.released");
    assert_eq!(capture.name().as_str(), "released");
    assert_eq!(capture.type_name(), "bool");
}
