//! Structural checks over the schema document's converted JSON value.

use serde_json::Value;

use crate::{
    ConstraintIndex, ConstraintPath, RelatedLocation, RuleIndex, RulePath, SchemaErrorKind,
    ScopePath, SourceRange,
};

use super::{JsonMap, Loader, RangeKey};

pub(super) const DOCUMENT_FIELDS: &[&str] = &[
    "version",
    "title",
    "options",
    "frontmatter",
    "outline",
    "sections",
    "constraints",
];
pub(super) const OPTION_FIELDS: &[&str] = &[
    "match_case",
    "strip_inline_markup",
    "allow_skipped_levels",
    "ordered_sections",
];
pub(super) const FRONTMATTER_FIELDS: &[&str] = &["required", "allow", "schema", "captures"];
pub(super) const RULE_FIELDS: &[&str] = &[
    "id",
    "match",
    "allow",
    "required",
    "repeat",
    "strict",
    "ordered",
    "sections",
    "constraints",
    "captures",
    "order",
];

/// The key whose mapping declares captures, in a rule or in `frontmatter`.
///
/// §2.1 gives repeated keys of *this* mapping their own classification, and
/// the range walk addresses each of its entries, so the spelling is named
/// once here rather than repeated at each of those sites.
pub(super) const CAPTURES_FIELD: &str = "captures";

/// The key whose list declares a rule's value ordering (§3.8).
pub(super) const ORDER_FIELD: &str = "order";

impl Loader {
    pub(super) fn validate_document_shape(&mut self, value: &Value) {
        let Some(mapping) = value.as_object() else {
            self.shape_error_at(self.document_range, "schema document must be a mapping");
            return;
        };
        self.validate_known_fields(mapping, DOCUMENT_FIELDS, self.document_range);
        self.validate_required_field(mapping, "version", self.document_range);
        let outline_conflict = if mapping.contains_key("outline") {
            self.validate_outline_exclusivity(mapping)
        } else {
            self.validate_required_field(mapping, "sections", self.document_range);
            false
        };

        if let Some(value) = mapping.get("version") {
            if !is_yaml_integer(value) {
                self.shape_error_at(
                    self.range(RangeKey::DocumentField("version".into())),
                    "version must be an integer that fits in 64 bits and cannot be null",
                );
            }
        }
        if let Some(value) = mapping.get("title") {
            if !matches!(value, Value::String(_) | Value::Null) {
                self.shape_error_at(
                    self.range(RangeKey::DocumentField("title".into())),
                    "title must be a string or null",
                );
            }
        }
        if let Some(value) = mapping.get("outline") {
            // On a conflict the outline forest holds no collected ranges
            // (`sections` keeps the shared nested-rule key space), so its
            // shape is left to the reload after the conflict is resolved.
            if !outline_conflict {
                let range = self.range(RangeKey::DocumentField("outline".into()));
                self.validate_outline_shape(value, range);
            }
        }
        if let Some(value) = mapping.get("options") {
            self.validate_options_shape(value);
        }
        if let Some(value) = mapping.get("frontmatter") {
            self.validate_frontmatter_shape(value);
        }
        if let Some(value) = mapping.get("sections") {
            let range = self.range(RangeKey::DocumentField("sections".into()));
            self.validate_rules_shape(value, &ScopePath(Vec::new()), range);
        }
        if let Some(value) = mapping.get("constraints") {
            let range = self.range(RangeKey::DocumentField("constraints".into()));
            self.validate_constraints_shape(value, &ScopePath(Vec::new()), range);
        }
    }

    fn validate_frontmatter_shape(&mut self, value: &Value) {
        let range = self.range(RangeKey::DocumentField("frontmatter".into()));
        let Some(mapping) = value.as_object() else {
            self.shape_error_at(range, "frontmatter must be a mapping and cannot be null");
            return;
        };
        self.validate_known_fields(mapping, FRONTMATTER_FIELDS, range);
        for field in ["required", "allow"] {
            if let Some(value) = mapping.get(field) {
                if !matches!(value, Value::Bool(_)) {
                    self.shape_error_at(
                        self.range(RangeKey::FrontmatterField(field.into())),
                        format!("frontmatter.{field} must be a bool and cannot be null"),
                    );
                }
            }
        }
        if let Some(value) = mapping.get("schema") {
            if !matches!(value, Value::String(_) | Value::Object(_)) {
                self.shape_error_at(
                    self.range(RangeKey::FrontmatterField("schema".into())),
                    "frontmatter.schema must be a path string or mapping and cannot be null",
                );
            }
        }
    }

    fn validate_options_shape(&mut self, value: &Value) {
        let range = self.range(RangeKey::DocumentField("options".into()));
        let Some(mapping) = value.as_object() else {
            self.shape_error_at(range, "options must be a mapping and cannot be null");
            return;
        };
        self.validate_known_fields(mapping, OPTION_FIELDS, range);
        for field in OPTION_FIELDS.iter().copied() {
            if let Some(value) = mapping.get(field) {
                if !matches!(value, Value::Bool(_)) {
                    self.shape_error_at(
                        self.range(RangeKey::OptionField(field.into())),
                        format!("options.{field} must be a bool and cannot be null"),
                    );
                }
            }
        }
    }

    /// Rejects `outline` combined with either half of its sugar spelling.
    ///
    /// `title` + `sections` is defined as sugar for a single-rule `outline`,
    /// so a document declaring both forms has said the same thing twice and
    /// possibly differently. The error anchors at the second-declared key —
    /// the one a reader meets as the contradiction — with the first attached.
    fn validate_outline_exclusivity(&mut self, mapping: &JsonMap) -> bool {
        let outline_range = self.range(RangeKey::DocumentField("outline".into()));
        let mut conflict = false;
        for other in ["title", "sections"] {
            if !mapping.contains_key(other) {
                continue;
            }
            conflict = true;
            let other_range = self.range(RangeKey::DocumentField(other.into()));
            let outline_first = outline_range.range.start <= other_range.range.start;
            let (anchor, anchor_name, first, first_name) = if outline_first {
                (other_range, other, outline_range, "outline")
            } else {
                (outline_range, "outline", other_range, other)
            };
            self.error_with_related_at(
                SchemaErrorKind::ConflictingOutline,
                anchor,
                format!("`{anchor_name}` cannot be declared together with `{first_name}`"),
                vec![RelatedLocation {
                    range: first,
                    message: format!("`{first_name}` declared here"),
                }],
            );
        }
        conflict
    }

    fn validate_outline_shape(&mut self, value: &Value, range: SourceRange) {
        let Some(entries) = value.as_array() else {
            self.shape_error_at(range, "outline must be a sequence and cannot be null");
            return;
        };
        for (index, value) in entries.iter().enumerate() {
            let rule_range = self.range(RangeKey::OutlineRule(RuleIndex(index)));
            let Some(mapping) = value.as_object() else {
                self.shape_error_at(rule_range, "each outline rule must be a mapping");
                continue;
            };
            self.validate_known_fields(mapping, RULE_FIELDS, rule_range);
            self.validate_required_field(mapping, "match", rule_range);
            for field in ["id", "match", "repeat"] {
                if let Some(value) = mapping.get(field) {
                    if !matches!(value, Value::String(_)) {
                        self.shape_error_at(
                            self.range(RangeKey::OutlineRuleField(RuleIndex(index), field.into())),
                            format!("rule `{field}` must be a string and cannot be null"),
                        );
                    }
                }
            }
            for field in ["allow", "required", "strict", "ordered"] {
                if let Some(value) = mapping.get(field) {
                    if !matches!(value, Value::Bool(_)) {
                        self.shape_error_at(
                            self.range(RangeKey::OutlineRuleField(RuleIndex(index), field.into())),
                            format!("rule `{field}` must be a bool and cannot be null"),
                        );
                    }
                }
            }
            let child_scope = ScopePath(vec![RuleIndex(index)]);
            if let Some(children) = mapping.get("sections") {
                let range = self.range(RangeKey::OutlineRuleField(
                    RuleIndex(index),
                    "sections".into(),
                ));
                self.validate_rules_shape(children, &child_scope, range);
            }
            if let Some(constraints) = mapping.get("constraints") {
                let range = self.range(RangeKey::OutlineRuleField(
                    RuleIndex(index),
                    "constraints".into(),
                ));
                self.validate_constraints_shape(constraints, &child_scope, range);
            }
        }
    }

    fn validate_rules_shape(&mut self, value: &Value, scope: &ScopePath, range: SourceRange) {
        let Some(rules) = value.as_array() else {
            self.shape_error_at(range, "sections must be a sequence and cannot be null");
            return;
        };
        for (index, value) in rules.iter().enumerate() {
            let path = RulePath {
                scope: scope.clone(),
                index: RuleIndex(index),
            };
            let rule_range = self.range(RangeKey::Rule(path.clone()));
            let Some(mapping) = value.as_object() else {
                self.shape_error_at(rule_range, "each section rule must be a mapping");
                continue;
            };
            self.validate_known_fields(mapping, RULE_FIELDS, rule_range);
            self.validate_required_field(mapping, "match", rule_range);
            for field in ["id", "match", "repeat"] {
                if let Some(value) = mapping.get(field) {
                    if !matches!(value, Value::String(_)) {
                        self.shape_error_at(
                            self.range(RangeKey::RuleField(path.clone(), field.into())),
                            format!("rule `{field}` must be a string and cannot be null"),
                        );
                    }
                }
            }
            for field in ["allow", "required", "strict", "ordered"] {
                if let Some(value) = mapping.get(field) {
                    if !matches!(value, Value::Bool(_)) {
                        self.shape_error_at(
                            self.range(RangeKey::RuleField(path.clone(), field.into())),
                            format!("rule `{field}` must be a bool and cannot be null"),
                        );
                    }
                }
            }
            let mut child_scope = scope.clone();
            child_scope.0.push(RuleIndex(index));
            if let Some(children) = mapping.get("sections") {
                let range = self.range(RangeKey::RuleField(path.clone(), "sections".into()));
                self.validate_rules_shape(children, &child_scope, range);
            }
            if let Some(constraints) = mapping.get("constraints") {
                let range = self.range(RangeKey::RuleField(path, "constraints".into()));
                self.validate_constraints_shape(constraints, &child_scope, range);
            }
        }
    }

    fn validate_constraints_shape(&mut self, value: &Value, scope: &ScopePath, range: SourceRange) {
        let Some(constraints) = value.as_array() else {
            self.shape_error_at(range, "constraints must be a sequence and cannot be null");
            return;
        };
        for (index, constraint) in constraints.iter().enumerate() {
            let range = self.range(RangeKey::Constraint(ConstraintPath {
                scope: scope.clone(),
                index: ConstraintIndex(index),
            }));
            self.validate_constraint_shape(constraint, range);
        }
    }

    fn validate_constraint_shape(&mut self, value: &Value, range: SourceRange) {
        let Some(mapping) = value.as_object() else {
            self.shape_error_at(range, "constraint must be a single-key object");
            return;
        };
        if mapping.len() != 1 {
            self.shape_error_at(range, "constraint must contain exactly one keyword");
            return;
        }
        let Some((keyword, operand)) = mapping.iter().next() else {
            return;
        };
        match keyword.as_str() {
            "one_of" | "any_of" | "at_most_one" | "all_or_none" | "ordered" => {
                self.validate_ref_sequence(keyword, operand, true, range);
            }
            "requires" | "conflicts" => {
                let Some(implication) = operand.as_object() else {
                    self.shape_error_at(range, format!("{keyword} operand must be an object"));
                    return;
                };
                let consequence = if keyword == "requires" {
                    "then"
                } else {
                    "then_not"
                };
                let allowed = ["if", consequence];
                self.validate_known_fields(implication, &allowed, range);
                self.validate_required_field(implication, "if", range);
                self.validate_required_field(implication, consequence, range);
                if let Some(condition) = implication.get("if") {
                    self.validate_ref_scalar(condition, range);
                }
                if let Some(value) = implication.get(consequence) {
                    if value.is_array() {
                        self.validate_ref_sequence(consequence, value, false, range);
                    } else {
                        self.validate_ref_scalar(value, range);
                    }
                }
            }
            _ => self.shape_error_at(range, format!("unknown constraint keyword `{keyword}`")),
        }
    }

    fn validate_ref_sequence(
        &mut self,
        name: &str,
        value: &Value,
        require_two: bool,
        range: SourceRange,
    ) {
        let Some(values) = value.as_array() else {
            self.shape_error_at(range, format!("{name} must be a sequence of refs"));
            return;
        };
        let minimum = if require_two { 2 } else { 1 };
        if values.len() < minimum {
            let noun = if minimum == 1 { "ref" } else { "refs" };
            self.shape_error_at(range, format!("{name} requires at least {minimum} {noun}"));
        }
        for value in values {
            self.validate_ref_scalar(value, range);
        }
    }

    fn validate_ref_scalar(&mut self, value: &Value, range: SourceRange) {
        if !matches!(value, Value::String(_)) {
            self.shape_error_at(range, "constraint refs must be strings and cannot be null");
        }
    }

    fn validate_known_fields(&mut self, mapping: &JsonMap, allowed: &[&str], range: SourceRange) {
        // A JSON object's keys are strings by construction: a YAML key that
        // was not one has already been rejected by the conversion, with the
        // key's own range.
        for key in mapping.keys() {
            if !allowed.contains(&key.as_str()) {
                self.shape_error_at(range, format!("unknown field `{key}`"));
            }
        }
    }

    fn validate_required_field(&mut self, mapping: &JsonMap, field: &str, range: SourceRange) {
        if !mapping.contains_key(field) {
            self.shape_error_at(range, format!("missing required field `{field}`"));
        }
    }
}

/// Whether a value is an integer the schema's own fields can hold.
///
/// The engine preserves a number's exact spelling, so an integer of any
/// magnitude arrives here as a number rather than failing the parse; one that
/// does not fit the 64-bit fields is a shape complaint against the value, not
/// a syntax error against the document.
fn is_yaml_integer(value: &Value) -> bool {
    match value {
        Value::Number(number) => number.as_i64().is_some() || number.as_u64().is_some(),
        _ => false,
    }
}
