//! Loading, semantic validation, and normalization of Outlint schemas.
//!
//! `InvalidSchema` intentionally owns all source and error data, so boxing it
//! merely to reduce the result enum would complicate the public loader API.

#![allow(clippy::result_large_err)]

mod constraints;
mod frontmatter_schema;
mod rules;
mod shape;
mod yaml;

#[cfg(test)]
mod tests;

use std::{collections::BTreeMap, sync::Arc};

use serde::Deserialize;
use serde_json::Value;

use crate::{
    ByteOffset, ConstraintIndex, ConstraintPath, DocumentShape, GuardPath, InvalidSchema,
    JsonSchemaResourceContents, LinkedJsonSchemaInput, LoadSchemaResult, LoadedSchema, Matcher,
    NonEmpty, OrderIndex, OutlineProvenance, RelatedLocation, RuleIndex, RulePath, Schema,
    SchemaError, SchemaErrorKind, SchemaLocations, SchemaNode, SchemaSource, SchemaSources,
    SchemaVersion, ScopePath, SourceId, SourceLabel, SourceRange, TextRange, TitleSlot,
};

use self::constraints::constraints_mut;
use self::frontmatter_schema::{
    external_source_id, prepare_external_schema, single_external_error, PreparedExternalError,
    PreparedExternalSchema,
};
use self::yaml::{
    char_range, classify_duplicate_keys, parse_schema_yaml, schema_yaml_to_json, RangeIndex,
    SchemaYamlError, SchemaYamlNode,
};

pub use self::frontmatter_schema::{
    json_schema_external_references, linked_frontmatter_schema_path,
};
pub(crate) use self::frontmatter_schema::{
    json_schema_reference_budget_message, json_schema_reference_count,
    preloaded_json_schema_registry, NoExternalRetrieve, MAX_JSON_SCHEMA_REFERENCES,
};

/// The object domain schema documents are validated in: JSON Schema's own.
type JsonMap = serde_json::Map<String, Value>;

/// Loads an Outlint schema from UTF-8 source text.
///
/// The returned model contains only normalized values. Errors are accumulated
/// where later checks do not depend on an earlier invalid value.
///
/// # Example
///
/// ```
/// use outlint_core::{load_schema, Matcher};
///
/// let loaded = load_schema(
///     r#"
/// version: 2
/// title: null
/// sections:
///   - match: Overview
/// "#,
/// )?;
///
/// assert!(matches!(loaded.schema.document, outlint_core::DocumentShape::Title(_)));
/// # Ok::<(), outlint_core::InvalidSchema>(())
/// ```
pub fn load_schema(source: &str) -> LoadSchemaResult {
    load_schema_with_label(source, None)
}

/// Loads an Outlint schema from source text with a diagnostic display label.
pub fn load_schema_with_label(source: &str, label: Option<SourceLabel>) -> LoadSchemaResult {
    Loader::new(Arc::from(source), label, None).load()
}

/// Loads an Outlint schema with an already-preloaded linked JSON Schema graph.
///
/// This is the complete pure loader boundary for filesystem-backed schemas:
/// callers perform all reads first and provide stable logical resource URIs.
pub fn load_schema_with_resources(
    source: &str,
    label: Option<SourceLabel>,
    external: Option<LinkedJsonSchemaInput>,
) -> LoadSchemaResult {
    Loader::new(Arc::from(source), label, external).load()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSchema {
    #[serde(rename = "version")]
    _version: Value,
    title: Option<String>,
    #[serde(default)]
    options: RawOptions,
    #[serde(default)]
    frontmatter: Option<RawFrontmatter>,
    /// Absent only when `outline` is declared; the shape validation enforces
    /// exactly one of the two before this structure is built.
    sections: Option<Vec<RawRule>>,
    outline: Option<Vec<RawRule>>,
    #[serde(default)]
    forbid_sections: Vec<RawGuard>,
    extras: Option<String>,
    unordered: Option<bool>,
    #[serde(default)]
    constraints: Vec<Value>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFrontmatter {
    required: Option<bool>,
    allow: Option<bool>,
    schema: Option<RawFrontmatterSchema>,
    /// The `captures` declaration exactly as written, unnormalized.
    ///
    /// Held as an opaque value because `deny_unknown_fields` decides only
    /// that the key is known; the declaration's own shape, names, paths, and
    /// types are §2.3 questions, and `frontmatter_schema` answers them
    /// against the YAML tree, where every one of them has a source range to
    /// anchor an `invalid-capture` at. A typed field here would answer them
    /// through serde instead, and lose the range.
    captures: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawFrontmatterSchema {
    Path(String),
    Mapping(JsonMap),
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawOptions {
    match_case: Option<bool>,
    strip_inline_markup: Option<bool>,
    allow_skipped_levels: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRule {
    id: Option<String>,
    #[serde(rename = "match")]
    matcher: String,
    required: Option<bool>,
    repeat: Option<String>,
    sections: Option<Vec<RawRule>>,
    #[serde(default)]
    forbid_sections: Vec<RawGuard>,
    extras: Option<String>,
    unordered: Option<bool>,
    #[serde(default)]
    constraints: Vec<Value>,
    /// The rule's `captures` mapping exactly as written, unnormalized.
    ///
    /// See [`RawFrontmatter::captures`]: held raw so `rules` can normalize it
    /// against the YAML tree and keep its ranges.
    captures: Option<Value>,
    /// The rule's `order` list exactly as written, unnormalized.
    ///
    /// See [`RawFrontmatter::captures`]: held raw for the same reason.
    order: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawGuard {
    #[serde(rename = "match")]
    matcher: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum RangeKey {
    DocumentField(String),
    OptionField(String),
    FrontmatterField(String),
    Rule(RulePath),
    RuleField(RulePath, String),
    Guard(GuardPath),
    GuardField(GuardPath, String),
    Constraint(ConstraintPath),
    /// An `h1`-level rule in the top-level `outline` list.
    OutlineRule(RuleIndex),
    /// One field of an `h1`-level rule in the top-level `outline` list.
    OutlineRuleField(RuleIndex, String),
    /// One entry of a rule's `captures` mapping, keyed by the raw spelling.
    ///
    /// The key is the source text, not a validated [`CaptureName`]: a range
    /// has to exist for the declaration whose name is about to be *rejected*,
    /// which is precisely the declaration that has no validated name.
    ///
    /// [`CaptureName`]: crate::CaptureName
    RuleCapture(RulePath, String),
    /// One entry of an `h1`-level `outline` rule's `captures` mapping.
    OutlineRuleCapture(RuleIndex, String),
    /// One entry of `frontmatter.captures`, keyed by the raw spelling.
    FrontmatterCapture(String),
    /// One entry of a rule's `order` list, by its zero-based position.
    RuleOrderEntry(RulePath, OrderIndex),
    /// One entry of an `h1`-level `outline` rule's `order` list.
    OutlineRuleOrderEntry(RuleIndex, OrderIndex),
}

struct Loader {
    sources: SchemaSources,
    document_range: SourceRange,
    /// Character-index → byte-offset bridge for the primary source, shared by
    /// the range index and every positioned refusal the parse produced.
    char_offsets: Vec<usize>,
    /// The one tree parsed over the source, or the refusal that stopped it.
    /// Consumed by [`Loader::load`]; the range index was read off it first.
    parsed: Option<Result<SchemaYamlNode, SchemaYamlError>>,
    ranges: RangeIndex,
    errors: Vec<SchemaError>,
    nodes: BTreeMap<SchemaNode, SourceRange>,
    raw_constraints: BTreeMap<ScopePath, Vec<Value>>,
    external_schema: Option<PreparedExternalSchema>,
    /// Whether the document declares the general `outline:` form.
    ///
    /// The outline's rules are built at the empty scope they semantically
    /// live in, while their spellings were collected under the dedicated
    /// outline range keys; this flag makes [`Loader::source_key`] bridge the
    /// two. Sugar schemas need no bridge: their `sections` forest is both
    /// spelled and built at the empty scope.
    outline_general: bool,
}

impl Loader {
    fn new(
        source: Arc<str>,
        label: Option<SourceLabel>,
        external_schema: Option<LinkedJsonSchemaInput>,
    ) -> Self {
        let document_range = SourceRange {
            source: SourceId(0),
            range: TextRange {
                start: ByteOffset(0),
                end: ByteOffset(source.len()),
            },
        };
        let char_offsets = source
            .char_indices()
            .map(|(offset, _)| offset)
            .chain(std::iter::once(source.len()))
            .collect::<Vec<_>>();
        // One parse serves everything: the reader bounds nesting and alias
        // expansion itself, so the tree it yields is safe for every recursive
        // walk that follows — the range index here, the conversion in `load`.
        let parsed = parse_schema_yaml(&source);
        let ranges = match &parsed {
            Ok(tree) => RangeIndex::from_tree(tree, &char_offsets),
            Err(_) => RangeIndex::default(),
        };
        let mut sources = primary_sources(Arc::clone(&source), label);
        let external_schema = external_schema.map(|external| {
            let mut source_ids = BTreeMap::new();
            let mut source_id_exhausted = false;
            for (index, resource) in external.resources.iter().enumerate() {
                let Some(id) = external_source_id(index) else {
                    source_id_exhausted = true;
                    break;
                };
                source_ids.entry(resource.uri.clone()).or_insert(id);
                sources.documents.insert(
                    id,
                    SchemaSource {
                        label: resource.label.clone(),
                        text: match &resource.contents {
                            JsonSchemaResourceContents::Loaded(text) => Arc::clone(text),
                            JsonSchemaResourceContents::ReadFailure(_) => Arc::from(""),
                        },
                    },
                );
            }
            if source_id_exhausted {
                PreparedExternalSchema {
                    root_source: SourceId(0),
                    result: Err(single_external_error(PreparedExternalError {
                        source: SourceId(0),
                        message: "too many linked JSON Schema resources to assign source ids"
                            .into(),
                    })),
                }
            } else {
                prepare_external_schema(&external, &source_ids)
            }
        });
        Self {
            sources,
            document_range,
            char_offsets,
            parsed: Some(parsed),
            ranges,
            errors: Vec::new(),
            nodes: BTreeMap::new(),
            raw_constraints: BTreeMap::new(),
            external_schema,
            outline_general: false,
        }
    }

    fn load(mut self) -> LoadSchemaResult {
        let parsed = self
            .parsed
            .take()
            .expect("the tree is parsed once and consumed once");
        let tree = match parsed {
            Ok(tree) => tree,
            Err(error) => {
                self.push_yaml_error(error);
                return self.failure();
            }
        };
        // Repeated keys are decided before anything is converted: the JSON
        // object the conversion builds cannot hold two entries for one key,
        // so a duplicate that reaches it is one the tree still knew about and
        // the object no longer does. Classification needs the ordered tree,
        // and every duplicate it finds is independent of every other, so the
        // whole set is reported at once.
        let duplicates = classify_duplicate_keys(&tree);
        if !duplicates.is_empty() {
            for duplicate in duplicates {
                self.push_yaml_error(duplicate);
            }
            return self.failure();
        }
        let value = match schema_yaml_to_json(tree) {
            Ok(value) => value,
            Err(error) => {
                self.push_yaml_error(error);
                return self.failure();
            }
        };

        let version = self.classify_version(&value);
        self.validate_document_shape(&value);
        if !self.errors.is_empty() {
            return self.failure();
        }

        // The shapes are validated against the tree's ranges first because
        // serde's data-model errors carry no positions at all.
        let frontmatter_declared = value
            .as_object()
            .is_some_and(|mapping| mapping.contains_key("frontmatter"));
        // Serde folds `title: null` and an absent `title` into the same
        // `None`, but they declare different things: null is an explicit "no
        // h1", absence is the bare-sections sugar.
        let title_null = value
            .as_object()
            .is_some_and(|mapping| matches!(mapping.get("title"), Some(Value::Null)));
        let raw: RawSchema = match serde_json::from_value(value) {
            Ok(raw) => raw,
            Err(error) => {
                self.error_at(
                    SchemaErrorKind::InvalidDocumentShape,
                    self.document_range,
                    format!("invalid schema document shape: {error}"),
                );
                return self.failure();
            }
        };

        let frontmatter = self.build_frontmatter(raw.frontmatter, frontmatter_declared);

        let options = Self::build_options(&raw.options);
        let match_case = options.match_case;
        let root_scope = ScopePath(Vec::new());
        let document = if let Some(entries) = raw.outline {
            self.outline_general = true;
            self.build_outline_scope(
                entries,
                &root_scope,
                match_case,
                raw.forbid_sections,
                raw.extras,
                raw.unordered,
                raw.constraints,
            )
            .map(DocumentShape::Outline)
        } else {
            let title = raw.title.as_deref().and_then(|matcher| {
                let range = self.range(RangeKey::DocumentField("title".into()));
                self.nodes.insert(SchemaNode::Title, range);
                self.build_matcher(matcher, match_case, range)
            });
            if title_null {
                let range = self.range(RangeKey::DocumentField("title".into()));
                self.nodes.insert(SchemaNode::Title, range);
            }
            if !title_null && raw.title.is_none() {
                // Bare `sections:` implies `title: "*"`, but there is no
                // `title:` key to anchor title diagnostics on. The `sections`
                // key is the spelling that implied the rule, so it carries
                // the anchor.
                let range = self.range(RangeKey::DocumentField("sections".into()));
                self.nodes.insert(SchemaNode::Title, range);
            }
            let children = self.build_child_scope(
                raw.sections,
                raw.forbid_sections,
                raw.extras,
                raw.unordered,
                raw.constraints,
                &root_scope,
                match_case,
                None,
            );
            children.map(|children| {
                DocumentShape::Title(if title_null {
                    TitleSlot::Forbidden { children }
                } else {
                    TitleSlot::Required {
                        matcher: title.unwrap_or(Matcher::Any),
                        spelled: if raw.title.is_some() {
                            OutlineProvenance::Spelled
                        } else {
                            OutlineProvenance::ImpliedBySections
                        },
                        children,
                    }
                })
            })
        };

        let (Some(version), Some(frontmatter), Some(document)) = (version, frontmatter, document)
        else {
            self.validate_constraint_lexical_refs();
            return self.failure();
        };
        let mut schema = Schema {
            version,
            options,
            frontmatter,
            document,
        };

        let mut normalized = BTreeMap::new();
        for (scope, constraints) in std::mem::take(&mut self.raw_constraints) {
            let mut built = Vec::with_capacity(constraints.len());
            for (index, constraint) in constraints.into_iter().enumerate() {
                let range = self.range(RangeKey::Constraint(ConstraintPath {
                    scope: scope.clone(),
                    index: ConstraintIndex(index),
                }));
                self.nodes.insert(
                    SchemaNode::Constraint(ConstraintPath {
                        scope: scope.clone(),
                        index: ConstraintIndex(index),
                    }),
                    range,
                );
                if let Some(constraint) = self.build_constraint(&schema, &scope, constraint, range)
                {
                    built.push(constraint);
                }
            }
            normalized.insert(scope, built);
        }

        if !self.errors.is_empty() {
            return self.failure();
        }
        for (scope, constraints) in normalized {
            if let Some(target) = constraints_mut(&mut schema, &scope) {
                *target = constraints;
            }
        }

        Ok(LoadedSchema {
            schema,
            sources: self.sources,
            locations: SchemaLocations {
                document: self.document_range,
                nodes: self.nodes,
            },
        })
    }

    /// Classifies a present integer version independently of other shape
    /// errors, preserving serde_json's arbitrary-precision spelling.
    fn classify_version(&mut self, value: &Value) -> Option<SchemaVersion> {
        let mapping = value.as_object()?;
        let raw = mapping.get("version")?;
        if !self::shape::is_yaml_integer(raw) {
            return None;
        }
        if raw
            .as_number()
            .is_some_and(|number| number.to_string() == "2")
        {
            Some(SchemaVersion::V2)
        } else {
            self.error_at(
                SchemaErrorKind::UnsupportedVersion,
                self.range(RangeKey::DocumentField("version".into())),
                format!("unsupported schema version {raw}; expected 2"),
            );
            None
        }
    }

    /// Reports a refusal from the YAML engine against the source's bytes.
    fn push_yaml_error(&mut self, error: SchemaYamlError) {
        let range = match error.span {
            Some((start, end)) => char_range(start, end, &self.char_offsets),
            None => self.document_range,
        };
        self.error_at(error.kind, range, error.message);
    }

    fn range(&self, key: RangeKey) -> SourceRange {
        self.ranges.get(&self.source_key(key), self.document_range)
    }

    /// Maps a semantic range key to the key its spelling was collected under.
    ///
    /// The two differ only for the general form's top-level rules: they are
    /// built at the empty scope but their spellings were collected under the
    /// dedicated outline keys. Every deeper scope, and every sugar schema,
    /// was collected exactly where it is built.
    fn source_key(&self, key: RangeKey) -> RangeKey {
        if !self.outline_general {
            return key;
        }
        match key {
            RangeKey::Rule(path) if path.scope.0.is_empty() => RangeKey::OutlineRule(path.index),
            RangeKey::RuleField(path, field) if path.scope.0.is_empty() => {
                RangeKey::OutlineRuleField(path.index, field)
            }
            RangeKey::RuleCapture(path, name) if path.scope.0.is_empty() => {
                RangeKey::OutlineRuleCapture(path.index, name)
            }
            RangeKey::RuleOrderEntry(path, order_index) if path.scope.0.is_empty() => {
                RangeKey::OutlineRuleOrderEntry(path.index, order_index)
            }
            other => other,
        }
    }

    /// The anchor of a rule's identity: its `id` spelling, else its `match`,
    /// else the rule itself.
    fn rule_id_range(&self, path: &RulePath) -> SourceRange {
        for field in ["id", "match"] {
            let key = self.source_key(RangeKey::RuleField(path.clone(), field.into()));
            if let Some(range) = self.ranges.ranges.get(&key) {
                return *range;
            }
        }
        self.range(RangeKey::Rule(path.clone()))
    }

    fn shape_error_at(&mut self, range: SourceRange, message: impl Into<String>) {
        self.error_at(SchemaErrorKind::InvalidDocumentShape, range, message);
    }

    fn error_at(&mut self, kind: SchemaErrorKind, range: SourceRange, message: impl Into<String>) {
        self.errors.push(SchemaError {
            kind,
            range,
            related: Vec::new(),
            message: message.into(),
        });
    }

    fn error_with_related_at(
        &mut self,
        kind: SchemaErrorKind,
        range: SourceRange,
        message: impl Into<String>,
        related: Vec<RelatedLocation>,
    ) {
        self.errors.push(SchemaError {
            kind,
            range,
            related,
            message: message.into(),
        });
    }

    fn failure(mut self) -> LoadSchemaResult {
        if self.errors.is_empty() {
            self.errors.push(SchemaError {
                kind: SchemaErrorKind::InvalidDocumentShape,
                range: self.document_range,
                related: Vec::new(),
                message: "schema could not be loaded".into(),
            });
        }
        let first = self.errors.remove(0);
        Err(InvalidSchema {
            sources: self.sources,
            errors: NonEmpty {
                first,
                rest: self.errors,
            },
        })
    }
}

fn primary_sources(text: Arc<str>, label: Option<SourceLabel>) -> SchemaSources {
    SchemaSources {
        primary: SourceId(0),
        documents: BTreeMap::from([(SourceId(0), SchemaSource { label, text })]),
    }
}
