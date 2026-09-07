//! Range-aware loading and normalization of preamble content and list-item rules.

use std::collections::HashMap;

use serde_json::{Map, Value};

use crate::{
    AtLeastTwo, BlockMatcher, Cardinality, ContentOwner, ContentRule, ContentRuleIndex,
    ContentRulePath, ContentScope, ItemRule, ItemRuleIndex, ItemRulePath, ItemScope, ListKind,
    Matcher, RelatedLocation, RuleId, SchemaErrorKind, SchemaNode, SourceRange,
};

use super::rules::is_slug;
use super::{Loader, RangeKey, RawContentOwner};

const CONTENT_FIELDS: &[&str] = &[
    "id",
    "block",
    "one_of",
    "required",
    "repeat",
    "list_kind",
    "items",
];
const ALTERNATIVE_FIELDS: &[&str] = &["block", "list_kind"];
const ITEM_FIELDS: &[&str] = &["id", "match", "required", "repeat"];

#[derive(Clone, Copy)]
enum OuterBlock {
    Paragraph,
    List(Option<ListKind>),
    Any,
}

impl Loader {
    pub(super) fn build_content_scope(
        &mut self,
        raw: Option<&Value>,
        raw_owner: &RawContentOwner,
        owner: ContentOwner,
        match_case: bool,
    ) -> Option<ContentScope> {
        let Some(raw) = raw else {
            return Some(ContentScope::Omitted);
        };
        let Some(entries) = raw.as_array() else {
            self.content_error_at(
                self.content_field_range(raw_owner),
                "content must be a sequence and cannot be null",
            );
            return None;
        };
        let mut complete = true;
        let mut rules = Vec::with_capacity(entries.len());
        for (index, entry) in entries.iter().enumerate() {
            match self.build_content_rule(entry, raw_owner, index, match_case) {
                Some(rule) => {
                    let content_path = ContentRulePath {
                        owner: owner.clone(),
                        index: ContentRuleIndex(index),
                    };
                    let content_range = self.range(RangeKey::ContentRule(raw_owner.clone(), index));
                    self.nodes
                        .insert(SchemaNode::ContentRule(content_path.clone()), content_range);
                    if let ContentRule::List {
                        items: ItemScope::Declared(items),
                        ..
                    } = &rule
                    {
                        for (item_index, _) in items.iter().enumerate() {
                            let item_range = self.range(RangeKey::ItemRule(
                                raw_owner.clone(),
                                index,
                                item_index,
                            ));
                            self.nodes.insert(
                                SchemaNode::ItemRule(ItemRulePath {
                                    content: content_path.clone(),
                                    index: ItemRuleIndex(item_index),
                                }),
                                item_range,
                            );
                        }
                    }
                    rules.push(rule);
                }
                None => complete = false,
            }
        }
        complete.then_some(ContentScope::Declared(rules))
    }

    fn build_content_rule(
        &mut self,
        raw: &Value,
        owner: &RawContentOwner,
        index: usize,
        match_case: bool,
    ) -> Option<ContentRule> {
        let rule_range = self.range(RangeKey::ContentRule(owner.clone(), index));
        let Some(mapping) = raw.as_object() else {
            self.content_error_at(rule_range, "each content entry must be a mapping");
            return None;
        };
        let unknown_valid = self.reject_unknown_content_fields(mapping, owner, index);

        let block = mapping.get("block");
        let one_of = mapping.get("one_of");
        let outer = match (block, one_of) {
            (None, None) => {
                self.content_error_at(
                    rule_range,
                    "content rule must declare exactly one of `block` and `one_of`",
                );
                return None;
            }
            (Some(_), Some(_)) => {
                let block_range = self.content_key_range(owner, index, "block");
                let choice_range = self.content_key_range(owner, index, "one_of");
                self.content_error_at(
                    if block_range.range.start <= choice_range.range.start {
                        block_range
                    } else {
                        choice_range
                    },
                    "content rule cannot declare both `block` and `one_of`",
                );
                return None;
            }
            (Some(value), None) => match value.as_str() {
                Some("p") => OuterForm::Block(OuterBlock::Paragraph),
                Some("list") => OuterForm::Block(OuterBlock::List(None)),
                Some("any") => OuterForm::Block(OuterBlock::Any),
                _ => {
                    self.content_error_at(
                        self.content_key_range(owner, index, "block"),
                        "`block` must be one of `p`, `list`, or `any`",
                    );
                    return None;
                }
            },
            (None, Some(value)) => {
                if !value.is_array() {
                    self.content_error_at(
                        self.content_key_range(owner, index, "one_of"),
                        "`one_of` must be a sequence",
                    );
                    return None;
                }
                OuterForm::Choice(value)
            }
        };

        let mut valid = unknown_valid;
        match outer {
            OuterForm::Block(OuterBlock::List(_)) => {}
            OuterForm::Block(_) | OuterForm::Choice(_) => {
                for field in ["list_kind", "items"] {
                    if mapping.contains_key(field) {
                        valid = false;
                        self.content_error_at(
                            self.content_key_range(owner, index, field),
                            format!("`{field}` is only applicable to `block: list`"),
                        );
                    }
                }
            }
        }

        let id = self.load_explicit_id(mapping, owner, index, &mut valid);
        let cardinality = self.load_content_cardinality(mapping, owner, index, &mut valid);

        match outer {
            OuterForm::Block(mut block) => {
                if matches!(block, OuterBlock::List(_)) {
                    let list_kind = self.load_list_kind(mapping, owner, index, &mut valid);
                    block = OuterBlock::List(list_kind);
                }
                let items = if matches!(block, OuterBlock::List(_)) {
                    self.build_item_scope(
                        mapping.get("items"),
                        owner,
                        index,
                        match_case,
                        &mut valid,
                    )
                } else {
                    ItemScope::Omitted
                };
                let cardinality = cardinality?;
                valid.then_some(match block {
                    OuterBlock::Paragraph => ContentRule::Paragraph { id, cardinality },
                    OuterBlock::List(list_kind) => ContentRule::List {
                        id,
                        cardinality,
                        list_kind,
                        items,
                    },
                    OuterBlock::Any => ContentRule::Any { id, cardinality },
                })
            }
            OuterForm::Choice(value) => {
                let alternatives = self.load_one_of(value, owner, index, &mut valid);
                let cardinality = cardinality?;
                match (valid, alternatives) {
                    (true, Some(alternatives)) => Some(ContentRule::OneOf {
                        id,
                        cardinality,
                        alternatives,
                    }),
                    _ => None,
                }
            }
        }
    }

    fn reject_unknown_content_fields(
        &mut self,
        mapping: &Map<String, Value>,
        owner: &RawContentOwner,
        index: usize,
    ) -> bool {
        let mut valid = true;
        for field in mapping.keys() {
            if !CONTENT_FIELDS.contains(&field.as_str()) {
                valid = false;
                self.shape_error_at(
                    self.content_key_range(owner, index, field),
                    format!("unknown field `{field}`"),
                );
            }
        }
        valid
    }

    fn load_explicit_id(
        &mut self,
        mapping: &Map<String, Value>,
        owner: &RawContentOwner,
        index: usize,
        valid: &mut bool,
    ) -> Option<RuleId> {
        let value = mapping.get("id")?;
        let range = self.content_value_range(owner, index, "id");
        let Some(id) = value.as_str() else {
            *valid = false;
            self.shape_error_at(
                range,
                "content rule `id` must be a string and cannot be null",
            );
            return None;
        };
        if !is_slug(id) {
            *valid = false;
            self.shape_error_at(range, format!("rule id `{id}` is not a lowercase slug"));
            return None;
        }
        if matches!(owner, RawContentOwner::Document) {
            if let Some(purpose) = super::rules::reserved_root_id(id) {
                *valid = false;
                self.error_at(
                    SchemaErrorKind::ReservedId,
                    range,
                    format!("top-level rule id `{id}` is reserved for {purpose}"),
                );
            }
        }
        Some(RuleId(id.to_owned()))
    }

    fn load_content_cardinality(
        &mut self,
        mapping: &Map<String, Value>,
        owner: &RawContentOwner,
        index: usize,
        valid: &mut bool,
    ) -> Option<Cardinality> {
        let (required, repeat, shape_valid, range) =
            self.raw_cardinality(mapping, owner, index, valid);
        if !shape_valid {
            return None;
        }
        let cardinality = self.build_cardinality(required, repeat, true, range);
        *valid &= cardinality.is_some();
        cardinality
    }

    fn raw_cardinality<'a>(
        &mut self,
        mapping: &'a Map<String, Value>,
        owner: &RawContentOwner,
        index: usize,
        valid: &mut bool,
    ) -> (Option<bool>, Option<&'a str>, bool, SourceRange) {
        let mut shape_valid = true;
        let required = match mapping.get("required") {
            Some(Value::Bool(value)) => Some(*value),
            Some(_) => {
                *valid = false;
                shape_valid = false;
                self.shape_error_at(
                    self.content_value_range(owner, index, "required"),
                    "rule `required` must be a bool and cannot be null",
                );
                None
            }
            None => None,
        };
        let repeat = match mapping.get("repeat") {
            Some(Value::String(value)) => Some(value.as_str()),
            Some(_) => {
                *valid = false;
                shape_valid = false;
                self.shape_error_at(
                    self.content_value_range(owner, index, "repeat"),
                    "rule `repeat` must be a string and cannot be null",
                );
                None
            }
            None => None,
        };
        let range = if repeat.is_some() {
            self.content_value_range(owner, index, "repeat")
        } else if required.is_some() {
            self.content_value_range(owner, index, "required")
        } else {
            self.range(RangeKey::ContentRule(owner.clone(), index))
        };
        (required, repeat, shape_valid, range)
    }

    fn load_list_kind(
        &mut self,
        mapping: &Map<String, Value>,
        owner: &RawContentOwner,
        index: usize,
        valid: &mut bool,
    ) -> Option<ListKind> {
        let value = mapping.get("list_kind")?;
        match value.as_str() {
            Some("bullet") => Some(ListKind::Bullet),
            Some("ordered") => Some(ListKind::Ordered),
            Some("any") => None,
            _ => {
                *valid = false;
                self.content_error_at(
                    self.content_value_range(owner, index, "list_kind"),
                    "`list_kind` must be one of `bullet`, `ordered`, or `any`",
                );
                None
            }
        }
    }

    fn load_one_of(
        &mut self,
        value: &Value,
        owner: &RawContentOwner,
        rule_index: usize,
        valid: &mut bool,
    ) -> Option<AtLeastTwo<BlockMatcher>> {
        let alternatives = value.as_array()?;
        if alternatives.len() < 2 {
            *valid = false;
            self.content_error_at(
                self.content_value_range(owner, rule_index, "one_of"),
                "`one_of` requires at least two alternatives",
            );
            return None;
        }
        let mut normalized = Vec::with_capacity(alternatives.len());
        let mut first_ranges: HashMap<BlockMatcher, SourceRange> = HashMap::new();
        for (alternative_index, alternative) in alternatives.iter().enumerate() {
            let alternative_range = self.range(RangeKey::ContentAlternative(
                owner.clone(),
                rule_index,
                alternative_index,
            ));
            let Some(mapping) = alternative.as_object() else {
                *valid = false;
                self.content_error_at(
                    alternative_range,
                    "each `one_of` alternative must be a mapping",
                );
                continue;
            };
            let Some(block_value) = mapping.get("block") else {
                *valid = false;
                self.content_error_at(
                    alternative_range,
                    "each `one_of` alternative must declare exactly one `block`",
                );
                for field in mapping.keys().filter(|field| {
                    !CONTENT_FIELDS.contains(&field.as_str())
                        && !ALTERNATIVE_FIELDS.contains(&field.as_str())
                }) {
                    self.shape_error_at(
                        self.alternative_key_range(owner, rule_index, alternative_index, field),
                        format!("unknown field `{field}`"),
                    );
                }
                continue;
            };
            let block_label = match block_value.as_str() {
                Some("p") => Some("p"),
                Some("list") => Some("list"),
                Some("any") => Some("any"),
                _ => {
                    *valid = false;
                    self.content_error_at(
                        self.alternative_key_range(owner, rule_index, alternative_index, "block"),
                        "alternative `block` must be one of `p`, `list`, or `any`",
                    );
                    None
                }
            };
            let Some(block_label) = block_label else {
                for field in mapping.keys().filter(|field| {
                    !CONTENT_FIELDS.contains(&field.as_str())
                        && !ALTERNATIVE_FIELDS.contains(&field.as_str())
                }) {
                    self.shape_error_at(
                        self.alternative_key_range(owner, rule_index, alternative_index, field),
                        format!("unknown field `{field}`"),
                    );
                }
                continue;
            };
            let errors_before_members = self.errors.len();
            for field in mapping
                .keys()
                .filter(|field| !ALTERNATIVE_FIELDS.contains(&field.as_str()))
            {
                *valid = false;
                self.reject_alternative_extra(owner, rule_index, alternative_index, field);
            }
            let block = match block_label {
                "p" => {
                    self.reject_inapplicable_alternative_list_kind(
                        mapping,
                        owner,
                        rule_index,
                        alternative_index,
                        valid,
                    );
                    Some(BlockMatcher::Paragraph)
                }
                "list" => self
                    .load_alternative_list_kind(
                        mapping,
                        owner,
                        rule_index,
                        alternative_index,
                        valid,
                    )
                    .map(|list_kind| BlockMatcher::List { list_kind }),
                "any" => {
                    self.reject_inapplicable_alternative_list_kind(
                        mapping,
                        owner,
                        rule_index,
                        alternative_index,
                        valid,
                    );
                    Some(BlockMatcher::Any)
                }
                _ => None,
            };
            if self.errors.len() == errors_before_members {
                let Some(block) = block else {
                    continue;
                };
                if let Some(first) = first_ranges.get(&block) {
                    *valid = false;
                    self.error_with_related_at(
                        SchemaErrorKind::InvalidContentRule,
                        alternative_range,
                        "duplicate normalized `one_of` alternative",
                        vec![RelatedLocation {
                            range: *first,
                            message: "first equivalent alternative declared here".into(),
                        }],
                    );
                } else {
                    first_ranges.insert(block.clone(), alternative_range);
                    normalized.push(block);
                }
            }
        }
        if !*valid || normalized.len() < 2 {
            return None;
        }
        let mut values = normalized.into_iter();
        let first = values.next()?;
        let second = values.next()?;
        Some(AtLeastTwo {
            first,
            second,
            rest: values.collect(),
        })
    }

    fn reject_alternative_extra(
        &mut self,
        owner: &RawContentOwner,
        rule_index: usize,
        alternative_index: usize,
        field: &str,
    ) {
        let range = self.alternative_key_range(owner, rule_index, alternative_index, field);
        if CONTENT_FIELDS.contains(&field) {
            self.content_error_at(
                range,
                format!("`{field}` is not applicable in an alternative"),
            );
        } else {
            self.shape_error_at(range, format!("unknown field `{field}`"));
        }
    }

    fn reject_inapplicable_alternative_list_kind(
        &mut self,
        mapping: &Map<String, Value>,
        owner: &RawContentOwner,
        rule_index: usize,
        alternative_index: usize,
        valid: &mut bool,
    ) {
        if mapping.contains_key("list_kind") {
            *valid = false;
            self.content_error_at(
                self.alternative_key_range(owner, rule_index, alternative_index, "list_kind"),
                "alternative `list_kind` is only applicable to `block: list`",
            );
        }
    }

    fn load_alternative_list_kind(
        &mut self,
        mapping: &Map<String, Value>,
        owner: &RawContentOwner,
        rule_index: usize,
        alternative_index: usize,
        valid: &mut bool,
    ) -> Option<Option<ListKind>> {
        let Some(value) = mapping.get("list_kind") else {
            return Some(None);
        };
        match value.as_str() {
            Some("bullet") => Some(Some(ListKind::Bullet)),
            Some("ordered") => Some(Some(ListKind::Ordered)),
            Some("any") => Some(None),
            _ => {
                *valid = false;
                self.content_error_at(
                    self.alternative_value_range(owner, rule_index, alternative_index, "list_kind"),
                    "alternative `list_kind` must be one of `bullet`, `ordered`, or `any`",
                );
                None
            }
        }
    }

    fn build_item_scope(
        &mut self,
        raw: Option<&Value>,
        owner: &RawContentOwner,
        content_index: usize,
        match_case: bool,
        valid: &mut bool,
    ) -> ItemScope {
        let Some(raw) = raw else {
            return ItemScope::Omitted;
        };
        let Some(entries) = raw.as_array() else {
            *valid = false;
            self.content_error_at(
                self.content_value_range(owner, content_index, "items"),
                "`items` must be a sequence and cannot be null",
            );
            return ItemScope::Omitted;
        };
        let mut rules = Vec::with_capacity(entries.len());
        for (item_index, entry) in entries.iter().enumerate() {
            match self.build_item_rule(entry, owner, content_index, item_index, match_case) {
                Some(rule) => rules.push(rule),
                None => *valid = false,
            }
        }
        ItemScope::Declared(rules)
    }

    fn build_item_rule(
        &mut self,
        raw: &Value,
        owner: &RawContentOwner,
        content_index: usize,
        item_index: usize,
        match_case: bool,
    ) -> Option<ItemRule> {
        let rule_range = self.range(RangeKey::ItemRule(owner.clone(), content_index, item_index));
        let Some(mapping) = raw.as_object() else {
            self.content_error_at(rule_range, "each item rule must be a mapping");
            return None;
        };
        let mut valid = true;
        for field in mapping.keys() {
            if !ITEM_FIELDS.contains(&field.as_str()) {
                valid = false;
                self.shape_error_at(
                    self.item_key_range(owner, content_index, item_index, field),
                    format!("unknown field `{field}`"),
                );
            }
        }
        let id = match mapping.get("id") {
            Some(Value::String(id)) if is_slug(id) => Some(RuleId(id.clone())),
            Some(Value::String(id)) => {
                valid = false;
                self.shape_error_at(
                    self.item_value_range(owner, content_index, item_index, "id"),
                    format!("rule id `{id}` is not a lowercase slug"),
                );
                None
            }
            Some(_) => {
                valid = false;
                self.shape_error_at(
                    self.item_value_range(owner, content_index, item_index, "id"),
                    "item rule `id` must be a string and cannot be null",
                );
                None
            }
            None => None,
        };
        let matcher_value = match mapping.get("match") {
            Some(Value::String(value)) => Some(value.as_str()),
            Some(_) => {
                valid = false;
                self.shape_error_at(
                    self.item_value_range(owner, content_index, item_index, "match"),
                    "item rule `match` must be a string and cannot be null",
                );
                None
            }
            None => {
                valid = false;
                self.shape_error_at(rule_range, "item rule is missing required field `match`");
                None
            }
        };
        let matcher_range = self.item_value_range(owner, content_index, item_index, "match");
        let matcher =
            matcher_value.and_then(|source| self.build_matcher(source, match_case, matcher_range));
        let (required, repeat, cardinality_shape_valid) =
            self.raw_item_cardinality(mapping, owner, content_index, item_index, &mut valid);
        let cardinality_range = if repeat.is_some() {
            self.item_value_range(owner, content_index, item_index, "repeat")
        } else if required.is_some() {
            self.item_value_range(owner, content_index, item_index, "required")
        } else {
            matcher_range
        };
        let cardinality = if cardinality_shape_valid
            && (matcher.is_some() || required.is_some() || repeat.is_some())
        {
            self.build_cardinality(
                required,
                repeat,
                matcher
                    .as_ref()
                    .is_some_and(|matcher| matches!(matcher, Matcher::Exact(_))),
                cardinality_range,
            )
        } else {
            None
        };
        valid &= matcher.is_some() && cardinality.is_some() && cardinality_shape_valid;
        match (valid, matcher, cardinality) {
            (true, Some(matcher), Some(cardinality)) => Some(ItemRule {
                id,
                matcher,
                cardinality,
            }),
            _ => None,
        }
    }

    fn raw_item_cardinality<'a>(
        &mut self,
        mapping: &'a Map<String, Value>,
        owner: &RawContentOwner,
        content_index: usize,
        item_index: usize,
        valid: &mut bool,
    ) -> (Option<bool>, Option<&'a str>, bool) {
        let mut shape_valid = true;
        let required = match mapping.get("required") {
            Some(Value::Bool(value)) => Some(*value),
            Some(_) => {
                *valid = false;
                shape_valid = false;
                self.shape_error_at(
                    self.item_value_range(owner, content_index, item_index, "required"),
                    "item rule `required` must be a bool and cannot be null",
                );
                None
            }
            None => None,
        };
        let repeat = match mapping.get("repeat") {
            Some(Value::String(value)) => Some(value.as_str()),
            Some(_) => {
                *valid = false;
                shape_valid = false;
                self.shape_error_at(
                    self.item_value_range(owner, content_index, item_index, "repeat"),
                    "item rule `repeat` must be a string and cannot be null",
                );
                None
            }
            None => None,
        };
        (required, repeat, shape_valid)
    }

    fn content_field_range(&self, owner: &RawContentOwner) -> SourceRange {
        match owner {
            RawContentOwner::Document => self.range(RangeKey::DocumentField("content".into())),
            RawContentOwner::Rule(path) => {
                self.range(RangeKey::RuleField(path.clone(), "content".into()))
            }
        }
    }

    fn content_key_range(&self, owner: &RawContentOwner, index: usize, field: &str) -> SourceRange {
        self.range(RangeKey::ContentRuleFieldKey(
            owner.clone(),
            index,
            field.into(),
        ))
    }

    fn content_value_range(
        &self,
        owner: &RawContentOwner,
        index: usize,
        field: &str,
    ) -> SourceRange {
        self.range(RangeKey::ContentRuleFieldValue(
            owner.clone(),
            index,
            field.into(),
        ))
    }

    fn alternative_key_range(
        &self,
        owner: &RawContentOwner,
        rule: usize,
        alternative: usize,
        field: &str,
    ) -> SourceRange {
        self.range(RangeKey::ContentAlternativeFieldKey(
            owner.clone(),
            rule,
            alternative,
            field.into(),
        ))
    }

    fn alternative_value_range(
        &self,
        owner: &RawContentOwner,
        rule: usize,
        alternative: usize,
        field: &str,
    ) -> SourceRange {
        self.range(RangeKey::ContentAlternativeFieldValue(
            owner.clone(),
            rule,
            alternative,
            field.into(),
        ))
    }

    fn item_key_range(
        &self,
        owner: &RawContentOwner,
        rule: usize,
        item: usize,
        field: &str,
    ) -> SourceRange {
        self.range(RangeKey::ItemRuleFieldKey(
            owner.clone(),
            rule,
            item,
            field.into(),
        ))
    }

    fn item_value_range(
        &self,
        owner: &RawContentOwner,
        rule: usize,
        item: usize,
        field: &str,
    ) -> SourceRange {
        self.range(RangeKey::ItemRuleFieldValue(
            owner.clone(),
            rule,
            item,
            field.into(),
        ))
    }

    fn content_error_at(&mut self, range: SourceRange, message: impl Into<String>) {
        self.error_at(SchemaErrorKind::InvalidContentRule, range, message);
    }
}

enum OuterForm<'a> {
    Block(OuterBlock),
    Choice(&'a Value),
}
