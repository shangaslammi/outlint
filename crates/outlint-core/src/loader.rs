//! Loading, semantic validation, and normalization of Outlint schemas.
//!
//! `InvalidSchema` intentionally owns all source and error data, so boxing it
//! merely to reduce the result enum would complicate the public loader API.

#![allow(clippy::result_large_err)]

use std::{
    borrow::Cow,
    collections::{BTreeMap, HashMap, HashSet},
    sync::Arc,
};

use num_bigint::{BigInt, BigUint};
use saphyr_parser::{
    Event as YamlEvent, Parser as YamlParser, ScalarStyle, Span, StrInput, Tag as YamlTag,
};
use serde::Deserialize;
use serde_json::Value;
use unicode_normalization::UnicodeNormalization;

use crate::markdown::{
    deeper_yaml_nesting, exact_yaml_scalar_to_json, validate_yaml_container_tag, ExactYamlBudget,
    ExactYamlScalar, YamlValueError,
};
use crate::matcher::{compile_anchored_pattern, compile_glob_pattern};
use crate::{
    AtLeastTwo, ByteOffset, CanonicalFloat, CanonicalInteger, Cardinality, Constraint,
    ConstraintIndex, ConstraintPath, ExactText, FrontmatterKey, FrontmatterPolicy, FrontmatterRef,
    FrontmatterScalar, GlobPattern, InvalidSchema, JsonSchemaResourceContents,
    LinkedJsonSchemaInput, LoadSchemaResult, LoadedSchema, Matcher, NonEmpty, Options,
    OutlineProvenance, Proposition, RefAnchor, RegexPattern, RelatedLocation, RuleId, RuleIndex,
    RuleOutcome, RulePath, RuleRef, Schema, SchemaError, SchemaErrorKind, SchemaLocations,
    SchemaNode, SchemaSource, SchemaSources, SchemaVersion, ScopePath, SectionRule, SourceId,
    SourceLabel, SourceRange, TextRange, UpperBound,
};

/// The object domain schema documents are validated in: JSON Schema's own.
type JsonMap = serde_json::Map<String, Value>;

/// Loads an Outlint schema from UTF-8 source text.
///
/// The returned model contains only normalized values. Errors are accumulated
/// where later checks do not depend on an earlier invalid value.
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

#[derive(Debug)]
struct PreparedExternalSchema {
    root_source: SourceId,
    result: Result<crate::FrontmatterSchema, NonEmpty<PreparedExternalError>>,
}

#[derive(Debug)]
struct PreparedExternalError {
    source: SourceId,
    message: String,
}

/// Returns the linked frontmatter schema path declared by valid outer YAML.
///
/// The path is returned exactly as declared. Resolution against a lexical file
/// location belongs to the I/O shell.
pub fn linked_frontmatter_schema_path(source: &str) -> Option<String> {
    let value = schema_yaml_to_json(parse_schema_yaml(source).ok()?).ok()?;
    value
        .as_object()?
        .get("frontmatter")?
        .as_object()?
        .get("schema")?
        .as_str()
        .map(str::to_owned)
}

/// Finds external documents referenced from one draft 2020-12 resource.
///
/// Each reference is resolved both from the resource's lexical physical URI,
/// without applying `$id`, and from its logical URI according to JSON Schema
/// `$id` semantics. Returned URIs have fragments removed and preserve
/// traversal order, including same-document references and duplicates.
pub fn json_schema_external_references(
    source: &str,
    physical_base_uri: &str,
    logical_base_uri: &str,
) -> Result<Vec<crate::JsonSchemaExternalReference>, String> {
    let value: serde_json::Value = serde_json::from_str(source)
        .map_err(|error| format!("invalid JSON Schema document: {error}"))?;
    let physical_base = jsonschema::Uri::parse(physical_base_uri.to_owned()).map_err(|error| {
        format!("invalid physical JSON Schema base URI `{physical_base_uri}`: {error:?}")
    })?;
    let logical_base = jsonschema::Uri::parse(logical_base_uri.to_owned()).map_err(|error| {
        format!("invalid logical JSON Schema base URI `{logical_base_uri}`: {error:?}")
    })?;
    let mut references = Vec::new();
    collect_external_references(&value, &physical_base, &logical_base, &mut references)?;
    Ok(references)
}

fn collect_external_references(
    value: &serde_json::Value,
    physical_base: &jsonschema::Uri<String>,
    inherited_logical_base: &jsonschema::Uri<String>,
    references: &mut Vec<crate::JsonSchemaExternalReference>,
) -> Result<(), String> {
    let mut logical_base = inherited_logical_base.clone();
    if let Some(identifier) = value
        .as_object()
        .and_then(|object| object.get("$id"))
        .and_then(serde_json::Value::as_str)
    {
        logical_base = jsonschema::uri::resolve_against(&logical_base.borrow(), identifier)
            .map_err(|error| format!("invalid JSON Schema `$id` `{identifier}`: {error}"))?;
    }
    if let Some(object) = value.as_object() {
        for keyword in ["$ref", "$dynamicRef"] {
            if let Some(reference) = object.get(keyword).and_then(serde_json::Value::as_str) {
                let physical = jsonschema::uri::resolve_against(&physical_base.borrow(), reference)
                    .map_err(|error| {
                        format!("invalid JSON Schema `{keyword}` `{reference}`: {error}")
                    })?;
                let logical = jsonschema::uri::resolve_against(&logical_base.borrow(), reference)
                    .map_err(|error| {
                    format!("invalid JSON Schema `{keyword}` `{reference}`: {error}")
                })?;
                let physical_uri = physical.as_str().split('#').next().unwrap_or_default();
                let logical_uri = logical.as_str().split('#').next().unwrap_or_default();
                if !physical_uri.is_empty() && !logical_uri.is_empty() {
                    references.push(crate::JsonSchemaExternalReference {
                        physical_uri: physical_uri.to_owned(),
                        logical_uri: logical_uri.to_owned(),
                    });
                }
            }
        }
    }
    for child in jsonschema::Draft::Draft202012.subresources_of(value) {
        collect_external_references(child, physical_base, &logical_base, references)?;
    }
    Ok(())
}

/// Refuses resolution outside an immutable preloaded registry.
#[derive(Debug)]
pub(crate) struct NoExternalRetrieve;

impl jsonschema::Retrieve for NoExternalRetrieve {
    fn retrieve(
        &self,
        uri: &jsonschema::Uri<String>,
    ) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
        Err(format!("JSON Schema resource `{uri}` was not preloaded").into())
    }
}

/// Starts a registry that can resolve only resources supplied by the caller.
pub(crate) fn preloaded_json_schema_registry<'a>() -> jsonschema::RegistryBuilder<'a> {
    jsonschema::Registry::new().retriever(NoExternalRetrieve)
}

fn prepare_external_schema(
    input: &LinkedJsonSchemaInput,
    source_ids: &BTreeMap<String, SourceId>,
) -> PreparedExternalSchema {
    let root_source = source_ids
        .get(&input.root_uri)
        .copied()
        .unwrap_or(SourceId(0));
    let result = prepare_external_schema_result(input, source_ids);
    PreparedExternalSchema {
        root_source,
        result,
    }
}

fn prepare_external_schema_result(
    input: &LinkedJsonSchemaInput,
    source_ids: &BTreeMap<String, SourceId>,
) -> Result<crate::FrontmatterSchema, NonEmpty<PreparedExternalError>> {
    let mut parsed = BTreeMap::new();
    let mut seen = HashSet::new();
    let mut errors = Vec::new();
    let mut references = 0usize;
    for (index, resource) in input.resources.iter().enumerate() {
        let source = external_source_id(index).unwrap_or(SourceId(0));
        if !seen.insert(resource.uri.clone()) {
            errors.push(PreparedExternalError {
                source,
                message: format!("duplicate JSON Schema resource URI `{}`", resource.uri),
            });
            continue;
        }
        let text = match &resource.contents {
            JsonSchemaResourceContents::Loaded(text) => text,
            JsonSchemaResourceContents::ReadFailure(message) => {
                errors.push(PreparedExternalError {
                    source,
                    message: message.clone(),
                });
                continue;
            }
        };
        let value: serde_json::Value = match serde_json::from_str(text) {
            Ok(value) => value,
            Err(error) => {
                errors.push(PreparedExternalError {
                    source,
                    message: format!("invalid linked JSON Schema document: {error}"),
                });
                continue;
            }
        };
        if let Err(message) = validate_json_schema_document(&value) {
            errors.push(PreparedExternalError { source, message });
            continue;
        }
        // The budget spans the graph rather than any one document, so it is
        // charged as the documents arrive and reported against the one that
        // spends the last of it. Nothing later can bring the total back under,
        // and an over-budget graph is never compiled, so stop reading here
        // instead of listing local faults in resources that will not be used.
        references = references.saturating_add(json_schema_reference_count(&value));
        if references > MAX_JSON_SCHEMA_REFERENCES {
            errors.push(PreparedExternalError {
                source,
                message: json_schema_reference_budget_message(),
            });
            break;
        }
        parsed.insert(resource.uri.clone(), value);
    }
    // Resource-local failures are independent and retain input order. The
    // registry is graph-dependent, so do not compile an incomplete graph or
    // add cascading resolution failures after any local error.
    if let Some(errors) = non_empty(errors) {
        return Err(errors);
    }
    let mut resources = parsed;
    let root = resources.remove(&input.root_uri).ok_or_else(|| {
        single_external_error(PreparedExternalError {
            source: root_source_id(source_ids, &input.root_uri),
            message: format!(
                "linked JSON Schema root resource `{}` was not preloaded",
                input.root_uri
            ),
        })
    })?;

    {
        let mut registry = preloaded_json_schema_registry()
            .add(input.root_uri.as_str(), &root)
            .map_err(|error| {
                single_external_error(external_registry_error(
                    error.to_string(),
                    source_ids,
                    &input.root_uri,
                ))
            })?;
        for (uri, resource) in &resources {
            registry = registry.add(uri.as_str(), resource).map_err(|error| {
                single_external_error(external_registry_error(error.to_string(), source_ids, uri))
            })?;
        }
        let registry = registry.prepare().map_err(|error| {
            single_external_error(external_registry_error(
                error.to_string(),
                source_ids,
                &input.root_uri,
            ))
        })?;
        jsonschema::draft202012::options()
            .with_registry(&registry)
            .with_base_uri(input.root_uri.clone())
            .with_retriever(NoExternalRetrieve)
            .build(&root)
            .map_err(|error| {
                let message = error.to_string();
                // jsonschema does not expose a structured resource origin, so
                // this attribution depends on its error wording containing a URI.
                let source = source_ids
                    .iter()
                    .find_map(|(uri, source)| message.contains(uri).then_some(*source))
                    .unwrap_or_else(|| root_source_id(source_ids, &input.root_uri));
                single_external_error(PreparedExternalError {
                    source,
                    message: format!("cannot compile linked JSON Schema: {message}"),
                })
            })?;
    }

    Ok(crate::FrontmatterSchema {
        root_uri: input.root_uri.clone(),
        root,
        resources,
    })
}

fn single_external_error(error: PreparedExternalError) -> NonEmpty<PreparedExternalError> {
    NonEmpty {
        first: error,
        rest: Vec::new(),
    }
}

/// How many reference keywords one linked JSON Schema graph may declare.
///
/// Compiling a reference re-enters the compiler at the target, so a chain of
/// references costs one stack frame per link however flat the documents that
/// spell it are: every link of `{"$ref":"#/x/1"}, {"$ref":"#/x/2"}, ...` sits
/// at the same JSON depth, which is why neither the YAML depth limit nor
/// `serde_json`'s parse limit sees the chain at all. What the limit therefore
/// has to bound is a count, not a nesting.
///
/// It counts occurrences rather than the longest chain because a chain's
/// length is only knowable by resolving every reference the way the compiler
/// does — through `$id`, `$anchor`, `$dynamicAnchor`, JSON pointers, and
/// across resources — and a second implementation of that resolution would
/// either refuse graphs the compiler handles or, worse, admit ones it cannot.
/// A count needs no resolution and still bounds the recursion, since a stack
/// path enters each reference keyword at most once: cycles are cut by the
/// compiler's own pending-node cache, which is why a self-reference or a
/// mutual pair compiles today rather than overflowing.
///
/// The value is the constant this crate already uses wherever structure has to
/// be bounded, and which `serde_json` defaults to. Measured
/// against the compiler, a link costs about 1.7 KB of stack optimized and
/// about 6 KB unoptimized, so 128 links stay under a megabyte in the tightest
/// configuration — an unoptimized build on a two-megabyte thread, where the
/// abort begins around 345 links. Measured against real schemas it is far
/// above anything one contains: the whole conformance corpus declares at most
/// four references across a linked graph and its deepest chain is two.
pub(crate) const MAX_JSON_SCHEMA_REFERENCES: usize = 128;

/// Reports the shared wording for a graph that spends more than the budget.
pub(crate) fn json_schema_reference_budget_message() -> String {
    format!(
        "linked JSON Schema declares more than {MAX_JSON_SCHEMA_REFERENCES} \
         `$ref` or `$dynamicRef` keywords"
    )
}

/// Counts the `$ref` and `$dynamicRef` keywords one JSON document declares.
///
/// The walk carries an explicit stack rather than recursing, so what it costs
/// the call stack does not depend on the limit it enforces. A counter that
/// recursed would be safe only while a limit's worth of its own frames still
/// fit, which ties the choice of limit to the shape of the check and would
/// turn a later raise of the limit into the very overflow being refused here.
/// Those two keywords are the ones whose
/// compilation re-enters the compiler under draft 2020-12, the only dialect
/// [`validate_json_schema_document`] admits, and they are the same pair
/// [`collect_external_references`] follows.
///
/// Every occurrence of either name is counted, including one used as a
/// property name or buried in a `const`, which cannot drive the compiler and
/// so only makes the bound stricter than it needs to be. Distinguishing them
/// would mean knowing which positions are schemas, which is the resolution
/// this count exists to avoid.
pub(crate) fn json_schema_reference_count(value: &serde_json::Value) -> usize {
    let mut references = 0usize;
    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        match value {
            serde_json::Value::Object(object) => {
                for (keyword, child) in object {
                    if matches!(keyword.as_str(), "$ref" | "$dynamicRef") {
                        references = references.saturating_add(1);
                    }
                    pending.push(child);
                }
            }
            serde_json::Value::Array(items) => pending.extend(items),
            _ => {}
        }
    }
    references
}

fn validate_json_schema_document(value: &serde_json::Value) -> Result<(), String> {
    if !value.is_object() && !value.is_boolean() {
        return Err("linked JSON Schema root must be an object or boolean".into());
    }
    if let Some(dialect) = value.as_object().and_then(|object| object.get("$schema")) {
        let supported = dialect.as_str().is_some_and(|dialect| {
            matches!(
                dialect,
                "https://json-schema.org/draft/2020-12/schema"
                    | "https://json-schema.org/draft/2020-12/schema#"
            )
        });
        if !supported {
            return Err(format!(
                "unsupported JSON Schema dialect `{dialect}`; expected draft 2020-12"
            ));
        }
    }
    jsonschema::draft202012::meta::validate(value)
        .map_err(|error| format!("invalid draft 2020-12 JSON Schema: {error}"))
}

fn root_source_id(source_ids: &BTreeMap<String, SourceId>, root_uri: &str) -> SourceId {
    source_ids.get(root_uri).copied().unwrap_or(SourceId(0))
}

fn external_source_id(index: usize) -> Option<SourceId> {
    u32::try_from(index)
        .ok()
        .and_then(|index| index.checked_add(1))
        .map(SourceId)
}

fn external_registry_error(
    message: String,
    source_ids: &BTreeMap<String, SourceId>,
    fallback_uri: &str,
) -> PreparedExternalError {
    // jsonschema exposes only display text here; source attribution therefore
    // intentionally depends on the message retaining the resource URI.
    let source = source_ids
        .iter()
        .find_map(|(uri, source)| message.contains(uri).then_some(*source))
        .unwrap_or_else(|| root_source_id(source_ids, fallback_uri));
    PreparedExternalError { source, message }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSchema {
    version: i64,
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
    constraints: Vec<Value>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFrontmatter {
    required: Option<bool>,
    allow: Option<bool>,
    schema: Option<RawFrontmatterSchema>,
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
    #[serde(default = "default_true")]
    allow: bool,
    required: Option<bool>,
    repeat: Option<String>,
    #[serde(default)]
    strict: bool,
    #[serde(default)]
    sections: Vec<RawRule>,
    #[serde(default)]
    constraints: Vec<Value>,
}

const fn default_true() -> bool {
    true
}

const DOCUMENT_FIELDS: &[&str] = &[
    "version",
    "title",
    "options",
    "frontmatter",
    "outline",
    "sections",
    "constraints",
];
const OPTION_FIELDS: &[&str] = &["match_case", "strip_inline_markup", "allow_skipped_levels"];
const FRONTMATTER_FIELDS: &[&str] = &["required", "allow", "schema"];
const RULE_FIELDS: &[&str] = &[
    "id",
    "match",
    "allow",
    "required",
    "repeat",
    "strict",
    "sections",
    "constraints",
];

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum RangeKey {
    DocumentField(String),
    OptionField(String),
    FrontmatterField(String),
    Rule(RulePath),
    RuleField(RulePath, String),
    Constraint(ConstraintPath),
    /// An `h1`-level rule in the top-level `outline` list.
    OutlineRule(RuleIndex),
    /// One field of an `h1`-level rule in the top-level `outline` list.
    OutlineRuleField(RuleIndex, String),
}

#[derive(Default)]
struct RangeIndex {
    ranges: BTreeMap<RangeKey, SourceRange>,
}

impl RangeIndex {
    /// Reads every addressable range off the one tree the loader parsed.
    ///
    /// The walk mirrors the shape validation below: document fields, options,
    /// frontmatter, and the rule and constraint forests. Lookups are linear
    /// scans over each mapping's ordered entries, which is the right cost for
    /// schema documents — a mapping here has a handful of keys, and the parse
    /// has already rejected duplicates, so the first match is the only one.
    fn from_tree(root: &SchemaYamlNode, char_offsets: &[usize]) -> Self {
        let mut index = Self::default();
        let Some(mapping) = root.as_mapping() else {
            return index;
        };
        let expansion = subtree_expansion(root, None);
        for &field in DOCUMENT_FIELDS {
            if let Some(node) = schema_mapping_get(mapping, field) {
                index.ranges.insert(
                    RangeKey::DocumentField(field.into()),
                    node_range(node, expansion, char_offsets),
                );
            }
        }
        for (section, fields) in [
            ("options", OPTION_FIELDS),
            ("frontmatter", FRONTMATTER_FIELDS),
        ] {
            let Some(node) = schema_mapping_get(mapping, section) else {
                continue;
            };
            let expansion = subtree_expansion(node, expansion);
            let Some(entries) = node.as_mapping() else {
                continue;
            };
            for &field in fields {
                if let Some(value) = schema_mapping_get(entries, field) {
                    index.ranges.insert(
                        match section {
                            "options" => RangeKey::OptionField(field.into()),
                            _ => RangeKey::FrontmatterField(field.into()),
                        },
                        node_range(value, expansion, char_offsets),
                    );
                }
            }
        }
        if let Some(node) = schema_mapping_get(mapping, "sections") {
            let expansion = subtree_expansion(node, expansion);
            if let Some(sections) = node.as_sequence() {
                index.collect_rules(sections, &ScopePath(Vec::new()), expansion, char_offsets);
            }
        } else if let Some(node) = schema_mapping_get(mapping, "outline") {
            // `outline` and `sections` share the nested-rule key space: an
            // outline rule's children live in the scope its index names. The
            // two lists are mutually exclusive, so when both appear the load
            // is already failing and only the `sections` forest — the one the
            // legacy validation errors point into — keeps its ranges.
            let expansion = subtree_expansion(node, expansion);
            if let Some(entries) = node.as_sequence() {
                index.collect_outline(entries, expansion, char_offsets);
            }
        }
        if let Some(node) = schema_mapping_get(mapping, "constraints") {
            let expansion = subtree_expansion(node, expansion);
            if let Some(constraints) = node.as_sequence() {
                index.collect_constraints(
                    constraints,
                    &ScopePath(Vec::new()),
                    expansion,
                    char_offsets,
                );
            }
        }
        index
    }

    fn collect_rules(
        &mut self,
        rules: &[SchemaYamlNode],
        scope: &ScopePath,
        expansion: Option<(usize, usize)>,
        char_offsets: &[usize],
    ) {
        for (index, node) in rules.iter().enumerate() {
            let path = RulePath {
                scope: scope.clone(),
                index: RuleIndex(index),
            };
            self.ranges.insert(
                RangeKey::Rule(path.clone()),
                node_range(node, expansion, char_offsets),
            );
            let expansion = subtree_expansion(node, expansion);
            let Some(mapping) = node.as_mapping() else {
                continue;
            };
            for &field in RULE_FIELDS {
                if let Some(value) = schema_mapping_get(mapping, field) {
                    self.ranges.insert(
                        RangeKey::RuleField(path.clone(), field.into()),
                        node_range(value, expansion, char_offsets),
                    );
                }
            }
            let mut child_scope = scope.clone();
            child_scope.0.push(RuleIndex(index));
            if let Some(node) = schema_mapping_get(mapping, "sections") {
                let expansion = subtree_expansion(node, expansion);
                if let Some(children) = node.as_sequence() {
                    self.collect_rules(children, &child_scope, expansion, char_offsets);
                }
            }
            if let Some(node) = schema_mapping_get(mapping, "constraints") {
                let expansion = subtree_expansion(node, expansion);
                if let Some(constraints) = node.as_sequence() {
                    self.collect_constraints(constraints, &child_scope, expansion, char_offsets);
                }
            }
        }
    }

    fn collect_outline(
        &mut self,
        entries: &[SchemaYamlNode],
        expansion: Option<(usize, usize)>,
        char_offsets: &[usize],
    ) {
        for (index, node) in entries.iter().enumerate() {
            self.ranges.insert(
                RangeKey::OutlineRule(RuleIndex(index)),
                node_range(node, expansion, char_offsets),
            );
            let expansion = subtree_expansion(node, expansion);
            let Some(mapping) = node.as_mapping() else {
                continue;
            };
            for &field in RULE_FIELDS {
                if let Some(value) = schema_mapping_get(mapping, field) {
                    self.ranges.insert(
                        RangeKey::OutlineRuleField(RuleIndex(index), field.into()),
                        node_range(value, expansion, char_offsets),
                    );
                }
            }
            let child_scope = ScopePath(vec![RuleIndex(index)]);
            if let Some(node) = schema_mapping_get(mapping, "sections") {
                let expansion = subtree_expansion(node, expansion);
                if let Some(children) = node.as_sequence() {
                    self.collect_rules(children, &child_scope, expansion, char_offsets);
                }
            }
            if let Some(node) = schema_mapping_get(mapping, "constraints") {
                let expansion = subtree_expansion(node, expansion);
                if let Some(constraints) = node.as_sequence() {
                    self.collect_constraints(constraints, &child_scope, expansion, char_offsets);
                }
            }
        }
    }

    fn collect_constraints(
        &mut self,
        constraints: &[SchemaYamlNode],
        scope: &ScopePath,
        expansion: Option<(usize, usize)>,
        char_offsets: &[usize],
    ) {
        for (index, node) in constraints.iter().enumerate() {
            self.ranges.insert(
                RangeKey::Constraint(ConstraintPath {
                    scope: scope.clone(),
                    index: ConstraintIndex(index),
                }),
                node_range(node, expansion, char_offsets),
            );
        }
    }

    fn get(&self, key: &RangeKey, fallback: SourceRange) -> SourceRange {
        self.ranges.get(key).copied().unwrap_or(fallback)
    }
}

/// One node of the tree the schema loader builds out of parser events.
///
/// A mapping keeps its entries as an ordered `Vec` rather than a map so that
/// two keys spelled differently but resolving alike stay visible to the
/// duplicate checks, and so that a key which is not a scalar at all still has
/// somewhere to live until the conversion rejects it. Collection tags are
/// validated as the events arrive — a schema document may carry no
/// non-standard tag at all — so only scalars still hold theirs, for the
/// conversion that resolves their values.
#[derive(Clone, Debug)]
struct SchemaYamlNode {
    kind: SchemaYamlKind,
    /// Half-open character-index range of the node's own spelling. A scalar's
    /// end is the parser's own, which is real under this engine; a
    /// collection's start event is zero-width but its marker sits on the
    /// collection's first token, so the range pairs it with the end event's
    /// far edge.
    start: usize,
    end: usize,
    /// The node is an alias's copy, and its range is the alias site. The whole
    /// copy anchors there: the ranges its entries carry belong to the anchor's
    /// definition, which is not the entry a range key into the copy names, and
    /// §6.2 permits the nearest enclosing entry with a position of its own —
    /// which the alias site is.
    expanded: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum SchemaYamlKind {
    Scalar(ExactYamlScalar),
    Sequence(Vec<SchemaYamlNode>),
    Mapping(Vec<(SchemaYamlNode, SchemaYamlNode)>),
}

/// Equality ignores positions: the duplicate checks ask whether two keys are
/// the same key, and two spellings of one key are no less duplicates for
/// sitting on different lines.
impl PartialEq for SchemaYamlNode {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind
    }
}

impl Eq for SchemaYamlNode {}

impl std::hash::Hash for SchemaYamlNode {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.kind.hash(state);
    }
}

impl SchemaYamlNode {
    fn as_mapping(&self) -> Option<&[(SchemaYamlNode, SchemaYamlNode)]> {
        match &self.kind {
            SchemaYamlKind::Mapping(entries) => Some(entries),
            _ => None,
        }
    }

    fn as_sequence(&self) -> Option<&[SchemaYamlNode]> {
        match &self.kind {
            SchemaYamlKind::Sequence(values) => Some(values),
            _ => None,
        }
    }

    fn scalar_text(&self) -> Option<&str> {
        match &self.kind {
            SchemaYamlKind::Scalar(scalar) => Some(&scalar.value),
            _ => None,
        }
    }
}

/// The value of the entry whose key spells `key`, by linear scan.
///
/// Scalar keys compare on their text, so `version` and `"version"` name the
/// same field here exactly as they collide in the JSON object the document
/// converts to.
fn schema_mapping_get<'a>(
    entries: &'a [(SchemaYamlNode, SchemaYamlNode)],
    key: &str,
) -> Option<&'a SchemaYamlNode> {
    entries
        .iter()
        .find(|(candidate, _)| candidate.scalar_text() == Some(key))
        .map(|(_, value)| value)
}

/// The range a subtree's entries anchor at, once an alias expansion encloses
/// them: the alias site's own range, carried down from the copy's root.
fn subtree_expansion(
    node: &SchemaYamlNode,
    inherited: Option<(usize, usize)>,
) -> Option<(usize, usize)> {
    inherited.or_else(|| node.expanded.then_some((node.start, node.end)))
}

/// Converts one node's character-index range into a byte-offset source range.
fn node_range(
    node: &SchemaYamlNode,
    expansion: Option<(usize, usize)>,
    char_offsets: &[usize],
) -> SourceRange {
    let (start, end) = expansion.unwrap_or((node.start, node.end));
    char_range(start, end, char_offsets)
}

/// A half-open character-index range as a byte-offset range into the source.
///
/// `saphyr-parser` markers count characters, while Outlint source ranges are
/// UTF-8 byte offsets; the caller's table bridges the units, so no marker
/// index ever slices the source directly. A zero-width range — a parse
/// error's marker, or a scalar the parser synthesised for an entry with no
/// spelling of its own — is widened to the one character it points at, so a
/// caret has something to sit under; at the end of input it stays empty.
fn char_range(start: usize, end: usize, char_offsets: &[usize]) -> SourceRange {
    let source_end = char_offsets.last().copied().unwrap_or(0);
    let start_byte = char_offsets.get(start).copied().unwrap_or(source_end);
    let mut end_byte = char_offsets
        .get(end)
        .copied()
        .unwrap_or(source_end)
        .max(start_byte);
    if end_byte <= start_byte {
        end_byte = char_offsets
            .get(start + 1)
            .copied()
            .unwrap_or(end_byte)
            .max(end_byte);
    }
    SourceRange {
        source: SourceId(0),
        range: TextRange {
            start: ByteOffset(start_byte),
            end: ByteOffset(end_byte),
        },
    }
}

/// A schema document the YAML engine refused, before validation began.
///
/// The range is in character indices — `None` anchors at the whole document —
/// and the kind rides along because not every refusal is a syntax error: a
/// non-string mapping key, for example, is a shape complaint with a position.
#[derive(Debug)]
struct SchemaYamlError {
    kind: SchemaErrorKind,
    span: Option<(usize, usize)>,
    message: String,
}

impl SchemaYamlError {
    fn syntax(span: &Span, mark: usize, message: String) -> Self {
        Self {
            kind: SchemaErrorKind::Syntax,
            span: Some((
                span.start.index() + mark,
                (span.end.index() + mark).max(span.start.index() + mark),
            )),
            message,
        }
    }
}

/// A parsed node held for the aliases that name it, with its size and depth.
///
/// Both numbers exist so an alias can be charged before the copy is made: the
/// size against the node budget, and the depth against the nesting limit the
/// copy carries to wherever it lands.
#[derive(Debug)]
struct AnchoredSchemaYamlNode {
    node: SchemaYamlNode,
    nodes: usize,
    depth: usize,
}

/// A node just built, beside how deeply its own collections nest — carried out
/// of the build because measuring it afterwards would be another walk of the
/// same recursion the depth bound exists to keep within the stack.
#[derive(Debug)]
struct SchemaYamlSubtree {
    node: SchemaYamlNode,
    depth: usize,
}

/// Builds the schema tree by pulling one event at a time from `saphyr-parser`.
///
/// This is the schema-document counterpart of the frontmatter reader in
/// `markdown.rs`, and it carries the same three protections through the same
/// shared machinery: the [`ExactYamlBudget`] that bounds alias expansion by
/// the input's own size, the [`MAX_YAML_DEPTH`](crate::markdown::MAX_YAML_DEPTH)
/// bound charged as the recursion descends, and the
/// alias-charged-before-clone ordering that refuses a bomb before building
/// it. What differs is only what a node remembers — character spans for
/// [`RangeIndex`], where frontmatter keeps line and column — and the words a
/// refusal is reported in.
struct SchemaYamlReader<'source> {
    parser: YamlParser<'source, StrInput<'source>>,
    anchors: BTreeMap<usize, AnchoredSchemaYamlNode>,
    budget: ExactYamlBudget,
    /// Characters removed from the head of the source before parsing — a
    /// byte-order mark or nothing — counted back into every reported index.
    mark: usize,
}

impl<'source> SchemaYamlReader<'source> {
    fn new(source: &'source str, mark: usize) -> Self {
        Self {
            parser: YamlParser::new_from_str(source),
            anchors: BTreeMap::new(),
            budget: ExactYamlBudget::default(),
            mark,
        }
    }

    /// Reads the next event, charging the budget for the input it took.
    fn next_event(&mut self) -> Result<(YamlEvent<'source>, Span), SchemaYamlError> {
        self.budget.events += 1;
        match self.parser.next_event() {
            Some(Ok(read)) => Ok(read),
            Some(Err(error)) => {
                let marker = error.marker();
                let span = Span::new(*marker, *marker);
                // `ScanError`'s own rendering calls its character index a byte
                // and holds a zero-based column, so the position is respelled:
                // a one-based line and a one-based character column, with a
                // removed byte-order mark counted back into the first line.
                let column = marker.col() + 1 + if marker.line() == 1 { self.mark } else { 0 };
                Err(SchemaYamlError::syntax(
                    &span,
                    self.mark,
                    format!(
                        "invalid YAML: {} at line {} column {column}",
                        error.info(),
                        marker.line(),
                    ),
                ))
            }
            None => Err(SchemaYamlError {
                kind: SchemaErrorKind::Syntax,
                span: None,
                message: "invalid YAML: the document ends before its structure does".into(),
            }),
        }
    }

    /// Refuses the second document a schema must not contain, at its start.
    ///
    /// The refusal lands before any of the second document's content is read:
    /// raw `next_event` does not clear the parser's anchor table between
    /// documents, so reading on would resolve the second document's aliases
    /// against the first one's anchors. The serde-era engine reported this
    /// verdict with no location at all; the start event's span is a real one.
    fn second_document_error(&self, span: &Span) -> SchemaYamlError {
        let column = span.start.col() + 1 + if span.start.line() == 1 { self.mark } else { 0 };
        SchemaYamlError::syntax(
            span,
            self.mark,
            format!(
                "invalid YAML: a second document opens at line {} column {column}; \
                 a schema is a single YAML document",
                span.start.line(),
            ),
        )
    }

    /// Rejects every tag outside the `tag:yaml.org,2002:` namespace.
    ///
    /// The core-schema tags keep the meaning the conversion gives them; a
    /// non-standard tag has no meaning a schema document could put to use, and
    /// the engine this loader left rejected such documents too.
    fn reject_non_standard_tag(
        &self,
        tag: Option<&YamlTag>,
        span: &Span,
    ) -> Result<(), SchemaYamlError> {
        match tag {
            Some(tag) if !tag.is_yaml_core_schema() => Err(SchemaYamlError::syntax(
                span,
                self.mark,
                format!(
                    "invalid YAML: non-standard tag `{}{}`",
                    tag.handle, tag.suffix
                ),
            )),
            _ => Ok(()),
        }
    }

    fn depth_error(&self, span: &Span) -> SchemaYamlError {
        SchemaYamlError::syntax(
            span,
            self.mark,
            "invalid YAML: nesting exceeds the depth limit".into(),
        )
    }

    fn budget_error(&self, span: &Span) -> SchemaYamlError {
        SchemaYamlError::syntax(
            span,
            self.mark,
            "invalid YAML: alias expansion exceeds the document's size limit".into(),
        )
    }

    fn value_error(&self, error: YamlValueError, span: &Span) -> SchemaYamlError {
        SchemaYamlError::syntax(span, self.mark, schema_value_error(error))
    }

    /// Builds the node the given event opens, reading whatever it contains.
    ///
    /// `depth` counts the collections already open around this node; the
    /// document's own root mapping is the first level, and the bound is
    /// charged before the frame is taken rather than after. What the node
    /// reaches below itself is returned with it, since an alias to it has to
    /// be charged that depth at a site this call knows nothing of.
    fn node(
        &mut self,
        event: YamlEvent<'source>,
        span: Span,
        depth: usize,
    ) -> Result<SchemaYamlSubtree, SchemaYamlError> {
        let spent = self.budget.nodes;
        let start = span.start.index() + self.mark;
        let (kind, end, anchor, reached) = match event {
            YamlEvent::Scalar(value, style, anchor, tag) => {
                let tag = tag.map(Cow::into_owned);
                self.reject_non_standard_tag(tag.as_ref(), &span)?;
                self.budget.spend(1).map_err(|_| self.budget_error(&span))?;
                (
                    SchemaYamlKind::Scalar(ExactYamlScalar {
                        value: value.into_owned(),
                        style,
                        tag,
                    }),
                    span.end.index() + self.mark,
                    anchor,
                    0,
                )
            }
            YamlEvent::SequenceStart(anchor, tag) => {
                let tag = tag.map(Cow::into_owned);
                self.reject_non_standard_tag(tag.as_ref(), &span)?;
                validate_yaml_container_tag(tag.as_ref(), "seq")
                    .map_err(|error| self.value_error(error, &span))?;
                let depth = deeper_yaml_nesting(depth, 1).map_err(|_| self.depth_error(&span))?;
                self.budget.spend(1).map_err(|_| self.budget_error(&span))?;
                let mut values = Vec::new();
                let mut inner = 0;
                let end;
                loop {
                    let (event, span) = self.next_event()?;
                    if matches!(event, YamlEvent::SequenceEnd) {
                        end = span.end.index() + self.mark;
                        break;
                    }
                    let value = self.node(event, span, depth)?;
                    inner = inner.max(value.depth);
                    values.push(value.node);
                }
                (SchemaYamlKind::Sequence(values), end, anchor, inner + 1)
            }
            YamlEvent::MappingStart(anchor, tag) => {
                let tag = tag.map(Cow::into_owned);
                self.reject_non_standard_tag(tag.as_ref(), &span)?;
                validate_yaml_container_tag(tag.as_ref(), "map")
                    .map_err(|error| self.value_error(error, &span))?;
                let depth = deeper_yaml_nesting(depth, 1).map_err(|_| self.depth_error(&span))?;
                self.budget.spend(1).map_err(|_| self.budget_error(&span))?;
                let mut entries: Vec<(SchemaYamlNode, SchemaYamlNode)> = Vec::new();
                let mut keys: BTreeMap<u64, Vec<usize>> = BTreeMap::new();
                let mut inner = 0;
                let end;
                loop {
                    let (event, span) = self.next_event()?;
                    if matches!(event, YamlEvent::MappingEnd) {
                        end = span.end.index() + self.mark;
                        break;
                    }
                    let key = self.node(event, span, depth)?;
                    let (event, span) = self.next_event()?;
                    let value = self.node(event, span, depth)?;
                    inner = inner.max(key.depth).max(value.depth);
                    let (key, value) = (key.node, value.node);
                    // Whole-node equality catches the keys the conversion
                    // never reduces to a string; keys that do resolve are
                    // caught again there, on the resolved text. The digest
                    // narrows the candidates so an aliased flood of large
                    // keys costs hashes rather than quadratic comparisons.
                    let digest = schema_yaml_key_digest(&key);
                    let alike = keys.entry(digest).or_default();
                    if alike.iter().any(|&entry| entries[entry].0 == key) {
                        return Err(duplicate_schema_key_error(&key));
                    }
                    alike.push(entries.len());
                    entries.push((key, value));
                }
                (SchemaYamlKind::Mapping(entries), end, anchor, inner + 1)
            }
            YamlEvent::Alias(anchor) => {
                let Some(anchored) = self.anchors.get(&anchor) else {
                    return Err(SchemaYamlError::syntax(
                        &span,
                        self.mark,
                        "invalid YAML: unresolved alias".into(),
                    ));
                };
                // Charged before the clone, size and depth both: a tree too
                // large or too deep to walk must not be built in order to
                // discover that it is.
                let (nodes, reached) = (anchored.nodes, anchored.depth);
                deeper_yaml_nesting(depth, reached).map_err(|_| self.depth_error(&span))?;
                self.budget
                    .spend(nodes)
                    .map_err(|_| self.budget_error(&span))?;
                let mut node = self
                    .anchors
                    .get(&anchor)
                    .expect("charged against a node the table holds")
                    .node
                    .clone();
                // The whole copy anchors at the alias site: its root takes the
                // site's own span, and `expanded` tells every walk to carry
                // that range over the definition spans the copy's entries
                // still hold. See [`SchemaYamlNode::expanded`].
                node.start = start;
                node.end = (span.end.index() + self.mark).max(start);
                node.expanded = true;
                return Ok(SchemaYamlSubtree {
                    node,
                    depth: reached,
                });
            }
            _ => {
                return Err(SchemaYamlError {
                    kind: SchemaErrorKind::Syntax,
                    span: None,
                    message: "invalid YAML: unexpected document boundary".into(),
                })
            }
        };
        let node = SchemaYamlNode {
            kind,
            start,
            end: end.max(start),
            expanded: false,
        };
        if anchor != 0 {
            // Anchor zero is `saphyr-parser`'s "no anchor", and a node is
            // registered only once it is built, so a collection cannot alias
            // itself: the alias inside is refused as unresolved.
            self.anchors.insert(
                anchor,
                AnchoredSchemaYamlNode {
                    node: node.clone(),
                    nodes: self.budget.nodes - spent,
                    depth: reached,
                },
            );
        }
        Ok(SchemaYamlSubtree {
            node,
            depth: reached,
        })
    }
}

/// Digests a mapping key so only the keys that could equal it are compared.
fn schema_yaml_key_digest(key: &SchemaYamlNode) -> u64 {
    let mut hasher = std::hash::DefaultHasher::new();
    std::hash::Hash::hash(key, &mut hasher);
    std::hash::Hasher::finish(&hasher)
}

/// Names a duplicate mapping key at the duplicate occurrence's own range.
fn duplicate_schema_key_error(key: &SchemaYamlNode) -> SchemaYamlError {
    SchemaYamlError {
        kind: SchemaErrorKind::Syntax,
        span: Some((key.start, key.end)),
        message: match key.scalar_text() {
            Some(text) => format!("invalid YAML: duplicate mapping key `{text}`"),
            None => "invalid YAML: duplicate mapping key".into(),
        },
    }
}

/// Reads a schema document's one YAML document, keeping every span.
///
/// A leading byte-order mark is removed before parsing — the parser would
/// otherwise deliver it as the first character of the first key, leaving a
/// document whose `version` entry is invisibly named something else — and
/// every reported index counts it back in. A source holding no document at
/// all parses as an empty scalar, which the shape validation then rejects as
/// the non-mapping it is. A second document is refused at its own start
/// marker; see [`SchemaYamlReader::second_document_error`].
fn parse_schema_yaml(source: &str) -> Result<SchemaYamlNode, SchemaYamlError> {
    let (body, mark) = match source.strip_prefix('\u{feff}') {
        Some(body) => (body, 1),
        None => (source, 0),
    };
    let mut reader = SchemaYamlReader::new(body, mark);
    let boundary_error = || SchemaYamlError {
        kind: SchemaErrorKind::Syntax,
        span: None,
        message: "invalid YAML: unexpected document boundary".into(),
    };
    let (event, _) = reader.next_event()?;
    if !matches!(event, YamlEvent::StreamStart) {
        return Err(boundary_error());
    }
    let (event, _) = reader.next_event()?;
    if matches!(event, YamlEvent::StreamEnd) {
        // Nothing but comments or blank lines: the empty scalar the YAML data
        // model gives such a stream, which fails shape validation as a null.
        return Ok(SchemaYamlNode {
            kind: SchemaYamlKind::Scalar(ExactYamlScalar {
                value: "~".into(),
                style: ScalarStyle::Plain,
                tag: None,
            }),
            start: 0,
            end: 0,
            expanded: false,
        });
    }
    if !matches!(event, YamlEvent::DocumentStart(_)) {
        return Err(boundary_error());
    }
    let (event, span) = reader.next_event()?;
    let value = reader.node(event, span, 0)?.node;
    let (event, _) = reader.next_event()?;
    if !matches!(event, YamlEvent::DocumentEnd) {
        return Err(boundary_error());
    }
    match reader.next_event()? {
        (YamlEvent::StreamEnd, _) => Ok(value),
        (YamlEvent::DocumentStart(_), span) => Err(reader.second_document_error(&span)),
        _ => Err(boundary_error()),
    }
}

/// Converts the parsed tree into the JSON value domain validation runs in.
///
/// Scalars resolve through the same conversion the frontmatter path uses, so
/// a scalar means the same thing in both document kinds, §1.6-exactness
/// included. Mapping keys must resolve to strings here — the JSON object this
/// builds has no other kind of key — and the resolved text is where two
/// spellings of one key are recognised as the duplicate they are.
fn schema_yaml_to_json(node: SchemaYamlNode) -> Result<Value, SchemaYamlError> {
    let span = (node.start, node.end);
    match node.kind {
        SchemaYamlKind::Scalar(scalar) => {
            exact_yaml_scalar_to_json(scalar).map_err(|error| SchemaYamlError {
                kind: SchemaErrorKind::Syntax,
                span: Some(span),
                message: schema_value_error(error),
            })
        }
        SchemaYamlKind::Sequence(values) => Ok(Value::Array(
            values
                .into_iter()
                .map(schema_yaml_to_json)
                .collect::<Result<_, _>>()?,
        )),
        SchemaYamlKind::Mapping(entries) => {
            let mut object = JsonMap::new();
            for (key, value) in entries {
                let key_span = (key.start, key.end);
                let non_string_key = || SchemaYamlError {
                    kind: SchemaErrorKind::InvalidDocumentShape,
                    span: Some(key_span),
                    message: "mapping keys must be strings".into(),
                };
                let SchemaYamlKind::Scalar(scalar) = key.kind else {
                    return Err(non_string_key());
                };
                let Value::String(key) =
                    exact_yaml_scalar_to_json(scalar).map_err(|error| SchemaYamlError {
                        kind: SchemaErrorKind::Syntax,
                        span: Some(key_span),
                        message: schema_value_error(error),
                    })?
                else {
                    return Err(non_string_key());
                };
                let value = schema_yaml_to_json(value)?;
                if object.contains_key(&key) {
                    return Err(SchemaYamlError {
                        kind: SchemaErrorKind::Syntax,
                        span: Some(key_span),
                        message: format!("invalid YAML: duplicate mapping key `{key}`"),
                    });
                }
                object.insert(key, value);
            }
            Ok(Value::Object(object))
        }
    }
}

/// The schema-document wording for a scalar or tag with no JSON value.
fn schema_value_error(error: YamlValueError) -> String {
    match error {
        YamlValueError::TaggedNull => "invalid YAML: invalid explicitly tagged null".into(),
        YamlValueError::TaggedBool => "invalid YAML: invalid explicitly tagged boolean".into(),
        YamlValueError::TaggedInt => "invalid YAML: invalid explicitly tagged integer".into(),
        YamlValueError::TaggedFloat => "invalid YAML: invalid explicitly tagged float".into(),
        YamlValueError::ScalarTag => "invalid YAML: invalid tag for a YAML scalar".into(),
        YamlValueError::ContainerTag(expected) => {
            format!("invalid YAML: invalid tag for a YAML {expected}")
        }
        YamlValueError::NonFinite => "invalid YAML: a non-finite number has no JSON value".into(),
        YamlValueError::Unrepresentable { lexeme, error } => {
            format!("invalid YAML: number `{lexeme}` is not representable: {error}")
        }
    }
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
        let value = match schema_yaml_to_json(tree) {
            Ok(value) => value,
            Err(error) => {
                self.push_yaml_error(error);
                return self.failure();
            }
        };

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

        let version_range = self.range(RangeKey::DocumentField("version".into()));
        let version = if raw.version == 1 {
            Some(SchemaVersion::V1)
        } else {
            self.error_at(
                SchemaErrorKind::UnsupportedVersion,
                version_range,
                format!("unsupported schema version {}; expected 1", raw.version),
            );
            None
        };

        let frontmatter = self.build_frontmatter(raw.frontmatter, frontmatter_declared);

        let match_case = raw.options.match_case.unwrap_or(false);
        let options = Self::build_options(&raw.options);
        let root_scope = ScopePath(Vec::new());
        // The empty scope key names what the source's top level spelled: the
        // outline scope for the general form, the `sections` scope for sugar.
        // `constraints_mut` routes it to the matching place in the built
        // schema, so both forms share the collection here.
        self.raw_constraints
            .insert(root_scope.clone(), raw.constraints);
        let (outline, outline_provenance) = if let Some(entries) = raw.outline {
            self.outline_general = true;
            (
                self.build_outline_scope(entries, &root_scope, match_case),
                OutlineProvenance::Outline,
            )
        } else {
            let outline_provenance = if title_null {
                OutlineProvenance::NoTitle
            } else if raw.title.is_some() {
                OutlineProvenance::Title
            } else {
                OutlineProvenance::BareSections
            };
            let title = raw.title.as_deref().and_then(|matcher| {
                let range = self.range(RangeKey::DocumentField("title".into()));
                self.nodes.insert(SchemaNode::Title, range);
                self.build_matcher(matcher, match_case, range)
            });
            if title_null {
                let range = self.range(RangeKey::DocumentField("title".into()));
                self.nodes.insert(SchemaNode::Title, range);
            }
            if outline_provenance == OutlineProvenance::BareSections {
                // Bare `sections:` implies `title: "*"`, but there is no
                // `title:` key to anchor title diagnostics on. The `sections`
                // key is the spelling that implied the rule, so it carries
                // the anchor.
                let range = self.range(RangeKey::DocumentField("sections".into()));
                self.nodes.insert(SchemaNode::Title, range);
            }
            let sections = self.build_scope(
                raw.sections
                    .expect("the shape validation requires `sections` without `outline`"),
                &root_scope,
                match_case,
            );
            // The sugar desugars UP into the canonical h1-rule list: one
            // synthesized rule whose matcher is the declared title (any text
            // when none is declared), required exactly once — or denied for
            // `title: null` — with the `sections` list as its child scope.
            // The rule has no id and no spelling of its own: publicly it is
            // `SchemaNode::Title`, and public scopes address its children.
            let outline = sections.map(|sections| {
                vec![SectionRule {
                    id: None,
                    // A failed title matcher already pushed its error; the
                    // any-text placeholder never reaches a caller because the
                    // load fails below.
                    matcher: title.unwrap_or(Matcher::Any),
                    outcome: if title_null {
                        RuleOutcome::Deny
                    } else {
                        RuleOutcome::Allow(Cardinality {
                            min: 1,
                            max: UpperBound::Bounded(1),
                        })
                    },
                    strict: false,
                    sections,
                    constraints: Vec::new(),
                }]
            });
            (outline, outline_provenance)
        };

        let (Some(version), Some(frontmatter), Some(outline)) = (version, frontmatter, outline)
        else {
            self.validate_constraint_lexical_refs();
            return self.failure();
        };
        let mut schema = Schema {
            version,
            options,
            frontmatter,
            outline,
            constraints: Vec::new(),
            outline_provenance,
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

    fn validate_document_shape(&mut self, value: &Value) {
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

    fn build_frontmatter(
        &mut self,
        raw: Option<RawFrontmatter>,
        declared: bool,
    ) -> Option<FrontmatterPolicy> {
        if !declared {
            return Some(FrontmatterPolicy::Optional { schema: None });
        }
        let frontmatter_range = self.range(RangeKey::DocumentField("frontmatter".into()));
        self.nodes
            .insert(SchemaNode::Frontmatter, frontmatter_range);
        let raw = raw?;
        let required = raw.required.unwrap_or(false);
        let allow = raw.allow.unwrap_or(true);
        if required && !allow {
            self.error_at(
                SchemaErrorKind::ConflictingFrontmatter,
                frontmatter_range,
                "frontmatter cannot be both required and forbidden",
            );
            return None;
        }
        let schema = match raw.schema {
            None => None,
            Some(RawFrontmatterSchema::Path(_path)) => {
                let schema_range = self.range(RangeKey::FrontmatterField("schema".into()));
                self.nodes
                    .insert(SchemaNode::FrontmatterSchemaDeclaration, schema_range);
                let Some(external) = self.external_schema.take() else {
                    self.error_at(
                        SchemaErrorKind::InvalidFrontmatterSchema,
                        schema_range,
                        "linked frontmatter schema requires a schema file path context",
                    );
                    return None;
                };
                let document_range = self.sources.documents.get(&external.root_source).map_or(
                    schema_range,
                    |source| SourceRange {
                        source: external.root_source,
                        range: TextRange {
                            start: ByteOffset(0),
                            end: ByteOffset(source.text.len()),
                        },
                    },
                );
                let schema = match external.result {
                    Ok(schema) => schema,
                    Err(errors) => {
                        for error in std::iter::once(errors.first).chain(errors.rest) {
                            let range = self.sources.documents.get(&error.source).map_or(
                                schema_range,
                                |source| SourceRange {
                                    source: error.source,
                                    range: TextRange {
                                        start: ByteOffset(0),
                                        end: ByteOffset(source.text.len()),
                                    },
                                },
                            );
                            self.error_at(
                                SchemaErrorKind::InvalidFrontmatterSchema,
                                range,
                                error.message,
                            );
                        }
                        return None;
                    }
                };
                self.nodes
                    .insert(SchemaNode::FrontmatterSchemaDocument, document_range);
                Some(schema)
            }
            Some(RawFrontmatterSchema::Mapping(_mapping)) => {
                self.error_at(
                    SchemaErrorKind::InvalidFrontmatterSchema,
                    self.range(RangeKey::FrontmatterField("schema".into())),
                    "inline frontmatter JSON Schema is not implemented yet; use a linked JSON file",
                );
                return None;
            }
        };
        Some(if required {
            FrontmatterPolicy::Required { schema }
        } else if allow {
            FrontmatterPolicy::Optional { schema }
        } else {
            FrontmatterPolicy::Forbidden { schema }
        })
    }

    fn validate_constraint_lexical_refs(&mut self) {
        let constraints = self.raw_constraints.clone();
        for (scope, values) in constraints {
            for (index, value) in values.iter().enumerate() {
                let range = self.range(RangeKey::Constraint(ConstraintPath {
                    scope: scope.clone(),
                    index: ConstraintIndex(index),
                }));
                let refs = constraint_ref_strings(value);
                let mut seen = HashSet::new();
                for reference in refs {
                    let valid = if reference.starts_with("fm.") {
                        parse_frontmatter_ref(reference).is_some()
                    } else {
                        parse_rule_ref(reference).is_some()
                    };
                    if !valid {
                        self.error_at(
                            SchemaErrorKind::UnresolvedRef,
                            range,
                            format!("invalid ref `{reference}`"),
                        );
                    }
                    if !seen.insert(reference) {
                        self.error_at(
                            SchemaErrorKind::DuplicateRef,
                            range,
                            format!("duplicate ref `{reference}` in one constraint"),
                        );
                    }
                }
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
        for field in ["match_case", "strip_inline_markup", "allow_skipped_levels"] {
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
            for field in ["allow", "required", "strict"] {
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
            for field in ["allow", "required", "strict"] {
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

    /// Builds the general `outline:` form: the canonical `h1`-rule list.
    ///
    /// Outline rules are ordinary rules — `id`, `strict`, any cardinality and
    /// nested constraints all mean what they mean in every other scope — so
    /// the list is built by the same scope builder, at the empty scope the
    /// rules semantically live in. Only their source spelling differs, which
    /// [`Loader::source_key`] maps on range lookup.
    ///
    /// An empty outline is refused rather than accepted as vacuous: an empty
    /// rule list constrains nothing (the outline scope is open, so `h1`
    /// headers would pass unvalidated), while the schema author who writes it
    /// almost certainly means "this document has no `h1`" — which
    /// `title: null` declares, keeping a `sections` list for the real top
    /// level. Accepting `outline: []` would validate nothing and pass every
    /// document silently.
    fn build_outline_scope(
        &mut self,
        entries: Vec<RawRule>,
        root_scope: &ScopePath,
        match_case: bool,
    ) -> Option<Vec<SectionRule>> {
        if entries.is_empty() {
            self.shape_error_at(
                self.range(RangeKey::DocumentField("outline".into())),
                "outline must declare at least one rule; a document with no h1 headers \
                 is declared with `title: null`",
            );
            return None;
        }
        self.build_scope(entries, root_scope, match_case)
    }

    fn build_options(raw: &RawOptions) -> Options {
        Options {
            match_case: raw.match_case.unwrap_or(false),
            strip_inline_markup: raw.strip_inline_markup.unwrap_or(true),
            allow_skipped_levels: raw.allow_skipped_levels.unwrap_or(false),
        }
    }

    fn build_scope(
        &mut self,
        rules: Vec<RawRule>,
        scope: &ScopePath,
        match_case: bool,
    ) -> Option<Vec<SectionRule>> {
        let mut semantic = Vec::with_capacity(rules.len());
        let mut semantic_indices = Vec::with_capacity(rules.len());
        let mut complete = true;
        for (index, raw) in rules.into_iter().enumerate() {
            let rule_path = RulePath {
                scope: scope.clone(),
                index: RuleIndex(index),
            };
            let rule_range = self.range(RangeKey::Rule(rule_path.clone()));
            self.nodes
                .insert(SchemaNode::Rule(rule_path.clone()), rule_range);
            let mut child_scope = scope.clone();
            child_scope.0.push(RuleIndex(index));
            self.raw_constraints
                .insert(child_scope.clone(), raw.constraints);

            let matcher_range = self.range(RangeKey::RuleField(rule_path.clone(), "match".into()));
            let matcher = self.build_matcher(&raw.matcher, match_case, matcher_range);
            let id_range = self.range(RangeKey::RuleField(
                rule_path.clone(),
                if raw.id.is_some() { "id" } else { "match" }.into(),
            ));
            let id = self.build_rule_id(raw.id.as_deref(), matcher.as_ref(), scope, id_range);
            let cardinality_field = if raw.repeat.is_some() {
                "repeat"
            } else if raw.required.is_some() {
                "required"
            } else {
                "allow"
            };
            let outcome_range = self.range(RangeKey::RuleField(
                rule_path.clone(),
                cardinality_field.into(),
            ));
            let outcome = self.build_outcome(
                raw.allow,
                raw.required,
                raw.repeat.as_deref(),
                outcome_range,
            );
            let children = self.build_scope(raw.sections, &child_scope, match_case);
            match (matcher, outcome, children) {
                (Some(matcher), Some(outcome), Some(sections)) => {
                    semantic_indices.push(index);
                    semantic.push(SectionRule {
                        id,
                        matcher,
                        outcome,
                        strict: raw.strict,
                        sections,
                        constraints: Vec::new(),
                    });
                }
                _ => complete = false,
            }
        }

        let mut ids: HashMap<RuleId, usize> = HashMap::new();
        for (&index, rule) in semantic_indices.iter().zip(&semantic) {
            let Some(id) = &rule.id else { continue };
            if let Some(first_index) = ids.get(id).copied() {
                let duplicate_path = RulePath {
                    scope: scope.clone(),
                    index: RuleIndex(index),
                };
                let first_path = RulePath {
                    scope: scope.clone(),
                    index: RuleIndex(first_index),
                };
                self.error_with_related_at(
                    SchemaErrorKind::DuplicateId,
                    self.rule_id_range(&duplicate_path),
                    format!("duplicate rule id `{}` in one scope", id.0),
                    vec![RelatedLocation {
                        range: self.rule_id_range(&first_path),
                        message: format!("first declared by sibling rule {first_index}"),
                    }],
                );
                complete = false;
            } else {
                ids.insert(id.clone(), index);
            }
        }

        complete.then_some(semantic)
    }

    fn build_rule_id(
        &mut self,
        explicit: Option<&str>,
        matcher: Option<&Matcher>,
        scope: &ScopePath,
        range: SourceRange,
    ) -> Option<RuleId> {
        if let Some(id) = explicit {
            if !is_slug(id) {
                self.error_at(
                    SchemaErrorKind::InvalidDocumentShape,
                    range,
                    format!("rule id `{id}` is not a lowercase slug"),
                );
                return None;
            }
            if scope.0.is_empty() && id == "fm" {
                self.error_at(
                    SchemaErrorKind::ReservedId,
                    range,
                    "top-level rule id `fm` is reserved for frontmatter refs",
                );
            }
            return Some(RuleId(id.to_owned()));
        }

        let Matcher::Exact(text) = matcher? else {
            return None;
        };
        let generated = auto_id(&text.0).map(RuleId);
        if scope.0.is_empty() && generated.as_ref().is_some_and(|id| id.0 == "fm") {
            self.error_at(
                SchemaErrorKind::ReservedId,
                range,
                "top-level auto-generated rule id `fm` is reserved for frontmatter refs",
            );
        }
        generated
    }

    fn build_matcher(
        &mut self,
        source: &str,
        match_case: bool,
        range: SourceRange,
    ) -> Option<Matcher> {
        if source == "*" {
            return Some(Matcher::Any);
        }
        if source.starts_with('/') && source.ends_with('/') {
            let Some(body) = source
                .strip_prefix('/')
                .and_then(|body| body.strip_suffix('/'))
            else {
                self.error_at(
                    SchemaErrorKind::InvalidMatcher,
                    range,
                    "a regex matcher needs separate opening and closing `/` delimiters",
                );
                return None;
            };
            let Some(body) = regex_body(body) else {
                self.error_at(
                    SchemaErrorKind::InvalidMatcher,
                    range,
                    format!("regex matcher `{source}` contains an unescaped `/`"),
                );
                return None;
            };
            if let Err(error) = compile_anchored_pattern(&body, match_case, false) {
                self.error_at(
                    SchemaErrorKind::InvalidMatcher,
                    range,
                    format!("invalid regex matcher `{source}`: {error}"),
                );
                return None;
            }
            return Some(Matcher::Regex(RegexPattern(body)));
        }
        if source.contains('*') {
            if let Err(error) = compile_glob_pattern(source, match_case) {
                self.error_at(
                    SchemaErrorKind::InvalidMatcher,
                    range,
                    format!("invalid glob matcher `{source}`: {error}"),
                );
                return None;
            }
            return Some(Matcher::Glob(GlobPattern(source.to_owned())));
        }
        Some(Matcher::Exact(ExactText(source.to_owned())))
    }

    fn build_outcome(
        &mut self,
        allow: bool,
        required: Option<bool>,
        repeat: Option<&str>,
        range: SourceRange,
    ) -> Option<RuleOutcome> {
        if required.is_some() && repeat.is_some() {
            self.error_at(
                SchemaErrorKind::ConflictingCardinality,
                range,
                "required and repeat cannot both be declared",
            );
            return None;
        }
        if !allow && (required.is_some() || repeat.is_some()) {
            self.error_at(
                SchemaErrorKind::ConflictingCardinality,
                range,
                "allow: false cannot be combined with required or repeat",
            );
            return None;
        }
        if !allow {
            return Some(RuleOutcome::Deny);
        }
        let cardinality = match (required, repeat) {
            (Some(true), None) => Cardinality {
                min: 1,
                max: UpperBound::Bounded(1),
            },
            (Some(false), None) => Cardinality {
                min: 0,
                max: UpperBound::Bounded(1),
            },
            (None, Some(repeat)) => match parse_repeat(repeat) {
                Some(cardinality) => cardinality,
                None => {
                    self.error_at(
                        SchemaErrorKind::InvalidRepeat,
                        range,
                        format!("invalid repeat `{repeat}`"),
                    );
                    return None;
                }
            },
            (None, None) => Cardinality {
                min: 0,
                max: UpperBound::Unbounded,
            },
            (Some(_), Some(_)) => return None,
        };
        Some(RuleOutcome::Allow(cardinality))
    }

    fn build_constraint(
        &mut self,
        schema: &Schema,
        scope: &ScopePath,
        value: Value,
        range: SourceRange,
    ) -> Option<Constraint> {
        let Some(mapping) = value.as_object() else {
            self.shape_error_at(range, "constraint must be a single-key object");
            return None;
        };
        if mapping.len() != 1 {
            self.shape_error_at(range, "constraint must contain exactly one keyword");
            return None;
        }
        let (keyword, operand) = mapping.iter().next()?;
        match keyword.as_str() {
            "one_of" | "any_of" | "at_most_one" | "all_or_none" => {
                let refs = self.parse_proposition_list(schema, scope, operand, false, range)?;
                let refs = at_least_two(refs).or_else(|| {
                    self.shape_error_at(range, format!("{keyword} requires at least two refs"));
                    None
                })?;
                Some(match keyword.as_str() {
                    "one_of" => Constraint::OneOf(refs),
                    "any_of" => Constraint::AnyOf(refs),
                    "at_most_one" => Constraint::AtMostOne(refs),
                    "all_or_none" => Constraint::AllOrNone(refs),
                    _ => return None,
                })
            }
            "requires" => self.build_implication(schema, scope, operand, true, range),
            "conflicts" => self.build_implication(schema, scope, operand, false, range),
            "ordered" => self.build_ordered(schema, scope, operand, range),
            _ => {
                self.shape_error_at(range, format!("unknown constraint keyword `{keyword}`"));
                None
            }
        }
    }

    fn build_implication(
        &mut self,
        schema: &Schema,
        scope: &ScopePath,
        operand: &Value,
        requires: bool,
        range: SourceRange,
    ) -> Option<Constraint> {
        let Some(mapping) = operand.as_object() else {
            self.shape_error_at(range, "requires/conflicts operand must be an object");
            return None;
        };
        let consequence_key = if requires { "then" } else { "then_not" };
        if mapping.len() != 2 {
            self.shape_error_at(
                range,
                format!(
                    "{} requires exactly `if` and `{consequence_key}`",
                    if requires { "requires" } else { "conflicts" }
                ),
            );
            return None;
        }
        let Some(condition_value) = mapping.get("if") else {
            self.shape_error_at(range, "requires/conflicts operand is missing `if`");
            return None;
        };
        let Some(consequence_value) = mapping.get(consequence_key) else {
            self.shape_error_at(
                range,
                format!("requires/conflicts operand is missing `{consequence_key}`"),
            );
            return None;
        };
        let condition = self.parse_proposition(schema, scope, condition_value, false, range);
        let consequence_values = scalar_or_sequence(consequence_value);
        if consequence_values.is_empty() {
            self.shape_error_at(
                range,
                format!("`{consequence_key}` must contain at least one ref"),
            );
            return None;
        }
        let mut identities = HashSet::new();
        if let Some((_, identity)) = &condition {
            identities.insert(identity.clone());
        }
        let mut consequences = Vec::new();
        let mut complete = condition.is_some();
        for value in consequence_values {
            if let Some((proposition, identity)) =
                self.parse_proposition(schema, scope, value, false, range)
            {
                if !identities.insert(identity) {
                    self.error_at(
                        SchemaErrorKind::DuplicateRef,
                        range,
                        format!("duplicate ref in `{consequence_key}`"),
                    );
                }
                consequences.push(proposition);
            } else {
                complete = false;
            }
        }
        if !complete {
            return None;
        }
        let (condition, _) = condition?;
        let consequences = non_empty(consequences)?;
        Some(if requires {
            Constraint::Requires {
                condition,
                consequences,
            }
        } else {
            Constraint::Conflicts {
                condition,
                exclusions: consequences,
            }
        })
    }

    fn build_ordered(
        &mut self,
        schema: &Schema,
        scope: &ScopePath,
        operand: &Value,
        range: SourceRange,
    ) -> Option<Constraint> {
        let values = operand.as_array().or_else(|| {
            self.shape_error_at(range, "ordered requires a list of refs");
            None
        })?;
        let mut refs = Vec::new();
        let mut identities = HashSet::new();
        let mut parent_scope: Option<Vec<usize>> = None;
        let mut complete = true;
        for value in values {
            let Some((proposition, identity)) =
                self.parse_proposition(schema, scope, value, true, range)
            else {
                complete = false;
                continue;
            };
            let Proposition::Rule(rule_ref) = proposition else {
                self.error_at(
                    SchemaErrorKind::OrderedScopeMismatch,
                    range,
                    "frontmatter refs cannot be used in ordered",
                );
                continue;
            };
            let ResolvedIdentity::Rule(target) = &identity else {
                continue;
            };
            let Some((_, target_parent)) = target.split_last() else {
                continue;
            };
            let target_parent = target_parent.to_vec();
            if parent_scope
                .as_ref()
                .is_some_and(|existing| existing != &target_parent)
            {
                self.error_at(
                    SchemaErrorKind::OrderedScopeMismatch,
                    range,
                    "all ordered refs must resolve in the same scope",
                );
            } else {
                parent_scope = Some(target_parent);
            }
            if !identities.insert(identity) {
                self.error_at(
                    SchemaErrorKind::DuplicateRef,
                    range,
                    "duplicate ref in ordered",
                );
            }
            refs.push(rule_ref);
        }
        if !complete {
            return None;
        }
        let refs = at_least_two(refs).or_else(|| {
            self.shape_error_at(range, "ordered requires at least two refs");
            None
        })?;
        Some(Constraint::Ordered(refs))
    }

    fn parse_proposition_list(
        &mut self,
        schema: &Schema,
        scope: &ScopePath,
        operand: &Value,
        ordered: bool,
        range: SourceRange,
    ) -> Option<Vec<Proposition>> {
        let values = operand.as_array().or_else(|| {
            self.shape_error_at(range, "constraint operand must be a list of refs");
            None
        })?;
        let mut identities = HashSet::new();
        let mut result = Vec::new();
        let mut complete = true;
        for value in values {
            if let Some((proposition, identity)) =
                self.parse_proposition(schema, scope, value, ordered, range)
            {
                if !identities.insert(identity) {
                    self.error_at(
                        SchemaErrorKind::DuplicateRef,
                        range,
                        "constraint contains a duplicate ref",
                    );
                }
                result.push(proposition);
            } else {
                complete = false;
            }
        }
        complete.then_some(result)
    }

    fn parse_proposition(
        &mut self,
        schema: &Schema,
        scope: &ScopePath,
        value: &Value,
        ordered: bool,
        range: SourceRange,
    ) -> Option<(Proposition, ResolvedIdentity)> {
        let Some(source) = value.as_str() else {
            self.shape_error_at(range, "constraint refs must be strings");
            return None;
        };
        if source.starts_with("fm.") {
            let Some(reference) = parse_frontmatter_ref(source) else {
                self.error_at(
                    SchemaErrorKind::UnresolvedRef,
                    range,
                    format!("invalid frontmatter ref `{source}`"),
                );
                return None;
            };
            let identity = frontmatter_identity(&reference, schema.options.match_case);
            return Some((
                Proposition::Frontmatter(reference.clone()),
                ResolvedIdentity::Frontmatter(identity),
            ));
        }

        let Some(reference) = parse_rule_ref(source) else {
            self.error_at(
                SchemaErrorKind::UnresolvedRef,
                range,
                format!("invalid or unresolved ref `{source}`"),
            );
            return None;
        };
        let Some(resolved) = resolve_ref(schema, scope, &reference) else {
            self.error_at(
                SchemaErrorKind::UnresolvedRef,
                range,
                format!("unresolved ref `{source}`"),
            );
            return None;
        };
        if resolved.denied {
            self.error_at(
                SchemaErrorKind::ForbiddenRef,
                range,
                format!("ref `{source}` passes through or targets an allow: false rule"),
            );
        }
        if ordered && resolved.repeated_non_final {
            self.error_at(
                SchemaErrorKind::OrderedScopeMismatch,
                range,
                format!("ordered ref `{source}` passes through a repeatable ancestor"),
            );
        }
        Some((
            Proposition::Rule(reference),
            ResolvedIdentity::Rule(resolved.structural_path),
        ))
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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum ResolvedIdentity {
    Rule(Vec<usize>),
    Frontmatter(FrontmatterRef),
}

struct ResolvedRule {
    structural_path: Vec<usize>,
    denied: bool,
    repeated_non_final: bool,
}

fn resolve_ref(schema: &Schema, scope: &ScopePath, reference: &RuleRef) -> Option<ResolvedRule> {
    // Both anchors resolve from the addressed root: the outline (`h1`) scope
    // for the general form, the `sections` scope for sugar — so a sugar
    // schema's `$.` references keep meaning what they always meant, while in
    // an `outline` schema `$` names the `h1` rules.
    let (mut rules, mut structural_path) = match reference.anchor {
        RefAnchor::SchemaRoot => (schema.addressed_root_rules(), Vec::new()),
        RefAnchor::CurrentScope => (
            rules_at_scope(schema, scope)?,
            scope.0.iter().map(|index| index.0).collect(),
        ),
    };
    let mut denied = false;
    let mut repeated_non_final = false;
    let segment_count = reference.path.rest.len() + 1;
    for (position, id) in reference.path.iter().enumerate() {
        let (index, rule) = rules
            .iter()
            .enumerate()
            .find(|(_, rule)| rule.id.as_ref() == Some(id))?;
        structural_path.push(index);
        denied |= matches!(rule.outcome, RuleOutcome::Deny);
        if position + 1 < segment_count {
            repeated_non_final |= !matches!(
                rule.outcome,
                RuleOutcome::Allow(Cardinality {
                    max: UpperBound::Bounded(0 | 1),
                    ..
                })
            );
        }
        rules = &rule.sections;
    }
    Some(ResolvedRule {
        structural_path,
        denied,
        repeated_non_final,
    })
}

fn rules_at_scope<'a>(schema: &'a Schema, scope: &ScopePath) -> Option<&'a [SectionRule]> {
    let mut rules = schema.addressed_root_rules();
    for index in &scope.0 {
        let rule = rules.get(index.0)?;
        rules = &rule.sections;
    }
    Some(rules)
}

/// The constraint list a public scope path names, in the built schema.
///
/// The empty scope names what the source's top level spelled: the outline
/// scope for the general form ([`Schema::constraints`]), the `sections` scope
/// for sugar — which is the synthesized rule's child scope, so its top-level
/// constraints live on that rule.
fn constraints_mut<'a>(
    schema: &'a mut Schema,
    scope: &ScopePath,
) -> Option<&'a mut Vec<Constraint>> {
    if schema.is_sugar() {
        let rule = schema.outline.first_mut()?;
        if scope.0.is_empty() {
            return Some(&mut rule.constraints);
        }
        constraints_in_rules_mut(&mut rule.sections, &scope.0)
    } else {
        if scope.0.is_empty() {
            return Some(&mut schema.constraints);
        }
        constraints_in_rules_mut(&mut schema.outline, &scope.0)
    }
}

fn constraints_in_rules_mut<'a>(
    rules: &'a mut [SectionRule],
    path: &[RuleIndex],
) -> Option<&'a mut Vec<Constraint>> {
    let (index, rest) = path.split_first()?;
    let rule = rules.get_mut(index.0)?;
    if rest.is_empty() {
        Some(&mut rule.constraints)
    } else {
        constraints_in_rules_mut(&mut rule.sections, rest)
    }
}

fn primary_sources(text: Arc<str>, label: Option<SourceLabel>) -> SchemaSources {
    SchemaSources {
        primary: SourceId(0),
        documents: BTreeMap::from([(SourceId(0), SchemaSource { label, text })]),
    }
}

fn is_slug(value: &str) -> bool {
    let mut previous_hyphen = true;
    for byte in value.bytes() {
        match byte {
            b'a'..=b'z' | b'0'..=b'9' => previous_hyphen = false,
            b'-' if !previous_hyphen => previous_hyphen = true,
            _ => return false,
        }
    }
    !value.is_empty() && !previous_hyphen
}

fn auto_id(value: &str) -> Option<String> {
    let mut result = String::new();
    let mut separator_pending = false;
    for character in value.nfkd().flat_map(char::to_lowercase) {
        if character.is_ascii_lowercase() || character.is_ascii_digit() {
            if separator_pending && !result.is_empty() {
                result.push('-');
            }
            result.push(character);
            separator_pending = false;
        } else {
            separator_pending = true;
        }
    }
    (!result.is_empty()).then_some(result)
}

fn regex_body(source: &str) -> Option<String> {
    let mut result = String::with_capacity(source.len());
    let mut characters = source.chars();
    while let Some(character) = characters.next() {
        if character == '/' {
            return None;
        }
        if character != '\\' {
            result.push(character);
            continue;
        }
        match characters.next() {
            Some('/') => result.push('/'),
            Some(next) => {
                result.push('\\');
                result.push(next);
            }
            None => result.push('\\'),
        }
    }
    Some(result)
}

fn parse_repeat(source: &str) -> Option<Cardinality> {
    let (min, max) = source.split_once("..")?;
    if min.is_empty() || max.is_empty() || max.contains("..") || !valid_decimal(min) {
        return None;
    }
    let min = min.parse::<u32>().ok()?;
    let max = if max == "n" {
        UpperBound::Unbounded
    } else {
        if !valid_decimal(max) {
            return None;
        }
        let max = max.parse::<u32>().ok()?;
        if max < min || max == 0 {
            return None;
        }
        UpperBound::Bounded(max)
    };
    Some(Cardinality { min, max })
}

fn valid_decimal(value: &str) -> bool {
    value == "0"
        || value
            .strip_prefix(|character: char| ('1'..='9').contains(&character))
            .is_some_and(|rest| rest.bytes().all(|byte| byte.is_ascii_digit()))
}

fn parse_rule_ref(source: &str) -> Option<RuleRef> {
    let (anchor, path) = if let Some(path) = source.strip_prefix("$.") {
        (RefAnchor::SchemaRoot, path)
    } else {
        (RefAnchor::CurrentScope, source)
    };
    let mut segments = path.split('.');
    let first = segments.next()?;
    if !is_slug(first) {
        return None;
    }
    let rest = segments
        .map(|segment| is_slug(segment).then(|| RuleId(segment.to_owned())))
        .collect::<Option<Vec<_>>>()?;
    Some(RuleRef {
        anchor,
        path: NonEmpty {
            first: RuleId(first.to_owned()),
            rest,
        },
    })
}

fn parse_frontmatter_ref(source: &str) -> Option<FrontmatterRef> {
    let body = source.strip_prefix("fm.")?;
    let (path, equals) = match body.split_once('=') {
        Some((path, literal)) => (path, Some(parse_frontmatter_scalar(literal))),
        None => (body, None),
    };
    let mut keys = path.split('.');
    let first = keys.next()?;
    if first.is_empty() {
        return None;
    }
    let rest = keys
        .map(|key| (!key.is_empty()).then(|| FrontmatterKey(key.to_owned())))
        .collect::<Option<Vec<_>>>()?;
    Some(FrontmatterRef {
        path: NonEmpty {
            first: FrontmatterKey(first.to_owned()),
            rest,
        },
        equals,
    })
}

fn frontmatter_identity(reference: &FrontmatterRef, match_case: bool) -> FrontmatterRef {
    let mut identity = reference.clone();
    if !match_case {
        if let Some(FrontmatterScalar::String(value)) = &mut identity.equals {
            *value = crate::case_fold::simple_fold(value).collect();
        }
    }
    identity
}

pub(crate) fn parse_frontmatter_scalar(source: &str) -> FrontmatterScalar {
    match source {
        "" | "~" | "null" | "Null" | "NULL" => FrontmatterScalar::Null,
        "true" | "True" | "TRUE" => FrontmatterScalar::Boolean(true),
        "false" | "False" | "FALSE" => FrontmatterScalar::Boolean(false),
        _ => {
            if let Some(integer) = canonical_integer(source) {
                FrontmatterScalar::Integer(CanonicalInteger(integer))
            } else if let Some(float) = canonical_float(source) {
                FrontmatterScalar::Float(CanonicalFloat(float))
            } else {
                FrontmatterScalar::String(source.to_owned())
            }
        }
    }
}

fn canonical_integer(source: &str) -> Option<String> {
    let (negative, unsigned) = strip_sign(source);
    let (base, digits) = if let Some(digits) = unsigned.strip_prefix("0o") {
        (8_u8, digits)
    } else if let Some(digits) = unsigned.strip_prefix("0x") {
        (16, digits)
    } else {
        (10, unsigned)
    };
    if digits.is_empty() {
        return None;
    }
    let value = BigUint::parse_bytes(digits.as_bytes(), u32::from(base))?;
    if value == BigUint::from(0_u8) {
        return Some("0".into());
    }
    Some(format!("{}{value}", if negative { "-" } else { "" }))
}

pub(crate) fn canonical_float(source: &str) -> Option<String> {
    let (negative, unsigned) = strip_sign(source);
    if matches!(unsigned, ".inf" | ".Inf" | ".INF") {
        return Some(if negative { "-inf" } else { "inf" }.into());
    }
    if matches!(unsigned, ".nan" | ".NaN" | ".NAN") {
        return (source == unsigned).then(|| "nan".into());
    }
    let (mantissa, exponent) = unsigned
        .split_once(['e', 'E'])
        .map_or((unsigned, "0"), |parts| parts);
    let has_float_marker = mantissa.contains('.') || unsigned.contains(['e', 'E']);
    if !has_float_marker {
        return None;
    }
    let exponent = exponent.parse::<BigInt>().ok()?;
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    if whole.is_empty() && fraction.is_empty() {
        return None;
    }
    if !whole.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let digits = format!("{whole}{fraction}");
    let trimmed_leading = digits.trim_start_matches('0');
    if trimmed_leading.is_empty() {
        return Some("0e0".into());
    }
    let trailing = trimmed_leading.len() - trimmed_leading.trim_end_matches('0').len();
    let coefficient = trimmed_leading.trim_end_matches('0');
    let adjusted = exponent - BigInt::from(fraction.len()) + BigInt::from(trailing);
    Some(format!(
        "{}{coefficient}e{adjusted}",
        if negative { "-" } else { "" }
    ))
}

fn strip_sign(source: &str) -> (bool, &str) {
    if let Some(unsigned) = source.strip_prefix('-') {
        (true, unsigned)
    } else if let Some(unsigned) = source.strip_prefix('+') {
        (false, unsigned)
    } else {
        (false, source)
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

fn scalar_or_sequence(value: &Value) -> Vec<&Value> {
    value
        .as_array()
        .map_or_else(|| vec![value], |values| values.iter().collect())
}

fn constraint_ref_strings(value: &Value) -> Vec<&str> {
    let Some(mapping) = value.as_object() else {
        return Vec::new();
    };
    let Some((keyword, operand)) = mapping.iter().next() else {
        return Vec::new();
    };
    match keyword.as_str() {
        "one_of" | "any_of" | "at_most_one" | "all_or_none" | "ordered" => operand
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect(),
        "requires" | "conflicts" => {
            let Some(implication) = operand.as_object() else {
                return Vec::new();
            };
            let consequence = if keyword == "requires" {
                "then"
            } else {
                "then_not"
            };
            let mut result = implication
                .get("if")
                .and_then(Value::as_str)
                .into_iter()
                .collect::<Vec<_>>();
            if let Some(value) = implication.get(consequence) {
                result.extend(
                    scalar_or_sequence(value)
                        .into_iter()
                        .filter_map(Value::as_str),
                );
            }
            result
        }
        _ => Vec::new(),
    }
}

fn non_empty<T>(mut values: Vec<T>) -> Option<NonEmpty<T>> {
    if values.is_empty() {
        return None;
    }
    let first = values.remove(0);
    Some(NonEmpty {
        first,
        rest: values,
    })
}

fn at_least_two<T>(mut values: Vec<T>) -> Option<AtLeastTwo<T>> {
    if values.len() < 2 {
        return None;
    }
    let first = values.remove(0);
    let second = values.remove(0);
    Some(AtLeastTwo {
        first,
        second,
        rest: values,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn valid(source: &str) -> Schema {
        match load_schema(source) {
            Ok(loaded) => loaded.schema,
            Err(invalid) => panic!("unexpected errors: {:#?}", invalid.errors),
        }
    }

    fn error_kinds(source: &str) -> Vec<SchemaErrorKind> {
        match load_schema(source) {
            Ok(loaded) => panic!("unexpected valid schema: {:#?}", loaded.schema),
            Err(invalid) => invalid.errors.iter().map(|error| error.kind).collect(),
        }
    }

    fn invalid(source: &str) -> InvalidSchema {
        match load_schema(source) {
            Ok(loaded) => panic!("unexpected valid schema: {:#?}", loaded.schema),
            Err(invalid) => invalid,
        }
    }

    fn source_slice(source: &str, range: SourceRange) -> &str {
        source
            .get(range.range.start.0..range.range.end.0)
            .unwrap_or("<invalid range>")
    }

    #[test]
    fn yaml_syntax_error_ranges_convert_character_columns_to_bytes() {
        for source in [
            "version: 1\ntitle: å: bad\nsections: []\n",
            "version: 1\ntitle: a: bad\nsections: []\n",
            "version: 1\rtitle: å: bad\rsections: []\r",
        ] {
            let invalid = load_schema(source).expect_err("schema has invalid YAML");
            let error = &invalid.errors.first;
            let expected_start = source
                .find(": bad")
                .unwrap_or_else(|| panic!("test source contains the bad colon"));

            assert_eq!(error.kind, SchemaErrorKind::Syntax);
            assert_eq!(error.range.range.start, ByteOffset(expected_start));
            assert_eq!(source_slice(source, error.range), ":");
            assert!(source.is_char_boundary(error.range.range.start.0));
            assert!(source.is_char_boundary(error.range.range.end.0));
        }
    }

    #[test]
    fn a_second_schema_document_is_refused_at_its_own_start_marker() {
        // The refusal lands on the second `---` before any of that document's
        // content is read — raw `next_event` does not clear the anchor table
        // between documents — and it carries the marker's real span, where the
        // serde-era engine could only anchor the whole document. The `---`
        // sits in the first column, so this doubles as the pin that a
        // first-column range survives the character-to-byte conversion.
        for (source, line) in [
            (
                "version: 1\nsections: []\n---\nversion: 1\nsections: []\n",
                3,
            ),
            ("version: 1\nsections: []\n...\n---\nsections: []\n", 4),
        ] {
            let invalid = invalid(source);
            assert_eq!(invalid.errors.first.kind, SchemaErrorKind::Syntax);
            assert_eq!(
                invalid.errors.first.message,
                format!(
                    "invalid YAML: a second document opens at line {line} column 1; \
                     a schema is a single YAML document"
                )
            );
            assert_eq!(source_slice(source, invalid.errors.first.range), "---");
            let start = invalid.errors.first.range.range.start.0;
            assert!(
                start == 0 || source.as_bytes()[start - 1] == b'\n',
                "the `---` anchor must sit in the first column"
            );
        }

        // A `...` that closes the only document opens nothing.
        assert_eq!(
            valid("version: 1\nsections: []\n...\n")
                .addressed_root_rules()
                .len(),
            0
        );
    }

    #[test]
    fn a_merge_key_is_an_ordinary_schema_field() {
        // `<<` belongs to YAML's optional merge type, not to the core schema,
        // and no parser this crate reads applies it. A schema author who writes
        // one therefore gets an unknown field named `<<` rather than the fields
        // of the mapping they aliased. Pinned rather than fixed: honoring merges
        // would make schemas that are rejected today start loading, which needs
        // a specification first.
        let source =
            "version: 1\nbase: &b\n  strip_inline_markup: true\noptions:\n  <<: *b\nsections: []\n";
        let invalid = invalid(source);
        let reported = invalid
            .errors
            .iter()
            .map(|error| (error.kind, error.message.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(
            reported,
            vec![
                (
                    SchemaErrorKind::InvalidDocumentShape,
                    "unknown field `base`"
                ),
                (SchemaErrorKind::InvalidDocumentShape, "unknown field `<<`"),
            ],
        );
    }

    /// A schema of `rules` rules, each the sole child of the one above it.
    ///
    /// Nesting is what a schema spends YAML depth on, two levels per rule: the
    /// `sections` sequence and the rule mapping it holds.
    fn nested_rule_schema(rules: usize) -> String {
        let mut source = String::from("version: 1\n");
        for rule in 0..rules {
            let indent = "  ".repeat(rule * 2);
            source.push_str(&format!(
                "{indent}sections:\n{indent}  - match: \"h{rule}\"\n"
            ));
        }
        source
    }

    #[test]
    fn schema_nesting_is_bounded() {
        // The reader charges the depth limit as its own recursion descends, so
        // a schema nesting past it is refused at the exact node that would
        // overrun the stack, before that node is built. Two levels per rule
        // plus the document's own mapping puts the deepest schema that fits at
        // 63 rules, and the first that does not at 64 — the boundary the
        // serde-era engine's identical limit drew.
        let schema = valid(&nested_rule_schema(63));
        assert_eq!(schema.addressed_root_rules().len(), 1);

        for rules in [64, 5_000] {
            let source = nested_rule_schema(rules);
            let invalid = invalid(&source);
            assert_eq!(invalid.errors.first.kind, SchemaErrorKind::Syntax);
            assert_eq!(
                invalid.errors.first.message,
                "invalid YAML: nesting exceeds the depth limit"
            );
            // The refusal is anchored where the 64th rule's mapping opens —
            // its first key — however much deeper the document goes on, since
            // nothing past the refusal is read.
            let overrun = source
                .match_indices("match")
                .nth(63)
                .map(|(offset, _)| offset)
                .expect("the fixture spells one `match` per rule");
            assert_eq!(invalid.errors.first.range.range.start, ByteOffset(overrun));
        }
    }

    /// A schema whose `constraints` entries chain anchors, each wrapping an
    /// alias to the entry above it in one more sequence.
    ///
    /// Every entry is one flow sequence in the source, so no event stream ever
    /// shows more than three open collections, while the tree the reader
    /// builds reaches `links` levels below the `constraints` sequence once
    /// the aliases are expanded.
    fn alias_deepened_schema(links: usize) -> String {
        let mut source =
            String::from("version: 1\nsections:\n  - match: Title\nconstraints:\n  - &x0 [1]\n");
        for line in 1..links {
            source.push_str(&format!("  - &x{line} [*x{}]\n", line - 1));
        }
        source
    }

    #[test]
    fn alias_expanded_schema_nesting_is_bounded_only_by_the_readers_own_limit() {
        // Depth an alias splices in is depth no event stream shows: an alias
        // is one event however deep the value it names. The reader therefore
        // charges an alias the whole depth of the node it copies — before the
        // clone — exactly as the frontmatter reader does. This guard used to
        // live inside `yaml_serde`; the frontmatter path once dropped that
        // dependency without replacing what it supplied (ec565c6, 25 GB of
        // RSS, two commits to recover), and this pin is what makes the same
        // loss loud on the schema path. The 127-link fixture is shallow
        // enough to build harmlessly were the guard gone, at which point the
        // loader would walk it to a constraint-shape complaint and the
        // message assertions below would fail plainly.
        //
        // The boundary from both sides: at 126 links the expanded tree fills
        // the limit of 128 exactly (root mapping, `constraints` sequence, 126
        // chained levels) and is built — proven by the loader getting past
        // parsing to reject the entries as constraints — and one more link
        // flips the outcome to an ordinary syntax diagnostic anchored at the
        // alias that splices the overrun in, not a crash. The boundary is the
        // one `yaml_serde`'s recursion limit drew before the port.
        let at_limit = alias_deepened_schema(126);
        let built = invalid(&at_limit);
        assert_eq!(
            built.errors.first.kind,
            SchemaErrorKind::InvalidDocumentShape
        );
        assert_eq!(
            built.errors.first.message,
            "constraint must be a single-key object"
        );

        for links in [127, 2_000] {
            let source = alias_deepened_schema(links);
            let refused = invalid(&source);
            assert_eq!(refused.errors.first.kind, SchemaErrorKind::Syntax);
            assert_eq!(
                refused.errors.first.message,
                "invalid YAML: nesting exceeds the depth limit"
            );
            // The reported position is the alias whose expansion would pass
            // the limit, however many further links the chain spells.
            assert_eq!(source_slice(&source, refused.errors.first.range), "*x125");
            // The same engine serves linked-schema discovery, which reports
            // the refused document as declaring no linked schema.
            assert_eq!(linked_frontmatter_schema_path(&source), None);
        }
    }

    /// A schema whose every `x` entry aliases the one above it four times.
    ///
    /// The `depth + 1` short lines this writes name `4 ^ (depth + 1)` leaf
    /// scalars between them; nothing nests deeply, so only the node budget
    /// stops it — the same shape the frontmatter bomb fixtures pin.
    fn alias_bomb_schema(depth: usize) -> String {
        let mut bomb = String::from("version: 1\nsections: []\nx0: &x0 [1,1,1,1]\n");
        for level in 1..=depth {
            let alias = format!("*x{}", level - 1);
            bomb.push_str(&format!(
                "x{level}: &x{level} [{alias},{alias},{alias},{alias}]\n"
            ));
        }
        bomb
    }

    #[test]
    fn schema_alias_expansion_is_bounded_by_the_node_budget() {
        // The wall clock is part of the assertion: a loader that expands the
        // bomb before refusing it returns the right verdict a gigabyte too
        // late, which is the regression the budget exists to prevent.
        for depth in [9, 12, 15] {
            let bomb = alias_bomb_schema(depth);
            let started = std::time::Instant::now();
            let refused = invalid(&bomb);
            let elapsed = started.elapsed();
            assert_eq!(refused.errors.first.kind, SchemaErrorKind::Syntax);
            assert_eq!(
                refused.errors.first.message,
                "invalid YAML: alias expansion exceeds the document's size limit"
            );
            // The refusal lands on the alias whose copy overruns the budget.
            assert!(source_slice(&bomb, refused.errors.first.range).starts_with("*x"));
            assert!(
                elapsed < std::time::Duration::from_secs(1),
                "an alias bomb at depth {depth} took {elapsed:?}, \
                 so it was expanded before being refused"
            );
        }

        // Ordinary reuse stays far under the budget: the aliased matcher is
        // copied once and the schema loads.
        let schema =
            valid("version: 1\nsections:\n  - match: &m Intro\n  - id: other\n    match: *m\n");
        assert_eq!(schema.addressed_root_rules().len(), 2);
    }

    #[test]
    fn non_standard_tags_are_rejected_anywhere_in_a_schema_document() {
        // Judgment call: a tag outside the yaml.org namespace has no meaning a
        // schema could use, and the serde-era engine rejected such documents
        // too. The refusal is uniform — scalar, collection, or the document's
        // own root — where the old engine incidentally accepted a root tag.
        let scalar = invalid("version: 1\ntitle: !custom Doc\nsections: []\n");
        assert_eq!(scalar.errors.first.kind, SchemaErrorKind::Syntax);
        assert_eq!(
            scalar.errors.first.message,
            "invalid YAML: non-standard tag `!custom`"
        );
        assert_eq!(
            source_slice(
                "version: 1\ntitle: !custom Doc\nsections: []\n",
                scalar.errors.first.range
            ),
            "Doc"
        );

        let root = invalid("--- !custom\nversion: 1\nsections: []\n");
        assert_eq!(root.errors.first.kind, SchemaErrorKind::Syntax);
        assert_eq!(
            root.errors.first.message,
            "invalid YAML: non-standard tag `!custom`"
        );

        // Core-schema tags keep their meaning.
        let schema = valid("version: !!int 1\ntitle: !!str Doc\nsections: []\n");
        assert!(matches!(
            schema.outline.first().map(|rule| &rule.matcher),
            Some(Matcher::Exact(_))
        ));
    }

    #[test]
    fn a_standard_tag_on_a_schema_collection_must_name_the_collection_kind() {
        // This verdict changed in the saphyr port: the serde-era engine
        // ignored a mismatched standard tag on a schema collection, so
        // `sections: !!map` over a block sequence loaded as if untagged. The
        // shared container-tag check now refuses the mismatch — the same rule
        // the frontmatter path applies — and this test records the new
        // behaviour deliberately. A tag that names the collection's own kind
        // keeps loading on both engines.
        let schema = valid("version: 1\nsections: !!seq\n  - match: A\n");
        assert_eq!(schema.addressed_root_rules().len(), 1);

        let source = "version: 1\nsections: !!map\n  - match: A\n";
        let refused = invalid(source);
        assert_eq!(refused.errors.first.kind, SchemaErrorKind::Syntax);
        assert_eq!(
            refused.errors.first.message,
            "invalid YAML: invalid tag for a YAML seq"
        );
        // The refusal anchors where the sequence starts: the first entry's
        // `-` at 3:3.
        assert_eq!(source_slice(source, refused.errors.first.range), "-");
        assert_eq!(refused.errors.first.range.range.start, ByteOffset(29));
    }

    #[test]
    fn an_oversized_version_is_a_shape_error_at_the_value() {
        // The engine preserves a number's exact spelling, so an integer of any
        // magnitude parses; one that does not fit the schema's own 64-bit
        // field is now a shape complaint against the value — the serde-era
        // engine refused the whole parse as a syntax error instead.
        let source = "version: 99999999999999999999999999\nsections: []\n";
        let invalid = invalid(source);
        assert_eq!(
            invalid.errors.first.kind,
            SchemaErrorKind::InvalidDocumentShape
        );
        assert_eq!(
            invalid.errors.first.message,
            "version must be an integer that fits in 64 bits and cannot be null"
        );
        assert_eq!(
            source_slice(source, invalid.errors.first.range),
            "99999999999999999999999999"
        );
    }

    #[test]
    fn one_leading_byte_order_mark_is_removed_before_parsing() {
        // Left in place, the mark becomes the first character of the first
        // key, and the loader rejects the document naming a `version` field
        // the author cannot see is misspelled. Exactly one is removed — the
        // same rule the frontmatter path applies — and every reported range
        // counts it back in, so a second mark stays visible.
        let schema = valid("\u{feff}version: 1\nsections: []\n");
        assert_eq!(schema.version, SchemaVersion::V1);

        let source = "\u{feff}\u{feff}version: 1\nsections: []\n";
        let doubled = invalid(source);
        assert!(doubled
            .errors
            .iter()
            .any(|error| error.message == "unknown field `\u{feff}version`"));
    }

    #[test]
    fn duplicate_keys_are_rejected_on_resolved_text_at_the_duplicate() {
        // `a` and `"a"` are one key however differently they are spelled; the
        // refusal names the key and anchors at the duplicate occurrence.
        for source in [
            "version: 1\nversion: 2\nsections: []\n",
            "version: 1\n\"version\": 2\nsections: []\n",
        ] {
            let refused = invalid(source);
            assert_eq!(refused.errors.first.kind, SchemaErrorKind::Syntax);
            assert_eq!(
                refused.errors.first.message,
                "invalid YAML: duplicate mapping key `version`"
            );
            assert!(refused.errors.first.range.range.start >= ByteOffset(11));
        }
    }

    #[test]
    fn applies_defaults_and_normalizes_rules() {
        let schema = valid(
            r#"
version: 1
sections:
  - match: API Reference
    required: true
  - id: api
    match: "/API: .+/"
    repeat: 0..n
  - match: "*"
    allow: false
"#,
        );
        assert!(!schema.options.match_case);
        assert!(schema.options.strip_inline_markup);
        assert!(!schema.options.allow_skipped_levels);
        let rules = schema.addressed_root_rules();
        assert_eq!(rules[0].id, Some(RuleId("api-reference".into())));
        assert_eq!(
            rules[0].outcome,
            RuleOutcome::Allow(Cardinality {
                min: 1,
                max: UpperBound::Bounded(1)
            })
        );
        assert!(matches!(rules[2].outcome, RuleOutcome::Deny));
    }

    #[test]
    fn classifies_matcher_forms_and_unescapes_regex_delimiter() {
        let schema = valid(
            r#"
version: 1
sections:
  - match: exact
  - match: prefix*suffix
  - match: "*"
  - match: /a\/b/
"#,
        );
        let rules = schema.addressed_root_rules();
        assert!(matches!(rules[0].matcher, Matcher::Exact(_)));
        assert!(matches!(rules[1].matcher, Matcher::Glob(_)));
        assert_eq!(rules[2].matcher, Matcher::Any);
        assert_eq!(rules[3].matcher, Matcher::Regex(RegexPattern("a/b".into())));
    }

    #[test]
    fn rejects_invalid_regex_and_repeat_while_collecting_errors() {
        let kinds = error_kinds(
            r#"
version: 1
sections:
  - match: /(?=lookaround)/
    repeat: 01..2
  - match: ok
    allow: false
    required: true
"#,
        );
        assert!(kinds.contains(&SchemaErrorKind::InvalidMatcher));
        assert!(kinds.contains(&SchemaErrorKind::InvalidRepeat));
        assert!(kinds.contains(&SchemaErrorKind::ConflictingCardinality));
    }

    #[test]
    fn rejects_a_single_regex_delimiter_without_panicking() {
        let kinds = error_kinds(
            r#"
version: 1
sections:
  - match: "/"
"#,
        );
        assert_eq!(kinds, vec![SchemaErrorKind::InvalidMatcher]);
    }

    #[test]
    fn regex_load_validation_uses_the_normalized_match_case_setting() {
        let body = "[a-z]{100000}";
        let case_insensitive = format!("version: 1\nsections:\n  - match: \"/{body}/\"\n");
        let invalid = load_schema(&case_insensitive)
            .expect_err("case-insensitive compiled regex exceeds the size limit");
        assert_eq!(invalid.errors.first.kind, SchemaErrorKind::InvalidMatcher);

        let case_sensitive = format!(
            "version: 1\noptions:\n  match_case: true\nsections:\n  - match: \"/{body}/\"\n"
        );
        let loaded = load_schema(&case_sensitive).expect("the same regex fits when case-sensitive");
        crate::PreparedValidator::new(&loaded.schema)
            .expect("loader and validator use identical case-sensitive settings");
    }

    #[test]
    fn oversized_glob_is_invalid_at_its_matcher_range_and_errors_are_collected() {
        let glob = format!("{}*", "a".repeat(200_000));
        let source = format!("version: 1\nsections:\n  - match: {glob}\n    repeat: 01..2\n");
        let invalid = load_schema(&source).expect_err("oversized glob must fail during loading");
        let errors = invalid.errors.iter().collect::<Vec<_>>();

        assert_eq!(errors.len(), 2);
        assert_eq!(errors[0].kind, SchemaErrorKind::InvalidMatcher);
        assert_eq!(source_slice(&source, errors[0].range), glob);
        assert_eq!(errors[1].kind, SchemaErrorKind::InvalidRepeat);

        let case_sensitive =
            format!("version: 1\noptions:\n  match_case: true\nsections:\n  - match: {glob}\n");
        let loaded = load_schema(&case_sensitive)
            .expect("the same glob fits when matching case-sensitively");
        crate::PreparedValidator::new(&loaded.schema)
            .expect("loader and validator use identical case-sensitive glob settings");
    }

    #[test]
    fn detects_auto_id_collisions_per_scope() {
        let kinds = error_kinds(
            r#"
version: 1
sections:
  - match: API
  - id: api
    match: Something else
"#,
        );
        assert!(kinds.contains(&SchemaErrorKind::DuplicateId));
    }

    #[test]
    fn rejects_auto_generated_reserved_fm_id() {
        let kinds = error_kinds(
            r#"
version: 1
sections:
  - match: fm
"#,
        );
        assert_eq!(kinds, vec![SchemaErrorKind::ReservedId]);
    }

    /// `root_level` was removed from the format: the title is always the `h1`
    /// and `sections` always describes `h2`. A schema still declaring it is
    /// rejected as an unknown option rather than silently ignored.
    #[test]
    fn rejects_the_removed_root_level_option() {
        let source = "version: 1\noptions:\n  root_level: 3\nsections: []\n";
        let invalid = invalid(source);
        let messages = invalid
            .errors
            .iter()
            .map(|error| (error.kind, error.message.clone()))
            .collect::<Vec<_>>();
        assert_eq!(
            messages,
            vec![(
                SchemaErrorKind::InvalidDocumentShape,
                "unknown field `root_level`".to_owned()
            )]
        );
    }

    #[test]
    fn rejects_every_explicit_null_typed_field_and_collects_them() {
        // `title: null` is the one legal null: it declares a document with no
        // h1, so only the four other nulls are rejected.
        let source = r#"version: 1
title: null
options:
  match_case: null
sections:
  - id: null
    match: valid
    required: null
    repeat: null
"#;
        let invalid = invalid(source);
        let errors = invalid.errors.iter().collect::<Vec<_>>();
        assert_eq!(errors.len(), 4);
        assert!(errors
            .iter()
            .all(|error| error.kind == SchemaErrorKind::InvalidDocumentShape
                && source_slice(source, error.range) == "null"));
        let mut actual = errors
            .iter()
            .map(|error| error.range.range.start.0)
            .collect::<Vec<_>>();
        actual.sort_unstable();
        let expected = source
            .match_indices("null")
            .map(|(offset, _)| offset)
            .skip(1)
            .collect::<Vec<_>>();
        assert_eq!(actual, expected);
    }

    #[test]
    fn title_null_declares_a_document_without_h1() {
        let source = "version: 1\ntitle: null\nsections:\n  - match: Overview\n";
        let loaded = load_schema(source).expect("title: null loads");
        // The declaration desugars to a denied any-text h1 rule carrying the
        // `sections` scope: a present h1 is not-allowed, and the sections
        // describe the document's top-level h2s.
        let rule = &loaded.schema.outline[0];
        assert_eq!(rule.matcher, Matcher::Any);
        assert_eq!(rule.outcome, RuleOutcome::Deny);
        assert_eq!(rule.sections.len(), 1);
        assert_eq!(loaded.schema.outline_provenance, OutlineProvenance::NoTitle);
        assert_eq!(
            source_slice(
                source,
                *loaded
                    .locations
                    .nodes
                    .get(&SchemaNode::Title)
                    .expect("title: null anchors the title node")
            ),
            "null"
        );
    }

    #[test]
    fn sugar_forms_carry_their_provenance() {
        let titled = valid("version: 1\ntitle: Doc\nsections: []\n");
        assert_eq!(titled.outline_provenance, OutlineProvenance::Title);
        let bare = valid("version: 1\nsections: []\n");
        assert_eq!(bare.outline_provenance, OutlineProvenance::BareSections);
    }

    #[test]
    fn outline_rules_are_the_canonical_model_and_anchor_at_their_spellings() {
        let source = r#"version: 1
outline:
  - match: Part
    required: true
    sections:
      - match: Overview
        required: true
"#;
        let loaded = load_schema(source).expect("a single-rule outline loads");
        let schema = &loaded.schema;
        assert_eq!(schema.outline_provenance, OutlineProvenance::Outline);
        assert_eq!(
            schema.outline[0].matcher,
            Matcher::Exact(ExactText("Part".into()))
        );
        assert_eq!(
            schema.outline[0].outcome,
            RuleOutcome::Allow(Cardinality {
                min: 1,
                max: UpperBound::Bounded(1)
            })
        );
        assert_eq!(
            schema.outline[0].sections[0].matcher,
            Matcher::Exact(ExactText("Overview".into()))
        );
        // The outline rule is an ordinary rule at the empty scope; its child
        // anchors one scope below. There is no title node: nothing in this
        // schema is a title.
        assert!(!loaded.locations.nodes.contains_key(&SchemaNode::Title));
        assert_eq!(
            source_slice(
                source,
                *loaded
                    .locations
                    .nodes
                    .get(&SchemaNode::Rule(RulePath {
                        scope: ScopePath(Vec::new()),
                        index: RuleIndex(0),
                    }))
                    .expect("the outline rule is the root scope's first rule")
            ),
            "match: Part\n    required: true\n    sections:\n      - match: Overview\n        required: true\n"
        );
        assert_eq!(
            source_slice(
                source,
                *loaded
                    .locations
                    .nodes
                    .get(&SchemaNode::Rule(RulePath {
                        scope: ScopePath(vec![RuleIndex(0)]),
                        index: RuleIndex(0),
                    }))
                    .expect("the outline rule's child sits one scope below")
            ),
            "match: Overview\n        required: true\n"
        );
    }

    #[test]
    fn sugar_and_outline_forms_parse_to_the_same_model() {
        let sugar = valid(
            r#"version: 1
title: "Doc *"
sections:
  - match: Overview
    required: true
    sections:
      - match: Details
  - match: Second
constraints:
  - any_of: [overview, second]
"#,
        );
        let general = valid(
            r#"version: 1
outline:
  - match: "Doc *"
    required: true
    sections:
      - match: Overview
        required: true
        sections:
          - match: Details
      - match: Second
    constraints:
      - any_of: [overview, second]
"#,
        );
        assert_eq!(sugar.outline_provenance, OutlineProvenance::Title);
        assert_eq!(general.outline_provenance, OutlineProvenance::Outline);
        let mut general_as_sugar = general;
        general_as_sugar.outline_provenance = OutlineProvenance::Title;
        assert_eq!(sugar, general_as_sugar);
    }

    #[test]
    fn an_outline_declares_any_number_of_ordinary_h1_rules() {
        let schema = valid(
            r#"version: 1
outline:
  - match: "Part *"
    repeat: "1..n"
  - id: appendix
    match: Appendix
    strict: true
"#,
        );
        assert_eq!(schema.outline.len(), 2);
        assert_eq!(
            schema.outline[0].outcome,
            RuleOutcome::Allow(Cardinality {
                min: 1,
                max: UpperBound::Unbounded
            })
        );
        assert_eq!(schema.outline[1].id, Some(RuleId("appendix".into())));
        assert!(schema.outline[1].strict);
    }

    #[test]
    fn an_empty_outline_is_refused_toward_title_null() {
        // `outline: []` would constrain nothing — the outline scope is open,
        // so h1 headers would pass unvalidated — while its author almost
        // certainly means "no h1", which `title: null` declares.
        let invalid = invalid("version: 1\noutline: []\n");
        assert_eq!(
            invalid.errors.first.message,
            "outline must declare at least one rule; a document with no h1 headers \
             is declared with `title: null`"
        );
    }

    #[test]
    fn outline_conflicts_with_title_at_the_second_declared_key() {
        let source = "version: 1\ntitle: Doc\noutline:\n  - match: Doc\n    required: true\n";
        let invalid = invalid(source);
        let errors = invalid.errors.iter().collect::<Vec<_>>();
        assert_eq!(errors.len(), 1);
        let error = errors[0];
        assert_eq!(error.kind, SchemaErrorKind::ConflictingOutline);
        assert_eq!(
            error.message,
            "`outline` cannot be declared together with `title`"
        );
        assert_eq!(
            source_slice(source, error.range),
            "- match: Doc\n    required: true\n"
        );
        assert_eq!(error.related.len(), 1);
        assert_eq!(source_slice(source, error.related[0].range), "Doc");
        assert_eq!(error.related[0].message, "`title` declared here");
    }

    #[test]
    fn outline_conflicts_with_sections_anchoring_whichever_comes_second() {
        // `outline` first: the error anchors at `sections`.
        let source = "version: 1\noutline:\n  - match: Doc\n    required: true\nsections: []\n";
        let invalid = invalid(source);
        let errors = invalid.errors.iter().collect::<Vec<_>>();
        assert_eq!(errors.len(), 1);
        let error = errors[0];
        assert_eq!(error.kind, SchemaErrorKind::ConflictingOutline);
        assert_eq!(
            error.message,
            "`sections` cannot be declared together with `outline`"
        );
        assert_eq!(source_slice(source, error.range), "[]");
        assert_eq!(error.related[0].message, "`outline` declared here");
    }

    #[test]
    fn top_level_constraints_beside_outline_attach_to_the_h1_scope() {
        // Their refs resolve among the outline rules themselves.
        let schema = valid(
            "version: 1\noutline:\n  - id: intro\n    match: Intro\n\
             \x20 - id: body\n    match: Body\nconstraints:\n  - ordered: [intro, body]\n",
        );
        assert_eq!(schema.constraints.len(), 1);
        assert!(schema
            .outline
            .iter()
            .all(|rule| rule.constraints.is_empty()));

        // A sugar schema's top-level constraints attach to the `sections`
        // scope instead — the desugared rule's child scope — leaving the
        // schema-level list empty.
        let sugar = valid(
            "version: 1\nsections:\n  - id: a\n    match: A\n  - id: b\n    match: B\n\
             constraints:\n  - ordered: [a, b]\n",
        );
        assert!(sugar.constraints.is_empty());
        assert_eq!(sugar.outline[0].constraints.len(), 1);
    }

    #[test]
    fn schema_root_refs_anchor_at_the_outline_scope_in_the_general_form() {
        // `$` names the h1 rules for `outline:` schemas; a sugar schema's
        // `$.` refs keep resolving against its `sections` scope.
        let schema = valid(
            "version: 1\noutline:\n  - id: doc\n    match: Doc\n    required: true\n\
             \x20   sections:\n      - id: a\n        match: A\n        constraints:\n\
             \x20         - requires: { if: \"$.doc.a\", then: \"$.doc\" }\n",
        );
        assert_eq!(schema.outline[0].sections[0].constraints.len(), 1);
        // The same spelling that resolved through `sections` before still
        // does: `$.a` in sugar reaches the top-level `sections` rule.
        let sugar = valid(
            "version: 1\nsections:\n  - id: a\n    match: A\n    sections:\n\
             \x20     - id: b\n        match: B\n    constraints:\n\
             \x20     - requires: { if: b, then: \"$.a\" }\n",
        );
        assert_eq!(sugar.outline[0].sections[0].constraints.len(), 1);
        // An unresolved `$.` ref in the general form is a real error, not a
        // gate: `$.a` skips the outline level.
        let unresolved = invalid(
            "version: 1\noutline:\n  - id: doc\n    match: Doc\n    required: true\n\
             \x20   sections:\n      - id: a\n        match: A\n    constraints:\n\
             \x20     - requires: { if: a, then: \"$.a\" }\n",
        );
        assert!(unresolved
            .errors
            .iter()
            .any(|error| error.kind == SchemaErrorKind::UnresolvedRef
                && error.message == "unresolved ref `$.a`"));
    }

    #[test]
    fn outline_rules_take_every_cardinality_spelling() {
        let schema = valid("version: 1\noutline:\n  - match: Doc\n    repeat: \"1..1\"\n");
        assert_eq!(
            schema.outline[0].outcome,
            RuleOutcome::Allow(Cardinality {
                min: 1,
                max: UpperBound::Bounded(1)
            })
        );
        // No cardinality at all is the ordinary open default.
        let default = valid("version: 1\noutline:\n  - match: Doc\n");
        assert_eq!(
            default.outline[0].outcome,
            RuleOutcome::Allow(Cardinality {
                min: 0,
                max: UpperBound::Unbounded
            })
        );
    }

    #[test]
    fn errors_inside_an_outline_rule_anchor_at_their_own_spellings() {
        let source = r#"version: 1
outline:
  - match: Doc
    required: true
    sections:
      - match: "/(/"
"#;
        let invalid = invalid(source);
        let regex = invalid
            .errors
            .iter()
            .find(|error| error.kind == SchemaErrorKind::InvalidMatcher)
            .expect("the child rule's regex is invalid");
        assert_eq!(source_slice(source, regex.range), "\"/(/\"");
    }

    #[test]
    fn constraints_on_an_outline_rule_anchor_at_their_own_spellings() {
        let source = r#"version: 1
outline:
  - match: Doc
    required: true
    sections:
      - match: Overview
    constraints:
      - one_of: [missing, alike]
"#;
        let invalid = invalid(source);
        let unresolved = invalid
            .errors
            .iter()
            .find(|error| error.kind == SchemaErrorKind::UnresolvedRef)
            .expect("the constraint refs do not resolve");
        assert_eq!(
            source_slice(source, unresolved.range),
            "one_of: [missing, alike]\n"
        );
    }

    #[test]
    fn ordered_refs_through_a_repeatable_h1_rule_are_refused() {
        // §5.1 at the outline level: an ordered ref whose path crosses a
        // repeatable ancestor has no single document position to compare, so
        // `Part` under `repeat: 1..n` cannot carry an ordered ref path.
        let invalid = invalid(
            "version: 1\noutline:\n  - id: part\n    match: \"Part *\"\n    repeat: \"1..n\"\n\
             \x20   sections:\n      - id: a\n        match: A\n      - id: b\n        match: B\n\
             constraints:\n  - ordered: [part.a, part.b]\n",
        );
        assert!(invalid
            .errors
            .iter()
            .any(|error| error.kind == SchemaErrorKind::OrderedScopeMismatch));
    }

    #[test]
    fn duplicate_id_error_and_related_location_point_to_each_scalar() {
        let source = r#"version: 1
sections:
  - id: duplicate
    match: First
  - id: duplicate
    match: Second
"#;
        let invalid = invalid(source);
        let error = invalid
            .errors
            .iter()
            .find(|error| error.kind == SchemaErrorKind::DuplicateId)
            .unwrap_or_else(|| panic!("missing duplicate-id error"));
        assert_eq!(source_slice(source, error.range), "duplicate");
        assert_eq!(error.related.len(), 1);
        assert_eq!(source_slice(source, error.related[0].range), "duplicate");
        assert_ne!(error.range.range.start, error.related[0].range.range.start);
    }

    #[test]
    fn successful_node_locations_are_narrower_than_the_document() {
        let source = r#"version: 1
title: "*"
sections:
  - match: Overview
  - match: Details
constraints:
  - ordered: [overview, details]
"#;
        let loaded = match load_schema(source) {
            Ok(loaded) => loaded,
            Err(invalid) => panic!("unexpected errors: {:#?}", invalid.errors),
        };
        let addresses = [
            SchemaNode::Title,
            SchemaNode::Rule(RulePath {
                scope: ScopePath(Vec::new()),
                index: RuleIndex(0),
            }),
            SchemaNode::Constraint(ConstraintPath {
                scope: ScopePath(Vec::new()),
                index: ConstraintIndex(0),
            }),
        ];
        for address in addresses {
            let range = loaded
                .locations
                .nodes
                .get(&address)
                .copied()
                .unwrap_or_else(|| panic!("missing range for {address:?}"));
            assert!(range.range.start > loaded.locations.document.range.start);
            assert!(range.range.end <= loaded.locations.document.range.end);
            assert!(range.range.start < range.range.end);
            assert_ne!(range, loaded.locations.document);
        }
    }

    #[test]
    fn repeat_accepts_u32_boundary_and_rejects_overflow() {
        let schema = valid(
            r#"
version: 1
sections:
  - match: many
    repeat: 4294967295..4294967295
"#,
        );
        assert_eq!(
            schema.addressed_root_rules()[0].outcome,
            RuleOutcome::Allow(Cardinality {
                min: u32::MAX,
                max: UpperBound::Bounded(u32::MAX)
            })
        );
        let kinds = error_kinds(
            r#"
version: 1
sections:
  - match: too-many
    repeat: 4294967296..n
"#,
        );
        assert_eq!(kinds, vec![SchemaErrorKind::InvalidRepeat]);
    }

    #[test]
    fn external_source_ids_report_exhaustion_instead_of_saturating() {
        assert_eq!(external_source_id(0), Some(SourceId(1)));
        #[cfg(target_pointer_width = "64")]
        assert_eq!(external_source_id(u32::MAX as usize), None);
    }

    #[test]
    fn resolves_constraints_and_normalizes_frontmatter_scalars() {
        let schema = valid(
            r#"
version: 1
sections:
  - match: Overview
    sections:
      - match: Goals
  - match: Deployment
constraints:
  - requires: { if: deployment, then: [$.overview.goals, fm.count=0x10] }
"#,
        );
        let Constraint::Requires { consequences, .. } = &schema.outline[0].constraints[0] else {
            panic!("expected requires")
        };
        assert_eq!(
            consequences.rest[0],
            Proposition::Frontmatter(FrontmatterRef {
                path: NonEmpty {
                    first: FrontmatterKey("count".into()),
                    rest: vec![]
                },
                equals: Some(FrontmatterScalar::Integer(CanonicalInteger("16".into())))
            })
        );
    }

    #[test]
    fn frontmatter_ref_identity_uses_simple_case_folding() {
        let duplicate = error_kinds(
            r#"
version: 1
sections: []
constraints:
  - any_of: [fm.key=ſ, fm.key=S]
"#,
        );
        assert_eq!(duplicate, vec![SchemaErrorKind::DuplicateRef]);

        let schema = valid(
            r#"
version: 1
sections: []
constraints:
  - any_of: [fm.key=ß, fm.key=ss]
"#,
        );
        assert_eq!(schema.outline[0].constraints.len(), 1);
    }

    #[test]
    fn rejects_dangling_forbidden_duplicate_and_mis_scoped_ordered_refs() {
        let kinds = error_kinds(
            r#"
version: 1
sections:
  - id: repeated
    match: Repeated
    sections:
      - match: Child
  - id: denied
    match: Denied
    allow: false
constraints:
  - any_of: [missing, missing]
  - requires: { if: denied, then: denied }
  - ordered: [repeated.child, denied]
"#,
        );
        assert!(kinds.contains(&SchemaErrorKind::UnresolvedRef));
        assert!(kinds.contains(&SchemaErrorKind::ForbiddenRef));
        assert!(kinds.contains(&SchemaErrorKind::OrderedScopeMismatch));
    }

    #[test]
    fn checks_constraint_lexemes_even_when_a_rule_cannot_be_built() {
        let kinds = error_kinds(
            r#"
version: 1
sections:
  - match: /(?=invalid)/
constraints:
  - any_of: [bad..ref, also..bad]
"#,
        );
        assert_eq!(
            kinds,
            vec![
                SchemaErrorKind::InvalidMatcher,
                SchemaErrorKind::UnresolvedRef,
                SchemaErrorKind::UnresolvedRef
            ]
        );
    }

    #[test]
    fn yaml_core_scalars_support_arbitrary_magnitude_without_signed_nan() {
        assert_eq!(
            parse_frontmatter_scalar("1e100000000000000000000000000000000000000"),
            FrontmatterScalar::Float(CanonicalFloat(
                "1e100000000000000000000000000000000000000".into()
            ))
        );
        assert_eq!(
            parse_frontmatter_scalar("-0xffffffffffffffffffffffffffffffff"),
            FrontmatterScalar::Integer(CanonicalInteger(
                "-340282366920938463463374607431768211455".into()
            ))
        );
        assert_eq!(
            parse_frontmatter_scalar("-.nan"),
            FrontmatterScalar::String("-.nan".into())
        );
        assert_eq!(
            parse_frontmatter_scalar("+.NaN"),
            FrontmatterScalar::String("+.NaN".into())
        );
    }

    #[test]
    fn normalizes_frontmatter_presence_policy() {
        let schema = valid(
            r#"
version: 1
frontmatter: { required: true }
sections: []
"#,
        );
        assert_eq!(
            schema.frontmatter,
            FrontmatterPolicy::Required { schema: None }
        );
    }

    #[test]
    fn linked_frontmatter_schema_requires_file_context() {
        let kinds = error_kinds(
            r#"
version: 1
frontmatter: { schema: frontmatter.schema.json }
sections: []
"#,
        );
        assert_eq!(kinds, vec![SchemaErrorKind::InvalidFrontmatterSchema]);
    }

    #[test]
    fn external_references_separate_physical_paths_from_id_based_logical_uris() {
        let references = json_schema_external_references(
            r#"{
                "$id": "https://example.com/schemas/root.json",
                "allOf": [
                    { "$ref": "defs.json" },
                    { "$id": "nested/child.json", "$ref": "more.json" }
                ]
            }"#,
            "file:///workspace/frontmatter.schema.json",
            "https://outlint.invalid/workspace/frontmatter.schema.json",
        )
        .expect("references resolve under both bases");

        assert_eq!(
            references,
            vec![
                crate::JsonSchemaExternalReference {
                    physical_uri: "file:///workspace/defs.json".into(),
                    logical_uri: "https://example.com/schemas/defs.json".into(),
                },
                crate::JsonSchemaExternalReference {
                    physical_uri: "file:///workspace/more.json".into(),
                    logical_uri: "https://example.com/schemas/nested/more.json".into(),
                },
            ]
        );
    }

    #[test]
    fn loads_and_resolves_linked_frontmatter_schema_with_local_ref() {
        let loaded = linked(
            r#"{"$schema":"https://json-schema.org/draft/2020-12/schema","$ref":"defs.json#/$defs/frontmatter"}"#,
            &[(
                "https://outlint.invalid/defs.json",
                r#"{"$defs":{"frontmatter":{"type":"object","required":["status"],"properties":{"status":{"enum":["draft","final"]}}}}}"#,
            )],
        )
        .expect("linked JSON Schema is valid");
        let FrontmatterPolicy::Optional { schema: Some(_) } = &loaded.schema.frontmatter else {
            panic!("expected an optional linked frontmatter schema")
        };
        assert!(loaded.sources.documents.contains_key(&SourceId(1)));
        assert!(loaded.sources.documents.contains_key(&SourceId(2)));
        assert!(loaded
            .locations
            .nodes
            .contains_key(&SchemaNode::FrontmatterSchemaDocument));
        let document = crate::parse_markdown(
            "---\nstatus: review\n---\n",
            crate::MarkdownOptions::default(),
        );
        let diagnostics =
            crate::validate(&loaded.schema, &document).expect("loader-created validator compiles");
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].id, crate::DiagnosticId::FrontmatterSchema);
    }

    #[test]
    fn accepts_circular_fragment_refs_without_validation_time_io() {
        linked(
            r##"{
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "$ref": "#/$defs/node",
                "$defs": {
                    "node": {
                        "type": "object",
                        "properties": { "child": { "$ref": "#/$defs/node" } }
                    }
                }
            }"##,
            &[],
        )
        .expect("circular local refs are valid");
    }

    #[test]
    fn accepts_cycles_across_preloaded_resources() {
        linked(
            r#"{"$ref":"other.json"}"#,
            &[(
                "https://outlint.invalid/other.json",
                r#"{"$ref":"root.json"}"#,
            )],
        )
        .expect("the immutable registry supports cross-resource cycles");
    }

    #[test]
    fn semantic_schema_equality_ignores_resource_labels() {
        let first = linked("{\"type\":\"object\"}", &[]).expect("first schema is valid");
        let mut input = resource("https://outlint.invalid/root.json", "{\"type\":\"object\"}");
        input.label = Some(SourceLabel("a/different/location.json".into()));
        let second = load_schema_with_resources(
            linked_schema_source(),
            Some(SourceLabel("elsewhere/schema.yml".into())),
            Some(LinkedJsonSchemaInput {
                root_uri: "https://outlint.invalid/root.json".into(),
                resources: vec![input],
            }),
        )
        .expect("second schema is valid");
        assert_eq!(first.schema, second.schema);
    }

    #[test]
    fn rejects_remote_refs_without_network_retrieval() {
        let remote_uri = "https://example.invalid/frontmatter.schema.json";
        let invalid = linked(
            r#"{
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "$ref": "https://example.invalid/frontmatter.schema.json"
            }"#,
            &[],
        )
        .expect_err("remote refs are unsupported");
        assert_eq!(
            invalid.errors.first.kind,
            SchemaErrorKind::InvalidFrontmatterSchema
        );
        assert_eq!(invalid.errors.first.range.source, SourceId(1));
        assert!(
            invalid.errors.first.message.contains(&format!(
                "JSON Schema resource `{remote_uri}` was not preloaded"
            )),
            "unexpected retrieval diagnostic: {}",
            invalid.errors.first.message
        );
        assert!(!invalid.errors.first.message.contains("Default retriever"));
    }

    #[test]
    fn preserves_ref_siblings_and_boolean_targets() {
        let loaded = linked(
            r#"{"$ref":"defs.json#/$defs/base","required":["sibling"]}"#,
            &[(
                "https://outlint.invalid/defs.json",
                r#"{"$defs":{"base":{"required":["target"]}}}"#,
            )],
        )
        .expect("ref with duplicate sibling keyword is valid");
        let document = crate::parse_markdown(
            "---\ntarget: true\n---\n",
            crate::MarkdownOptions::default(),
        );
        let diagnostics =
            crate::validate(&loaded.schema, &document).expect("loader-created validator compiles");
        assert_eq!(diagnostics.len(), 1, "`$ref` siblings must both apply");

        let loaded = linked(
            r#"{"$ref":"defs.json#/$defs/base","type":"object"}"#,
            &[(
                "https://outlint.invalid/defs.json",
                r#"{"$defs":{"base":false}}"#,
            )],
        )
        .expect("boolean ref target is valid");
        let diagnostics =
            crate::validate(&loaded.schema, &document).expect("loader-created validator compiles");
        assert_eq!(diagnostics.len(), 1, "false target must remain rejecting");
    }

    #[test]
    fn positions_invalid_linked_json_schema_in_its_own_source() {
        let invalid = linked("{ invalid json }", &[]).expect_err("linked JSON Schema is invalid");
        assert_eq!(invalid.errors.iter().count(), 1);
        assert_eq!(
            invalid.errors.first.kind,
            SchemaErrorKind::InvalidFrontmatterSchema
        );
        assert_eq!(invalid.errors.first.range.source, SourceId(1));
        let source = invalid
            .sources
            .documents
            .get(&SourceId(1))
            .unwrap_or_else(|| panic!("missing linked JSON Schema source"));
        assert_eq!(source.label, Some(SourceLabel("root.json".into())));
    }

    #[test]
    fn positions_invalid_transitive_resource_in_its_own_source() {
        let invalid = linked(
            r#"{"$ref":"defs.json"}"#,
            &[("https://outlint.invalid/defs.json", "{ invalid json }")],
        )
        .expect_err("transitive resource is invalid");
        assert_eq!(invalid.errors.first.range.source, SourceId(2));
        assert_eq!(
            invalid.sources.documents[&SourceId(2)].label,
            Some(SourceLabel("defs.json".into()))
        );
    }

    #[test]
    fn collects_independent_linked_resource_errors_in_input_order() {
        let invalid = linked(
            r#"{"allOf":[{"$ref":"first.json"},{"$ref":"second.json"}]}"#,
            &[
                ("https://outlint.invalid/first.json", "{ invalid json }"),
                ("https://outlint.invalid/second.json", "[]"),
            ],
        )
        .expect_err("both invalid linked resources must be reported");
        let errors = invalid.errors.iter().collect::<Vec<_>>();

        assert_eq!(errors.len(), 2);
        assert_eq!(errors[0].kind, SchemaErrorKind::InvalidFrontmatterSchema);
        assert_eq!(errors[0].range.source, SourceId(2));
        assert!(errors[0]
            .message
            .starts_with("invalid linked JSON Schema document:"));
        assert_eq!(errors[1].kind, SchemaErrorKind::InvalidFrontmatterSchema);
        assert_eq!(errors[1].range.source, SourceId(3));
        assert_eq!(
            errors[1].message,
            "linked JSON Schema root must be an object or boolean"
        );
    }

    #[test]
    fn duplicate_linked_resource_error_uses_the_duplicate_occurrence_source() {
        let uri = "https://outlint.invalid/duplicate.json";
        let mut first = resource(uri, "{ invalid json }");
        first.label = Some(SourceLabel("first-duplicate.json".into()));
        let mut second = resource(uri, "{}");
        second.label = Some(SourceLabel("second-duplicate.json".into()));
        let invalid = load_schema_with_resources(
            linked_schema_source(),
            Some(SourceLabel("schema.yml".into())),
            Some(LinkedJsonSchemaInput {
                root_uri: "https://outlint.invalid/root.json".into(),
                resources: vec![
                    resource(
                        "https://outlint.invalid/root.json",
                        r#"{"$ref":"duplicate.json"}"#,
                    ),
                    first,
                    second,
                ],
            }),
        )
        .expect_err("invalid and duplicate resources must both be reported");
        let errors = invalid.errors.iter().collect::<Vec<_>>();

        assert_eq!(errors.len(), 2);
        assert_eq!(errors[0].range.source, SourceId(2));
        assert_eq!(errors[1].range.source, SourceId(3));
        assert!(errors[1]
            .message
            .starts_with("duplicate JSON Schema resource URI"));
        assert_eq!(
            invalid.sources.documents[&SourceId(3)].label,
            Some(SourceLabel("second-duplicate.json".into()))
        );
    }

    #[test]
    fn positions_linked_schema_read_failures_at_the_unreadable_resource() {
        let message = "cannot inspect linked JSON Schema 'missing.json': not found";
        for (resources, expected_source, expected_label) in [
            (
                vec![failed_resource(
                    "https://outlint.invalid/root.json",
                    "missing-root.json",
                    message,
                )],
                SourceId(1),
                "missing-root.json",
            ),
            (
                vec![
                    resource(
                        "https://outlint.invalid/root.json",
                        r#"{"$ref":"missing.json"}"#,
                    ),
                    failed_resource(
                        "https://outlint.invalid/missing.json",
                        "missing.json",
                        message,
                    ),
                ],
                SourceId(2),
                "missing.json",
            ),
        ] {
            let invalid = load_schema_with_resources(
                linked_schema_source(),
                Some(SourceLabel("schema.yml".into())),
                Some(LinkedJsonSchemaInput {
                    root_uri: "https://outlint.invalid/root.json".into(),
                    resources,
                }),
            )
            .expect_err("linked schema read failure is invalid");

            assert_eq!(
                invalid.errors.first.kind,
                SchemaErrorKind::InvalidFrontmatterSchema
            );
            assert_eq!(invalid.errors.first.message, message);
            assert_eq!(invalid.errors.first.range.source, expected_source);
            assert_eq!(
                invalid.errors.first.range.range,
                TextRange {
                    start: ByteOffset(0),
                    end: ByteOffset(0),
                }
            );
            let source = &invalid.sources.documents[&expected_source];
            assert_eq!(source.label, Some(SourceLabel(expected_label.into())));
            assert_eq!(&*source.text, "");
        }
    }

    #[test]
    fn rejects_missing_and_unsupported_linked_json_schemas() {
        let root = resource("https://outlint.invalid/not-root.json", "{}");
        let missing = load_schema_with_resources(
            linked_schema_source(),
            None,
            Some(LinkedJsonSchemaInput {
                root_uri: "https://outlint.invalid/root.json".into(),
                resources: vec![root],
            }),
        )
        .expect_err("missing linked schema is invalid");
        assert_eq!(
            missing.errors.first.kind,
            SchemaErrorKind::InvalidFrontmatterSchema
        );
        assert_eq!(missing.errors.first.range.source, SourceId(0));

        let unsupported = linked(
            r#"{"$schema":"http://json-schema.org/draft-07/schema#","type":"object"}"#,
            &[],
        )
        .expect_err("unsupported dialect is invalid");
        assert_eq!(
            unsupported.errors.first.kind,
            SchemaErrorKind::InvalidFrontmatterSchema
        );
        assert_eq!(unsupported.errors.first.range.source, SourceId(1));
    }

    fn linked(root: &str, resources: &[(&str, &str)]) -> LoadSchemaResult {
        let mut rest = Vec::new();
        for (uri, source) in resources {
            rest.push(resource(uri, source));
        }
        load_schema_with_resources(
            linked_schema_source(),
            Some(SourceLabel("schema.yml".into())),
            Some(LinkedJsonSchemaInput {
                root_uri: "https://outlint.invalid/root.json".into(),
                resources: std::iter::once(resource("https://outlint.invalid/root.json", root))
                    .chain(rest)
                    .collect(),
            }),
        )
    }

    fn resource(uri: &str, source: &str) -> crate::JsonSchemaResourceInput {
        crate::JsonSchemaResourceInput {
            uri: uri.into(),
            label: Some(SourceLabel(
                uri.rsplit('/').next().unwrap_or(uri).to_owned(),
            )),
            contents: crate::JsonSchemaResourceContents::Loaded(Arc::from(source)),
        }
    }

    fn failed_resource(uri: &str, label: &str, message: &str) -> crate::JsonSchemaResourceInput {
        crate::JsonSchemaResourceInput {
            uri: uri.into(),
            label: Some(SourceLabel(label.into())),
            contents: crate::JsonSchemaResourceContents::ReadFailure(message.into()),
        }
    }

    fn linked_schema_source() -> &'static str {
        "version: 1\nfrontmatter:\n  schema: root.json\ntitle: null\nsections: []\n"
    }

    /// Builds a document whose root reference starts a chain of `links` hops,
    /// the last of which targets `tail`. It declares `links + 1` references
    /// and nests three levels however long the chain is.
    fn reference_chain(links: usize, tail: &str) -> String {
        let mut definitions = serde_json::Map::new();
        definitions.insert("end".into(), serde_json::Value::Bool(true));
        for index in 0..links {
            let target = if index + 1 == links {
                tail.to_owned()
            } else {
                format!("#/$defs/{}", index + 1)
            };
            definitions.insert(index.to_string(), serde_json::json!({ "$ref": target }));
        }
        serde_json::json!({ "$ref": "#/$defs/0", "$defs": definitions }).to_string()
    }

    #[test]
    fn refuses_a_reference_chain_longer_than_the_compiler_can_recurse_over() {
        // Compiling a reference re-enters the compiler at its target, so a
        // chain costs a stack frame per link while every link of it sits at
        // the same JSON depth: the YAML depth limit and `serde_json`'s parse
        // limit are both satisfied with room to spare by a chain long enough
        // to abort the process. The count is charged before the graph reaches
        // the compiler, so the boundary is pinned on both sides -- a graph
        // spending the whole budget must still load, or the bound would be
        // free to drift downwards unnoticed.
        let at_budget = reference_chain(MAX_JSON_SCHEMA_REFERENCES - 1, "#/$defs/end");
        assert_eq!(
            json_schema_reference_count(
                &serde_json::from_str(&at_budget).expect("chain is valid JSON")
            ),
            MAX_JSON_SCHEMA_REFERENCES
        );
        linked(&at_budget, &[]).expect("a graph spending the whole budget still loads");

        let over_budget = reference_chain(MAX_JSON_SCHEMA_REFERENCES, "#/$defs/end");
        let invalid = linked(&over_budget, &[]).expect_err("one reference more is refused");
        assert_eq!(
            invalid.errors.first.kind,
            SchemaErrorKind::InvalidFrontmatterSchema
        );
        assert_eq!(
            invalid.errors.first.message,
            json_schema_reference_budget_message()
        );
        assert_eq!(invalid.errors.first.range.source, SourceId(1));
        assert!(invalid.errors.rest.is_empty());
    }

    #[test]
    fn the_reference_budget_counts_dynamic_references_too() {
        // `$dynamicRef` compiles through the same function as `$ref` and
        // re-enters the compiler the same way, so a chain of them aborts at
        // the same length. Counting only `$ref` would leave the crash
        // reachable by renaming one keyword.
        let over_budget = reference_chain(MAX_JSON_SCHEMA_REFERENCES, "#/$defs/end")
            .replace(r#""$ref""#, r#""$dynamicRef""#);
        let invalid = linked(&over_budget, &[]).expect_err("a dynamic chain is a chain");
        assert_eq!(
            invalid.errors.first.message,
            json_schema_reference_budget_message()
        );
    }

    #[test]
    fn the_reference_budget_spans_the_graph_and_names_where_it_runs_out() {
        // The compiler recurses across resource boundaries as readily as
        // within one, so a per-document budget would be no budget at all: two
        // documents each under it can name a chain twice as long as either.
        // The total is therefore charged over the graph, and reported against
        // the resource whose references spend the last of it rather than the
        // root, so the diagnostic points at a document the author can shorten.
        let half = MAX_JSON_SCHEMA_REFERENCES / 2;
        let root = reference_chain(half - 1, "defs.json#/$defs/0");
        let definitions = reference_chain(half, "#/$defs/end");
        assert_eq!(
            json_schema_reference_count(&serde_json::from_str(&root).expect("root is valid JSON"))
                + json_schema_reference_count(
                    &serde_json::from_str(&definitions).expect("defs are valid JSON")
                ),
            MAX_JSON_SCHEMA_REFERENCES + 1
        );

        let invalid = linked(
            &root,
            &[("https://outlint.invalid/defs.json", &definitions)],
        )
        .expect_err("a chain split across two documents is still one chain");
        assert_eq!(
            invalid.errors.first.kind,
            SchemaErrorKind::InvalidFrontmatterSchema
        );
        assert_eq!(
            invalid.errors.first.message,
            json_schema_reference_budget_message()
        );
        assert_eq!(invalid.errors.first.range.source, SourceId(2));
    }

    #[test]
    fn rejects_implication_objects_with_the_wrong_keys() {
        let kinds = error_kinds(
            r#"
version: 1
sections: []
constraints:
  - requires: { condition: foo, consequence: bar }
"#,
        );
        assert!(kinds.contains(&SchemaErrorKind::InvalidDocumentShape));
    }

    proptest! {
        #[test]
        fn regex_body_round_trips_delimiter_escaping_without_other_escapes(
            source in any::<String>(),
        ) {
            prop_assume!(!source.contains('\\'));
            let encoded = source
                .chars()
                .flat_map(|character| {
                    if character == '/' {
                        vec!['\\', '/']
                    } else {
                        vec![character]
                    }
                })
                .collect::<String>();
            let decoded = regex_body(&encoded);
            prop_assert_eq!(decoded.as_deref(), Some(source.as_str()));
        }

        #[test]
        fn parse_repeat_normalizes_valid_finite_bounds(min in any::<u32>(), max in any::<u32>()) {
            prop_assume!(max >= min && max > 0);
            let source = format!("{min}..{max}");
            prop_assert_eq!(
                parse_repeat(&source),
                Some(Cardinality {
                    min,
                    max: UpperBound::Bounded(max),
                })
            );
        }

        #[test]
        fn parse_repeat_normalizes_unbounded_bounds(min in any::<u32>()) {
            let source = format!("{min}..n");
            prop_assert_eq!(
                parse_repeat(&source),
                Some(Cardinality {
                    min,
                    max: UpperBound::Unbounded,
                })
            );
        }

        #[test]
        fn canonical_integer_normalization_is_idempotent(value in any::<i64>()) {
            let source = if value >= 0 {
                format!("+000{value}")
            } else {
                format!("-000{}", value.unsigned_abs())
            };
            let canonical = canonical_integer(&source).expect("generated decimal is valid");
            prop_assert_eq!(canonical.as_str(), value.to_string());
            let repeated = canonical_integer(&canonical);
            prop_assert_eq!(repeated.as_deref(), Some(canonical.as_str()));
        }

        #[test]
        fn canonical_float_normalization_is_idempotent(
            coefficient in any::<i64>(),
            exponent in any::<i16>(),
        ) {
            let source = format!("{coefficient}e{exponent}");
            let canonical = canonical_float(&source).expect("generated decimal float is valid");
            let repeated = canonical_float(&canonical);
            prop_assert_eq!(repeated.as_deref(), Some(canonical.as_str()));
        }
    }
}
