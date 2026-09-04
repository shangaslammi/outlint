//! The version 2 JSON output envelope and its field conversions.

use serde_json::{json, Map, Value};

use crate::diagnostics::{
    RenderedDiagnostic, RenderedMatcher, RenderedReference, RenderedScalar, RenderedSchemaNode,
    RenderedTarget, ResultKind, ValidationResult,
};

pub(super) fn render_json(results: &[ValidationResult]) -> String {
    let diagnostic_count = results
        .iter()
        .map(|result| result.diagnostics.len())
        .sum::<usize>();
    let results = results
        .iter()
        .map(|result| {
            json!({
                "kind": match result.kind {
                    ResultKind::Document => "document",
                    ResultKind::Schema => "schema",
                },
                "path": result.path,
                "schema": result.schema,
                "diagnostics": result.diagnostics.iter().map(diagnostic_json).collect::<Vec<_>>()
            })
        })
        .collect::<Vec<_>>();
    let document_count = results
        .iter()
        .filter(|result| result["kind"] == "document")
        .count();
    let schema_count = results.len() - document_count;
    format!(
        "{}\n",
        json!({
            "version": 2,
            "results": results,
            "summary": {
                "files": results.len(),
                "documents": document_count,
                "schemas": schema_count,
                "diagnostics": diagnostic_count
            }
        })
    )
}

fn diagnostic_json(diagnostic: &RenderedDiagnostic) -> Value {
    let mut object = Map::new();
    object.insert("id".into(), json!(diagnostic.id));
    object.insert("message".into(), json!(diagnostic.message));
    object.insert(
        "location".into(),
        json!({ "line": diagnostic.line, "column": diagnostic.column }),
    );
    if let Some(target) = &diagnostic.target {
        object.insert("target".into(), target_json(target));
    }
    if let Some(node) = &diagnostic.schema_node {
        object.insert("schema_node".into(), schema_node_json(node));
    }
    if let Some(location) = &diagnostic.schema_location {
        object.insert(
            "schema_location".into(),
            json!({
                "path": location.path,
                "line": location.line,
                "column": location.column
            }),
        );
    }
    if !diagnostic.involved_headers.is_empty() {
        object.insert(
            "involved_headers".into(),
            Value::Array(
                diagnostic
                    .involved_headers
                    .iter()
                    .map(|header| {
                        json!({
                            "header_path": header.header_path,
                            "location": { "line": header.line, "column": header.column }
                        })
                    })
                    .collect(),
            ),
        );
    }
    if !diagnostic.references.is_empty() {
        object.insert(
            "references".into(),
            Value::Array(diagnostic.references.iter().map(reference_json).collect()),
        );
    }
    Value::Object(object)
}

fn target_json(target: &RenderedTarget) -> Value {
    match target {
        RenderedTarget::Header { path } => json!({ "kind": "header", "path": path }),
        RenderedTarget::MissingHeader { parent, matcher } => {
            json!({ "kind": "missing_header", "parent": parent, "matcher": matcher })
        }
        RenderedTarget::Document => json!({ "kind": "document" }),
        RenderedTarget::Frontmatter {
            line_range,
            pointer,
        } => {
            let mut object = Map::new();
            object.insert("kind".into(), json!("frontmatter"));
            if let Some(range) = line_range {
                object.insert(
                    "line_range".into(),
                    json!({
                        "start_line": range.start_line,
                        "end_line": range.end_line
                    }),
                );
            }
            if let Some(pointer) = pointer {
                object.insert("pointer".into(), json!(pointer));
            }
            Value::Object(object)
        }
    }
}

fn schema_node_json(node: &RenderedSchemaNode) -> Value {
    match node {
        RenderedSchemaNode::Title => json!({ "kind": "title" }),
        RenderedSchemaNode::Frontmatter => json!({ "kind": "frontmatter" }),
        RenderedSchemaNode::FrontmatterSchemaDeclaration => {
            json!({ "kind": "frontmatter_schema_declaration" })
        }
        RenderedSchemaNode::FrontmatterSchemaDocument => {
            json!({ "kind": "frontmatter_schema_document" })
        }
        RenderedSchemaNode::Rule { scope, index } => {
            json!({ "kind": "rule", "scope": scope, "index": index })
        }
        RenderedSchemaNode::Capture { scope, index, name } => {
            json!({ "kind": "capture", "scope": scope, "index": index, "name": name })
        }
        RenderedSchemaNode::FrontmatterCapture { name } => {
            json!({ "kind": "frontmatter_capture", "name": name })
        }
        RenderedSchemaNode::OrderEntry {
            scope,
            index,
            order_index,
        } => json!({
            "kind": "order_entry",
            "scope": scope,
            "index": index,
            "order_index": order_index
        }),
        RenderedSchemaNode::Constraint { scope, index } => {
            json!({ "kind": "constraint", "scope": scope, "index": index })
        }
    }
}

fn reference_json(reference: &RenderedReference) -> Value {
    match reference {
        RenderedReference::Rule {
            anchor,
            path,
            matcher,
        } => json!({
            "kind": "rule",
            "anchor": anchor,
            "path": path,
            "matcher": matcher_json(matcher)
        }),
        RenderedReference::Frontmatter { path, equals } => {
            let mut object = Map::new();
            object.insert("kind".into(), json!("frontmatter"));
            object.insert("path".into(), json!(path));
            if let Some(equals) = equals {
                object.insert("equals".into(), scalar_json(equals));
            }
            Value::Object(object)
        }
        RenderedReference::ResolvedRule {
            locator,
            anchor,
            path,
            positions,
            matcher,
        } => {
            let mut object = Map::new();
            object.insert("kind".into(), json!("rule"));
            object.insert("locator".into(), json!(locator));
            object.insert("anchor".into(), json!(anchor));
            object.insert("path".into(), json!(path));
            if let Some(positions) = positions {
                object.insert(
                    "positions".into(),
                    Value::Array(positions.iter().map(position_json).collect()),
                );
            }
            object.insert("matcher".into(), matcher_json(matcher));
            Value::Object(object)
        }
        RenderedReference::FrontmatterQuery {
            locator,
            query,
            equals,
        } => {
            let mut object = Map::new();
            object.insert("kind".into(), json!("frontmatter_query"));
            object.insert("locator".into(), json!(locator));
            object.insert("query".into(), json!(query));
            if let Some(equals) = equals {
                object.insert("equals".into(), scalar_json(equals));
            }
            Value::Object(object)
        }
        RenderedReference::FrontmatterCapture {
            locator,
            name,
            value_type,
        } => json!({
            "kind": "frontmatter_capture",
            "locator": locator,
            "name": name,
            "type": value_type
        }),
    }
}

/// One `positions` entry: an arbitrary-precision JSON integer, or null.
///
/// §11.3 is explicit that "position values are arbitrary-precision JSON
/// integers; consumers MUST NOT assume they fit a 64-bit integer type", so
/// the decimal text becomes a JSON *number* and never a quoted string. It is
/// parsed rather than converted through a machine integer because a value
/// wider than `u64` must survive intact. The lane that finalizes the version
/// 3 envelope pins this against the contract tests; well-formed digits are
/// all that ever reach it, so a refusal here can only mean the model was
/// built wrong, and null is the honest rendering of that.
fn position_json(digits: &Option<String>) -> Value {
    match digits {
        Some(digits) => serde_json::from_str(digits).unwrap_or(Value::Null),
        None => Value::Null,
    }
}

fn matcher_json(matcher: &RenderedMatcher) -> Value {
    match matcher {
        RenderedMatcher::Exact(value) => json!({ "kind": "exact", "value": value }),
        RenderedMatcher::Glob(value) => json!({ "kind": "glob", "value": value }),
        RenderedMatcher::Regex(value) => json!({ "kind": "regex", "value": value }),
        RenderedMatcher::Any => json!({ "kind": "any" }),
        RenderedMatcher::Unknown => json!({ "kind": "unknown" }),
    }
}

fn scalar_json(scalar: &RenderedScalar) -> Value {
    match scalar {
        RenderedScalar::Null => json!({ "type": "null", "value": null }),
        RenderedScalar::Boolean(value) => json!({ "type": "boolean", "value": value }),
        RenderedScalar::Integer(value) => json!({ "type": "integer", "value": value }),
        RenderedScalar::Float(value) => json!({ "type": "float", "value": value }),
        RenderedScalar::String(value) => json!({ "type": "string", "value": value }),
    }
}

#[cfg(test)]
mod tests {
    use super::diagnostic_json;
    use crate::diagnostics::RenderedDiagnostic;

    #[test]
    fn json_locations_preserve_unsigned_64_bit_values() {
        let line = u64::from(u32::MAX) + 1;
        let diagnostic = RenderedDiagnostic {
            id: "test".into(),
            message: "test".into(),
            source_path: "document.md".into(),
            line,
            column: line,
            target: None,
            schema_node: None,
            schema_location: None,
            involved_headers: Vec::new(),
            references: Vec::new(),
        };

        let json = diagnostic_json(&diagnostic);
        assert_eq!(json["location"]["line"].as_u64(), Some(line));
        assert_eq!(json["location"]["column"].as_u64(), Some(line));
    }
}
