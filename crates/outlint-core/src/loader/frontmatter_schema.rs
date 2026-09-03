//! Inline and linked frontmatter JSON Schema preparation.

use std::collections::{BTreeMap, HashSet};

use serde_json::Value;

use crate::{
    ByteOffset, FrontmatterPolicy, JsonSchemaResourceContents, LinkedJsonSchemaInput, NonEmpty,
    SchemaErrorKind, SchemaNode, SourceId, SourceRange, TextRange,
};

use super::constraints::non_empty;
use super::yaml::{parse_schema_yaml, schema_yaml_to_json};
use super::{JsonMap, Loader, RangeKey, RawFrontmatter, RawFrontmatterSchema};

#[derive(Debug)]
pub(super) struct PreparedExternalSchema {
    pub(super) root_source: SourceId,
    pub(super) result: Result<crate::FrontmatterSchema, NonEmpty<PreparedExternalError>>,
}

#[derive(Debug)]
pub(super) struct PreparedExternalError {
    pub(super) source: SourceId,
    pub(super) message: String,
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

pub(super) fn prepare_external_schema(
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

/// Stable hierarchical identity for resolving relative `$id` values inline.
///
/// The reserved `.invalid` top-level domain cannot identify a retrievable
/// resource, and inline reference values are checked before compilation, so
/// this supplies URI hierarchy without opening an external loading path.
const INLINE_FRONTMATTER_SCHEMA_URI: &str =
    "https://outlint.invalid/inline/frontmatter.schema.json";

fn prepare_inline_schema(mapping: JsonMap) -> Result<crate::FrontmatterSchema, NonEmpty<String>> {
    let root = Value::Object(mapping);
    let mut errors = invalid_inline_references(&root);
    // A malformed reference is also rejected by the draft meta-schema, but
    // the inline contract has a more specific rule and diagnostic. Avoid
    // reporting both descriptions for the same keyword.
    if errors.is_empty() {
        if let Err(message) = validate_json_schema_document(&root) {
            errors.push(message);
        }
    }
    if json_schema_reference_count(&root) > MAX_JSON_SCHEMA_REFERENCES {
        errors.push(json_schema_reference_budget_message());
    }
    if let Some(errors) = non_empty(errors) {
        return Err(errors);
    }

    {
        let registry = preloaded_json_schema_registry()
            .add(INLINE_FRONTMATTER_SCHEMA_URI, &root)
            .and_then(jsonschema::RegistryBuilder::prepare)
            .map_err(|error| {
                single_string_error(format!(
                    "cannot prepare inline frontmatter JSON Schema: {error}"
                ))
            })?;
        jsonschema::draft202012::options()
            .with_registry(&registry)
            .with_base_uri(INLINE_FRONTMATTER_SCHEMA_URI.to_owned())
            .with_retriever(NoExternalRetrieve)
            .build(&root)
            .map_err(|error| {
                single_string_error(format!(
                    "cannot compile inline frontmatter JSON Schema: {error}"
                ))
            })?;
    }

    Ok(crate::FrontmatterSchema {
        root_uri: INLINE_FRONTMATTER_SCHEMA_URI.into(),
        root,
        resources: BTreeMap::new(),
    })
}

fn single_string_error(message: String) -> NonEmpty<String> {
    NonEmpty {
        first: message,
        rest: Vec::new(),
    }
}

pub(super) fn invalid_inline_references(value: &Value) -> Vec<String> {
    let mut errors = Vec::new();
    walk_json_objects(value, |object| {
        for keyword in ["$ref", "$dynamicRef"] {
            if let Some(child) = object.get(keyword) {
                match child.as_str() {
                    Some(reference) if reference.starts_with('#') => {}
                    Some(reference) => errors.push(format!(
                        "inline frontmatter JSON Schema `{keyword}` must be fragment-only, found `{reference}`"
                    )),
                    None => errors.push(format!(
                        "inline frontmatter JSON Schema `{keyword}` must be a string beginning with `#`"
                    )),
                }
            }
        }
    });
    errors
}

fn walk_json_objects(value: &Value, mut visit: impl FnMut(&JsonMap)) {
    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        match value {
            Value::Object(object) => {
                visit(object);
                pending.extend(object.values());
            }
            Value::Array(items) => pending.extend(items),
            _ => {}
        }
    }
}

pub(super) fn single_external_error(
    error: PreparedExternalError,
) -> NonEmpty<PreparedExternalError> {
    NonEmpty {
        first: error,
        rest: Vec::new(),
    }
}

/// How many reference-shaped members one frontmatter schema graph may declare.
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
/// path enters each evaluated reference member at most once: cycles are cut by
/// the compiler's own pending-node cache, which is why a self-reference or a
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
        "frontmatter JSON Schema declares more than {MAX_JSON_SCHEMA_REFERENCES} \
         `$ref` or `$dynamicRef` members"
    )
}

/// Counts reserved `$ref` and `$dynamicRef` members in one JSON document.
///
/// The walk carries an explicit stack rather than recursing, so what it costs
/// the call stack does not depend on the limit it enforces. A counter that
/// recursed would be safe only while a limit's worth of its own frames still
/// fit, which ties the choice of limit to the shape of the check and would
/// turn a later raise of the limit into the very overflow being refused here.
/// Those two member names are the keywords whose
/// compilation re-enters the compiler under draft 2020-12, the only dialect
/// [`validate_json_schema_document`] admits, and they are the same pair
/// [`collect_external_references`] follows.
///
/// Every object member with either reserved name counts, including members in
/// instance-shaped or otherwise unreachable data. A fragment JSON Pointer may
/// turn any object into an evaluated schema, so limiting the walk to recognized
/// Draft 2020-12 subresources would leave a hidden reference chain unbounded.
pub(crate) fn json_schema_reference_count(value: &serde_json::Value) -> usize {
    let mut references = 0usize;
    walk_json_objects(value, |object| {
        references = references.saturating_add(
            usize::from(object.contains_key("$ref"))
                + usize::from(object.contains_key("$dynamicRef")),
        );
    });
    references
}

fn validate_json_schema_document(value: &serde_json::Value) -> Result<(), String> {
    if !value.is_object() && !value.is_boolean() {
        return Err("frontmatter JSON Schema root must be an object or boolean".into());
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

pub(super) fn external_source_id(index: usize) -> Option<SourceId> {
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

impl Loader {
    pub(super) fn build_frontmatter(
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
            Some(RawFrontmatterSchema::Mapping(mapping)) => {
                let schema_range = self.range(RangeKey::FrontmatterField("schema".into()));
                self.nodes
                    .insert(SchemaNode::FrontmatterSchemaDeclaration, schema_range);
                self.nodes
                    .insert(SchemaNode::FrontmatterSchemaDocument, schema_range);
                match prepare_inline_schema(mapping) {
                    Ok(schema) => Some(schema),
                    Err(errors) => {
                        for message in std::iter::once(errors.first).chain(errors.rest) {
                            self.error_at(
                                SchemaErrorKind::InvalidFrontmatterSchema,
                                schema_range,
                                message,
                            );
                        }
                        return None;
                    }
                }
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
}
