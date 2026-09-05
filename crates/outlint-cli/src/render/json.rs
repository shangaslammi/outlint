//! The version 4 JSON output envelope and its field conversions.

use serde_json::{json, Map, Value};

/// The envelope version this build emits.
///
/// §11.3 fixes the number and requires consumers to reject versions they do
/// not know rather than reading them as an older shape, so Typed Values is a
/// hard cut from 3 to 4: there is no second emission path, no negotiation,
/// and no `json-v2` format name to fall back to.
const ENVELOPE_VERSION: u64 = 4;

use crate::diagnostics::{
    RenderedDiagnostic, RenderedMatcher, RenderedPosition, RenderedReference, RenderedScalar,
    RenderedSchemaNode, RenderedTarget, ResultKind, ValidationResult,
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
            "version": ENVELOPE_VERSION,
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
        RenderedSchemaNode::Guard { scope, index } => {
            json!({ "kind": "guard", "scope": scope, "index": index })
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
/// integers; consumers MUST NOT assume they fit a 64-bit integer type", so a
/// subscript becomes a JSON *number* and never a quoted string. The number is
/// the one [`RenderedPosition`] already holds, so nothing is parsed, nothing
/// is narrowed, and there is no failure to fall back from: null here means
/// only what §11.3 says it means, an unsubscripted step in an otherwise
/// subscripted path.
fn position_json(position: &Option<RenderedPosition>) -> Value {
    match position {
        Some(position) => Value::Number(position.as_number().clone()),
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
    use serde_json::{json, Value};

    use super::{
        diagnostic_json, matcher_json, reference_json, render_json, scalar_json, schema_node_json,
        target_json,
    };
    use crate::diagnostics::{
        RenderedDiagnostic, RenderedInvolvedHeader, RenderedLineRange, RenderedLocation,
        RenderedMatcher, RenderedPosition, RenderedReference, RenderedScalar, RenderedSchemaNode,
        RenderedTarget, ResultKind, ValidationResult,
    };

    /// A `[i]` subscript far beyond `u64::MAX`, as §11.3's "consumers MUST NOT
    /// assume they fit a 64-bit integer type" requires to be representable.
    /// This is 2^128, whose exact decimal spelling must survive rendering.
    const ABOVE_U64: &str = "340282366920938463463374607431768211456";

    fn position(digits: &str) -> RenderedPosition {
        RenderedPosition::from_canonical_digits(digits.to_owned())
    }

    fn rule_coordinates() -> (Vec<usize>, usize) {
        (vec![1, 0], 2)
    }

    /// Compares each rendered value against its complete §11.3 fixture.
    ///
    /// Whole-object equality is the point: a member probe passes when a member
    /// is missing, extra, renamed, or emitted as null instead of omitted,
    /// which is exactly the class of drift these tables exist to catch.
    fn assert_exact<T>(render: impl Fn(&T) -> Value, fixtures: Vec<(T, Value)>) {
        for (value, expected) in fixtures {
            assert_eq!(render(&value), expected);
        }
    }

    /// §6.1's four target kinds and every optional-member combination the
    /// `frontmatter` kind admits. Absent `line_range` and absent `pointer` are
    /// omitted rather than emitted as null, and `Some("")` — the root pointer
    /// naming the mapping itself — stays distinct from `None`.
    #[test]
    fn target_shapes_match_their_version_4_fixtures_exactly() {
        let range = || {
            Some(RenderedLineRange {
                start_line: 1,
                end_line: 3,
            })
        };
        assert_exact(
            target_json,
            vec![
                (
                    RenderedTarget::Header {
                        path: vec!["A".into(), "B".into()],
                    },
                    json!({"kind": "header", "path": ["A", "B"]}),
                ),
                (
                    RenderedTarget::MissingHeader {
                        parent: vec!["A".into()],
                        matcher: "Step *".into(),
                    },
                    json!({"kind": "missing_header", "parent": ["A"], "matcher": "Step *"}),
                ),
                (RenderedTarget::Document, json!({"kind": "document"})),
                (
                    RenderedTarget::Frontmatter {
                        line_range: None,
                        pointer: None,
                    },
                    json!({"kind": "frontmatter"}),
                ),
                (
                    RenderedTarget::Frontmatter {
                        line_range: range(),
                        pointer: None,
                    },
                    json!({
                        "kind": "frontmatter",
                        "line_range": {"start_line": 1, "end_line": 3}
                    }),
                ),
                (
                    RenderedTarget::Frontmatter {
                        line_range: range(),
                        pointer: Some(String::new()),
                    },
                    json!({
                        "kind": "frontmatter",
                        "line_range": {"start_line": 1, "end_line": 3},
                        "pointer": ""
                    }),
                ),
                (
                    RenderedTarget::Frontmatter {
                        line_range: range(),
                        pointer: Some("/release".into()),
                    },
                    json!({
                        "kind": "frontmatter",
                        "line_range": {"start_line": 1, "end_line": 3},
                        "pointer": "/release"
                    }),
                ),
            ],
        );
    }

    /// Every §11.3 `schema_node` kind spelling, with the coordinates each one
    /// retains: rule and constraint keep `scope` and `index`, `capture` adds
    /// `name` to them, `order_entry` adds `order_index`, and
    /// `frontmatter_capture` has a `name` and no rule coordinates at all.
    #[test]
    fn schema_node_shapes_match_their_version_4_fixtures_exactly() {
        let (scope, index) = rule_coordinates();
        assert_exact(
            schema_node_json,
            vec![
                (RenderedSchemaNode::Title, json!({"kind": "title"})),
                (
                    RenderedSchemaNode::Frontmatter,
                    json!({"kind": "frontmatter"}),
                ),
                (
                    RenderedSchemaNode::FrontmatterSchemaDeclaration,
                    json!({"kind": "frontmatter_schema_declaration"}),
                ),
                (
                    RenderedSchemaNode::FrontmatterSchemaDocument,
                    json!({"kind": "frontmatter_schema_document"}),
                ),
                (
                    RenderedSchemaNode::Rule {
                        scope: scope.clone(),
                        index,
                    },
                    json!({"kind": "rule", "scope": [1, 0], "index": 2}),
                ),
                (
                    RenderedSchemaNode::Guard {
                        scope: scope.clone(),
                        index,
                    },
                    json!({"kind": "guard", "scope": [1, 0], "index": 2}),
                ),
                (
                    RenderedSchemaNode::Capture {
                        scope: scope.clone(),
                        index,
                        name: "version".into(),
                    },
                    json!({"kind": "capture", "scope": [1, 0], "index": 2, "name": "version"}),
                ),
                (
                    RenderedSchemaNode::FrontmatterCapture {
                        name: "release".into(),
                    },
                    json!({"kind": "frontmatter_capture", "name": "release"}),
                ),
                (
                    RenderedSchemaNode::OrderEntry {
                        scope: scope.clone(),
                        index,
                        order_index: 1,
                    },
                    json!({
                        "kind": "order_entry",
                        "scope": [1, 0],
                        "index": 2,
                        "order_index": 1
                    }),
                ),
                (
                    RenderedSchemaNode::Constraint { scope, index },
                    json!({"kind": "constraint", "scope": [1, 0], "index": 2}),
                ),
            ],
        );
    }

    /// The three §11.3 reference kinds with their members in declaration
    /// order, including both optional members in each state: a rule without
    /// `positions` and one with them, and a frontmatter query without and with
    /// `equals`.
    #[test]
    fn reference_shapes_match_their_version_4_fixtures_exactly() {
        assert_exact(
            reference_json,
            vec![
                (
                    RenderedReference::Rule {
                        locator: "release".into(),
                        anchor: "current_scope",
                        path: vec!["release".into()],
                        positions: None,
                        matcher: RenderedMatcher::Exact("Release".into()),
                    },
                    json!({
                        "kind": "rule",
                        "locator": "release",
                        "anchor": "current_scope",
                        "path": ["release"],
                        "matcher": {"kind": "exact", "value": "Release"}
                    }),
                ),
                (
                    RenderedReference::Rule {
                        locator: format!("$.release[{ABOVE_U64}].notes"),
                        anchor: "schema_root",
                        path: vec!["release".into(), "notes".into()],
                        positions: Some(vec![Some(position(ABOVE_U64)), None]),
                        matcher: RenderedMatcher::Exact("Notes".into()),
                    },
                    json!({
                        "kind": "rule",
                        "locator": format!("$.release[{ABOVE_U64}].notes"),
                        "anchor": "schema_root",
                        "path": ["release", "notes"],
                        "positions": [Value::Number(ABOVE_U64.parse().expect("a JSON number")), null],
                        "matcher": {"kind": "exact", "value": "Notes"}
                    }),
                ),
                (
                    RenderedReference::FrontmatterQuery {
                        locator: "fm[$.draft]".into(),
                        query: "$.draft".into(),
                        equals: None,
                    },
                    json!({
                        "kind": "frontmatter_query",
                        "locator": "fm[$.draft]",
                        "query": "$.draft"
                    }),
                ),
                (
                    RenderedReference::FrontmatterQuery {
                        locator: "fm[$.count]=0x10".into(),
                        query: "$.count".into(),
                        equals: Some(RenderedScalar::Integer("16".into())),
                    },
                    json!({
                        "kind": "frontmatter_query",
                        "locator": "fm[$.count]=0x10",
                        "query": "$.count",
                        "equals": {"type": "integer", "value": "16"}
                    }),
                ),
                (
                    RenderedReference::FrontmatterCapture {
                        locator: "fm.version".into(),
                        name: "version".into(),
                        value_type: "semver".into(),
                    },
                    json!({
                        "kind": "frontmatter_capture",
                        "locator": "fm.version",
                        "name": "version",
                        "type": "semver"
                    }),
                ),
            ],
        );
    }

    /// A subscript wider than `u64` reaches the wire as an unquoted JSON
    /// integer with every digit intact.
    ///
    /// The serialized text is asserted rather than only the parsed value,
    /// because the two failures worth catching are invisible to a `Value`
    /// comparison built the same wrong way: quoting the digits, and rounding
    /// them through a float, which would print `3.402823669209385e38`.
    #[test]
    fn positions_above_u64_are_emitted_as_unquoted_json_integers() {
        let reference = RenderedReference::Rule {
            locator: format!("$.release[{ABOVE_U64}].notes"),
            anchor: "schema_root",
            path: vec!["release".into(), "notes".into()],
            positions: Some(vec![Some(position(ABOVE_U64)), None]),
            matcher: RenderedMatcher::Any,
        };

        let rendered = reference_json(&reference);
        let text = serde_json::to_string(&rendered["positions"]).expect("renderable");
        // Unquoted, every digit intact, and not rounded through a float, which
        // would print `3.402823669209385e38` instead.
        assert_eq!(text, format!("[{ABOVE_U64},null]"));
        assert!(rendered["positions"][0].is_number());
        assert!(!rendered["positions"][0].is_f64());
    }

    /// §11.3: `positions` is present "when any name step has positional
    /// narrowing", and is then aligned with `path` throughout — one entry per
    /// step, null for the unsubscripted ones. A wholly unsubscripted path has
    /// no `positions` member at all.
    #[test]
    fn positions_are_omitted_entirely_or_aligned_with_every_path_step() {
        let render = |positions: Option<Vec<Option<RenderedPosition>>>| {
            reference_json(&RenderedReference::Rule {
                locator: "a.b.c".into(),
                anchor: "current_scope",
                path: vec!["a".into(), "b".into(), "c".into()],
                positions,
                matcher: RenderedMatcher::Any,
            })
        };

        assert!(
            render(None).get("positions").is_none(),
            "an unsubscripted path carries no `positions` member"
        );
        assert_eq!(
            render(Some(vec![None, Some(position("7")), None]))["positions"],
            json!([null, 7, null])
        );
    }

    /// §11.3 names four matcher kinds; the first three carry `value` and
    /// `any` carries nothing.
    #[test]
    fn matcher_shapes_match_their_version_4_fixtures_exactly() {
        assert_exact(
            matcher_json,
            vec![
                (
                    RenderedMatcher::Exact("Release".into()),
                    json!({"kind": "exact", "value": "Release"}),
                ),
                (
                    RenderedMatcher::Glob("Release *".into()),
                    json!({"kind": "glob", "value": "Release *"}),
                ),
                (
                    RenderedMatcher::Regex("Release (?<version>.+)".into()),
                    json!({"kind": "regex", "value": "Release (?<version>.+)"}),
                ),
                (RenderedMatcher::Any, json!({"kind": "any"})),
            ],
        );
    }

    /// The five §11.3 `equals` types. Integer and float values are canonical
    /// *strings*, not JSON numbers: an Outlint integer has no upper bound and
    /// a float's spelling is significant, so neither survives a JSON number.
    /// The other three use their corresponding JSON types.
    #[test]
    fn equality_scalar_shapes_match_their_version_4_fixtures_exactly() {
        assert_exact(
            scalar_json,
            vec![
                (RenderedScalar::Null, json!({"type": "null", "value": null})),
                (
                    RenderedScalar::Boolean(true),
                    json!({"type": "boolean", "value": true}),
                ),
                (
                    RenderedScalar::Integer("16".into()),
                    json!({"type": "integer", "value": "16"}),
                ),
                (
                    RenderedScalar::Float("15e-1".into()),
                    json!({"type": "float", "value": "15e-1"}),
                ),
                (
                    RenderedScalar::String("release".into()),
                    json!({"type": "string", "value": "release"}),
                ),
            ],
        );
    }

    fn skeleton(id: &str) -> RenderedDiagnostic {
        RenderedDiagnostic {
            id: id.into(),
            message: "explanatory prose".into(),
            source_path: "doc.md".into(),
            line: 1,
            column: 1,
            target: None,
            schema_node: None,
            schema_location: None,
            involved_headers: Vec::new(),
            references: Vec::new(),
        }
    }

    /// §11.3: `id`, `message`, and `location` are always present; every other
    /// member appears only when the corresponding semantic data exists.
    /// A schema-load error has none of them, so it renders as the bare
    /// skeleton — and in particular omits `target`, which §6 requires of a
    /// diagnostic about the schema file rather than about a document.
    #[test]
    fn a_diagnostic_without_optional_data_renders_as_the_bare_skeleton() {
        assert_eq!(
            diagnostic_json(&skeleton("diagnostic-id")),
            json!({
                "id": "diagnostic-id",
                "message": "explanatory prose",
                "location": {"line": 1, "column": 1}
            })
        );
    }

    /// The same skeleton with every optional member's data present, so the
    /// full member set is pinned alongside the empty one. Empty optional
    /// *collections* are omitted, which the skeleton above already shows.
    #[test]
    fn a_diagnostic_with_every_optional_member_renders_all_of_them() {
        let (scope, index) = rule_coordinates();
        let diagnostic = RenderedDiagnostic {
            target: Some(RenderedTarget::Header {
                path: vec!["Parent".into(), "Child".into()],
            }),
            schema_node: Some(RenderedSchemaNode::Capture {
                scope,
                index,
                name: "version".into(),
            }),
            schema_location: Some(RenderedLocation {
                path: "schema.yml".into(),
                line: 7,
                column: 9,
            }),
            involved_headers: vec![RenderedInvolvedHeader {
                header_path: vec!["Parent".into(), "Child".into()],
                line: 4,
                column: 1,
            }],
            references: vec![RenderedReference::FrontmatterCapture {
                locator: "fm.version".into(),
                name: "version".into(),
                value_type: "semver".into(),
            }],
            ..skeleton("invalid-value")
        };

        assert_eq!(
            diagnostic_json(&diagnostic),
            json!({
                "id": "invalid-value",
                "message": "explanatory prose",
                "location": {"line": 1, "column": 1},
                "target": {"kind": "header", "path": ["Parent", "Child"]},
                "schema_node": {
                    "kind": "capture",
                    "scope": [1, 0],
                    "index": 2,
                    "name": "version"
                },
                "schema_location": {"path": "schema.yml", "line": 7, "column": 9},
                "involved_headers": [{
                    "header_path": ["Parent", "Child"],
                    "location": {"line": 4, "column": 1}
                }],
                "references": [{
                    "kind": "frontmatter_capture",
                    "locator": "fm.version",
                    "name": "version",
                    "type": "semver"
                }]
            })
        );
    }

    /// The whole §11.3 envelope, compared as one value.
    #[test]
    fn the_envelope_matches_its_version_4_fixture_exactly() {
        let rendered = render_json(&[ValidationResult {
            kind: ResultKind::Document,
            path: "doc.md".into(),
            schema: "schema.yml".into(),
            diagnostics: Vec::new(),
        }]);
        assert!(rendered.ends_with('\n'), "one line, terminated");
        assert_eq!(
            serde_json::from_str::<Value>(&rendered).expect("one JSON document"),
            json!({
                "version": 4,
                "results": [{
                    "kind": "document",
                    "path": "doc.md",
                    "schema": "schema.yml",
                    "diagnostics": []
                }],
                "summary": {
                    "files": 1,
                    "documents": 1,
                    "schemas": 0,
                    "diagnostics": 0
                }
            })
        );
    }

    /// `RenderedMatcher::Unknown` is not a §11.3 kind and no `Matcher` in
    /// existence renders to it. It is the catch-all the `#[non_exhaustive]`
    /// core enum forces the CLI to write, kept so a matcher form added
    /// upstream cannot make this crate fail to build; the assertion records
    /// that it stays out of the four specified spellings.
    #[test]
    fn the_non_exhaustive_matcher_fallback_is_not_one_of_the_specified_kinds() {
        let kind = matcher_json(&RenderedMatcher::Unknown)["kind"].clone();
        assert!(!["exact", "glob", "regex", "any"].contains(&kind.as_str().expect("a kind")));
    }

    #[test]
    fn json_locations_preserve_unsigned_64_bit_values() {
        let line = u64::from(u32::MAX) + 1;
        let diagnostic = RenderedDiagnostic {
            line,
            column: line,
            ..skeleton("test")
        };

        let json = diagnostic_json(&diagnostic);
        assert_eq!(json["location"]["line"].as_u64(), Some(line));
        assert_eq!(json["location"]["column"].as_u64(), Some(line));
    }
}
