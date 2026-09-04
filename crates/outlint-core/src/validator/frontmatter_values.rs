//! One document's frontmatter runtime view: the §1.6 JSON root, and what
//! every declared §2.3 capture evaluated to.
//!
//! The view is built once per document and read from everywhere. That is not
//! only an economy: §4.6 has one capture answer a proposition from every
//! constraint scope, and a value re-selected per reader could answer twice.
//!
//! Selection is a walk over the decoded components of the declaration's
//! singular query. It is deliberately not a second JSONPath evaluation: §2.3
//! already parsed the path, and the components arrive with their RFC escapes
//! decoded, so nothing here has to understand JSONPath string syntax. What it
//! does have to understand is RFC 6901, because §6.1 makes the pointer
//! Outlint's own rendering of the node's path components rather than any
//! provider's spelling.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::locator::SingularComponent;
use crate::typed_value::{
    parse_frontmatter, FrontmatterValue, ParseFailure, ResolvedYamlKind, ValueType,
};
use crate::{
    CaptureName, DocumentFrontmatter, FrontmatterAnchor, FrontmatterAnchors, FrontmatterCapture,
    FrontmatterCaptureView, FrontmatterLocation, SchemaNode,
};

use super::constraints::Truth;
use super::diagnostic::{
    Diagnostic, DiagnosticId, DiagnosticLocation, DiagnosticReference, DiagnosticTarget,
    FrontmatterBlock, FrontmatterLineRange,
};
use super::engine::BoundValueState;

/// What every declared frontmatter capture evaluated to for one document.
pub(super) struct FrontmatterValues<'d> {
    /// The §1.6 YAML-to-JSON view of the block, built once.
    ///
    /// `None` when there is no usable mapping — an absent block or an
    /// `invalid-frontmatter` one — which is also the condition under which
    /// §2.3 leaves every capture unevaluated.
    root: Option<Value>,
    /// Why captures could not be evaluated, when they could not.
    block: BlockSource,
    /// The block's extent and its entries' positions, for anchoring a
    /// diagnostic at the value it is about. Present only for a mapping, which
    /// is the only block anything inside it can be reported against.
    positions: Option<(FrontmatterLocation, &'d FrontmatterAnchors)>,
    entries: BTreeMap<CaptureName, CaptureEntry>,
}

/// The state of the block the captures were read from.
///
/// The two absent cases are kept apart because §4.6 reads them differently: an
/// absent *required* block suppresses a dependent constraint, while an absent
/// optional one is ordinary falsity.
enum BlockSource {
    Mapping,
    AbsentOptional,
    AbsentRequired,
    Invalid,
}

/// One declaration's retained outcome.
struct CaptureEntry {
    state: BoundValueState,
    required: bool,
}

impl FrontmatterValues<'_> {
    /// The frontmatter JSON root, or `None` when the block is unusable.
    pub(super) fn root(&self) -> Option<&Value> {
        self.root.as_ref()
    }

    /// Whether the block exists but is not a valid mapping.
    ///
    /// §4.6 parts the two unusable blocks: an `invalid-frontmatter` block
    /// leaves a query "unevaluated and the entire containing constraint
    /// suppressed", while an absent one "produces an empty result" and is
    /// merely unsatisfied.
    pub(super) fn block_is_invalid(&self) -> bool {
        matches!(self.block, BlockSource::Invalid)
    }

    /// Builds one diagnostic about a value inside the block.
    ///
    /// `pointer` names the entry (§6.1) and `anchor` is the pointer whose
    /// position the reader is sent to; the two differ for `missing-value`,
    /// where the named path does not exist and the position comes from its
    /// deepest resolving ancestor. `references` is what §11.3 calls the
    /// diagnostic's semantic reference data, and is empty for a diagnostic
    /// whose responsible declaration the schema node already names. Returns
    /// `None` when there is no mapping, which is also when nothing inside one
    /// can be reported.
    pub(super) fn entry_diagnostic(
        &self,
        id: DiagnosticId,
        pointer: Option<String>,
        anchor: &str,
        schema_node: SchemaNode,
        message: String,
        references: Vec<DiagnosticReference>,
    ) -> Option<Diagnostic> {
        let (location, anchors) = self.positions?;
        let position = enclosing_anchor(anchors, anchor);
        Some(Diagnostic {
            id,
            target: DiagnosticTarget::Frontmatter {
                block: Some(FrontmatterBlock {
                    line_range: FrontmatterLineRange {
                        start_line: location.start_line,
                        end_line: location.end_line,
                    },
                    json_pointer: pointer,
                }),
            },
            location: DiagnosticLocation {
                range: location.range,
                line: position.map_or(location.start_line, |anchor| anchor.line),
                column: position.map_or(1, |anchor| anchor.column),
            },
            schema_node: Some(schema_node),
            involved_headers: Vec::new(),
            references,
            message,
        })
    }

    /// What `fm.<name>` reads as (§4.6).
    ///
    /// The answer comes from the retained state rather than from the
    /// diagnostics that state produced, which is what §6.3 requires:
    /// suppressing `invalid-value`, `missing-value`, `missing-frontmatter`,
    /// or `invalid-frontmatter` "never re-enables a dependent constraint".
    pub(super) fn truth(&self, name: &CaptureName) -> Truth {
        let Some(entry) = self.entries.get(name) else {
            // Unreachable from a loaded schema: §4.6 makes an unknown capture
            // name `unresolved-ref` at load, so a bound `fm.<name>` always
            // names a declaration this view evaluated.
            return Truth::Unsatisfied;
        };
        match &entry.state {
            // "satisfied iff the capture is valid and bound, except that a
            // bound `bool` capture contributes its boolean value: a valid
            // bound `false` is unsatisfied."
            BoundValueState::Valid(value) => Truth::from_bool(value.as_bool() != Some(false)),
            BoundValueState::Invalid => Truth::Suppressed,
            // A required capture that selected nothing is §4.6's "missing
            // required capture"; an optional one is "ordinary falsity".
            BoundValueState::Absent => {
                if entry.required {
                    Truth::Suppressed
                } else {
                    Truth::Unsatisfied
                }
            }
            BoundValueState::Unevaluated => match self.block {
                BlockSource::Invalid | BlockSource::AbsentRequired => Truth::Suppressed,
                BlockSource::AbsentOptional | BlockSource::Mapping => Truth::Unsatisfied,
            },
        }
    }
}

/// One primary diagnostic a capture evaluation calls for (§2.3, §6.2).
///
/// The facts, not the wording: the engine owns diagnostic prose and the
/// anchoring of a frontmatter position, and it builds both from these.
pub(super) struct CaptureProblem {
    /// The declaration the diagnostic is attributed to.
    pub(super) name: CaptureName,
    /// The pointer the diagnostic names, when one can be formed.
    pub(super) pointer: Option<String>,
    /// The pointer whose position anchors the diagnostic: the failing node
    /// itself, or the deepest resolving ancestor of an absent path (§6.1).
    pub(super) anchor: String,
    /// What went wrong.
    pub(super) reason: CaptureFailure,
}

pub(super) enum CaptureFailure {
    /// The selected node did not parse as its declared type (§2.4).
    Invalid {
        value_type: ValueType,
        /// The node's own spelling, for the message.
        source: String,
        failure: ParseFailure,
    },
    /// A `required: true` capture selected no value (§2.3).
    Missing,
}

/// Evaluates every declared capture against one document's frontmatter.
///
/// §8 runs this once, after the block's own presence and JSON Schema checks
/// and before the outline walk. A `frontmatter-schema` failure does not reach
/// it: §2.3 keeps the two mechanisms independent, "because a valid resolved
/// mapping still exists".
pub(super) fn evaluate<'d>(
    declared: FrontmatterCaptureView<'_>,
    frontmatter: &'d DocumentFrontmatter,
    block_required: bool,
) -> (FrontmatterValues<'d>, Vec<CaptureProblem>) {
    let (block, mapping, positions) = match frontmatter {
        DocumentFrontmatter::Mapping {
            value,
            location,
            anchors,
        } => (
            BlockSource::Mapping,
            Some(value),
            Some((*location, anchors)),
        ),
        DocumentFrontmatter::Invalid { .. } => (BlockSource::Invalid, None, None),
        DocumentFrontmatter::Absent => (
            if block_required {
                BlockSource::AbsentRequired
            } else {
                BlockSource::AbsentOptional
            },
            None,
            None,
        ),
    };
    // One clone per document rather than one per proposition: §4.6 evaluates
    // queries against this same root, and the mapping is the document's, not
    // the view's, to keep borrowing.
    let root = mapping.map(|mapping| Value::Object(mapping.clone()));
    let mut entries = BTreeMap::new();
    let mut problems = Vec::new();
    for (name, declaration) in declared.iter() {
        // §2.3: "When the document has no frontmatter block, or its block is
        // `invalid-frontmatter`, captures are not evaluated and produce
        // neither `missing-value` nor `invalid-value`."
        let state = match root.as_ref() {
            Some(root) => evaluate_capture(root, name, declaration, &mut problems),
            None => BoundValueState::Unevaluated,
        };
        entries.insert(
            name.clone(),
            CaptureEntry {
                state,
                required: declaration.is_required(),
            },
        );
    }
    (
        FrontmatterValues {
            root,
            block,
            positions,
            entries,
        },
        problems,
    )
}

/// The position of the entry `pointer` names, or of the nearest enclosing
/// entry that has one.
///
/// §6.2 permits exactly this fallback — "the nearest enclosing entry that has
/// a position of its own — `/list/0` to `/list`, and `/list` to the block" —
/// and it is what makes an absent path anchor at its deepest resolving
/// ancestor, and an entry with no spelling of its own anchor at its container
/// rather than at some neighbour's text. Reference tokens are escaped, so no
/// token contains the `/` this splits on.
fn enclosing_anchor(anchors: &FrontmatterAnchors, pointer: &str) -> Option<FrontmatterAnchor> {
    let mut candidate = pointer;
    loop {
        if let Some(anchor) = anchors.get(candidate) {
            return Some(anchor);
        }
        match candidate.rsplit_once('/') {
            Some((parent, _)) => candidate = parent,
            None => return None,
        }
    }
}

fn evaluate_capture(
    root: &Value,
    name: &CaptureName,
    declaration: &FrontmatterCapture,
    problems: &mut Vec<CaptureProblem>,
) -> BoundValueState {
    let value_type = declaration.value_type();
    match select(root, declaration.path().components()) {
        Selection::Node { value, pointer } if !value.is_null() => {
            let supplied = FrontmatterValue::new(value, resolved_kind(value));
            match parse_frontmatter(value_type, supplied) {
                Ok(parsed) => BoundValueState::Valid(parsed),
                Err(failure) => {
                    problems.push(CaptureProblem {
                        name: name.clone(),
                        pointer: Some(pointer.clone()),
                        anchor: pointer,
                        reason: CaptureFailure::Invalid {
                            value_type,
                            source: scalar_spelling(value),
                            failure,
                        },
                    });
                    BoundValueState::Invalid
                }
            }
        }
        // §2.3: "No result node, or one null result node, is **absent**." A
        // null node is spelled in the document, so it anchors and is named by
        // its own pointer.
        Selection::Node { pointer, .. } => {
            absent(name, declaration, Some(pointer.clone()), pointer, problems)
        }
        Selection::Absent { target, anchor } => absent(name, declaration, target, anchor, problems),
    }
}

fn absent(
    name: &CaptureName,
    declaration: &FrontmatterCapture,
    pointer: Option<String>,
    anchor: String,
    problems: &mut Vec<CaptureProblem>,
) -> BoundValueState {
    // §2.3: absence "produces `missing-value` exactly when that capture has
    // `required: true`; an optional absent capture is valid and unbound."
    if declaration.is_required() {
        problems.push(CaptureProblem {
            name: name.clone(),
            pointer,
            anchor,
            reason: CaptureFailure::Missing,
        });
    }
    BoundValueState::Absent
}

/// What a declaration's singular query selected, in RFC 6901 terms.
enum Selection<'a> {
    /// The query selected one node, at `pointer`.
    Node { value: &'a Value, pointer: String },
    /// The query selected nothing.
    Absent {
        /// The normalized path it addressed, when one can be formed (§6.1).
        target: Option<String>,
        /// The deepest ancestor that did resolve, as the anchoring floor.
        anchor: String,
    },
}

/// Walks a §2.3 singular query's decoded components over the JSON root.
///
/// Two pointers are built at once, and they differ only past the point where
/// the walk stops resolving: `anchor` follows the nodes that exist, while
/// `target` keeps spelling the path that was addressed. §6.1 wants both — the
/// pointer names the intended absent path, the position comes from the
/// deepest resolving ancestor — and neither can be recovered from the other.
fn select<'a>(root: &'a Value, components: &[SingularComponent]) -> Selection<'a> {
    let mut current = Some(root);
    let mut anchor = String::new();
    let mut target = Some(String::new());
    for component in components {
        let token = normalized_token(component, current);
        match (&token, target.as_mut()) {
            (Some(token), Some(path)) => {
                path.push('/');
                path.push_str(token);
            }
            // A negative index whose array is unknown names no position, so
            // no normalized path exists past this point.
            (None, _) => target = None,
            (Some(_), None) => {}
        }
        match (current.and_then(|node| descend(node, component)), &token) {
            (Some(next), Some(token)) => {
                anchor.push('/');
                anchor.push_str(token);
                current = Some(next);
            }
            _ => current = None,
        }
    }
    match current {
        Some(value) => Selection::Node {
            value,
            pointer: anchor,
        },
        None => Selection::Absent { target, anchor },
    }
}

/// The RFC 6901 reference token one component contributes, or `None` when it
/// names no position in this document.
fn normalized_token(component: &SingularComponent, current: Option<&Value>) -> Option<String> {
    match component {
        SingularComponent::Name(name) => Some(pointer_token(name)),
        SingularComponent::Index(index) if *index >= 0 => Some(index.to_string()),
        // §2.3 keeps negative indices declarative: one counts back from the
        // end, so it names a position only where the concrete array it counts
        // back through is known and long enough.
        SingularComponent::Index(index) => match current {
            Some(Value::Array(items)) => from_end(items.len(), *index).map(|at| at.to_string()),
            _ => None,
        },
    }
}

fn descend<'a>(node: &'a Value, component: &SingularComponent) -> Option<&'a Value> {
    match (node, component) {
        (Value::Object(map), SingularComponent::Name(name)) => map.get(name.as_ref()),
        (Value::Array(items), SingularComponent::Index(index)) => {
            let position = if *index >= 0 {
                usize::try_from(*index).ok()?
            } else {
                from_end(items.len(), *index)?
            };
            items.get(position)
        }
        // §2.3: "Traversal through a value of the wrong container kind
        // produces an empty nodelist under RFC 9535 and is therefore absence,
        // not a separate traversal error."
        _ => None,
    }
}

/// Normalizes a negative index against a concrete array length.
fn from_end(length: usize, index: i64) -> Option<usize> {
    let normalized = i64::try_from(length).ok()?.checked_add(index)?;
    usize::try_from(normalized).ok()
}

/// Escapes one RFC 6901 reference token: `~` as `~0` and `/` as `~1`.
fn pointer_token(name: &str) -> String {
    let mut token = String::with_capacity(name.len());
    for character in name.chars() {
        match character {
            '~' => token.push_str("~0"),
            '/' => token.push_str("~1"),
            other => token.push(other),
        }
    }
    token
}

/// The YAML kind a node of the §1.6 JSON view resolved to.
///
/// §2.4's strict capture check needs the YAML kind, which the JSON view
/// cannot carry, and every kind but one is the JSON variant itself. The
/// exception is a number: §1.6's conversion keeps its exact source spelling,
/// and a fraction or an exponent is what separates a YAML finite decimal from
/// a YAML integer once both have become JSON numbers. A tag does not enter
/// into it — §2.3 gives an unrecognized tag the kind its text resolves to
/// under the YAML 1.2 core schema, which is the kind the conversion already
/// applied.
fn resolved_kind(value: &Value) -> ResolvedYamlKind {
    match value {
        Value::Null => ResolvedYamlKind::Null,
        Value::Bool(_) => ResolvedYamlKind::Boolean,
        Value::Number(number) => {
            if is_whole_number(number.as_str()) {
                ResolvedYamlKind::Integer
            } else {
                ResolvedYamlKind::Float
            }
        }
        Value::String(_) => ResolvedYamlKind::String,
        Value::Array(_) => ResolvedYamlKind::Sequence,
        Value::Object(_) => ResolvedYamlKind::Mapping,
    }
}

fn is_whole_number(spelling: &str) -> bool {
    let digits = spelling.strip_prefix('-').unwrap_or(spelling);
    !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
}

/// A node's own spelling, for a message that quotes the value it rejected.
///
/// A collection has none to quote: the failure there is about its kind, and
/// rendering a mapping as text would be the coercion §2.4 forbids.
fn scalar_spelling(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.as_str().to_owned(),
        Value::Bool(boolean) => boolean.to_string(),
        Value::Null => "null".to_owned(),
        Value::Array(_) | Value::Object(_) => String::new(),
    }
}
