//! The CLI's rendering-ready diagnostic model.
//!
//! Core schema and document diagnostics are converted here into the flat
//! `Rendered*` shapes both output formats consume, together with schema source
//! locations and the total per-file ordering the JSON contract promises.

use outlint_core::{
    Diagnostic, DiagnosticReference, DiagnosticTarget, FrontmatterRef, FrontmatterScalar,
    InvalidSchema, LoadedSchema, Matcher, RefAnchor, RuleRef, SchemaError, SchemaLocations,
    SchemaNode, SchemaSources, SourceRange,
};

#[derive(Debug)]
pub(crate) struct InvocationOutput {
    pub(crate) results: Vec<ValidationResult>,
    pub(crate) operational_errors: Vec<String>,
}

#[derive(Debug)]
pub(crate) struct ValidationResult {
    pub(crate) kind: ResultKind,
    pub(crate) path: String,
    pub(crate) schema: String,
    pub(crate) diagnostics: Vec<RenderedDiagnostic>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResultKind {
    Document,
    Schema,
}

#[derive(Debug)]
pub(crate) struct RenderedDiagnostic {
    pub(crate) id: String,
    pub(crate) message: String,
    pub(crate) source_path: String,
    pub(crate) line: u64,
    pub(crate) column: u64,
    /// What the diagnostic is about. Absent for schema-load errors, which are
    /// about the schema file rather than anything inside a document.
    pub(crate) target: Option<RenderedTarget>,
    pub(crate) schema_node: Option<RenderedSchemaNode>,
    pub(crate) schema_location: Option<RenderedLocation>,
    pub(crate) involved_headers: Vec<RenderedInvolvedHeader>,
    pub(crate) references: Vec<RenderedReference>,
}

/// The rendering of [`DiagnosticTarget`], one variant per kind.
///
/// The variants are kept apart rather than flattened into one path because the
/// text they carry has different provenance: only [`Self::Header`] names text
/// that occurs in the document, [`Self::MissingHeader`]'s matcher is schema
/// text, and the remaining two name no header at all.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum RenderedTarget {
    Header {
        path: Vec<String>,
    },
    MissingHeader {
        parent: Vec<String>,
        matcher: String,
    },
    Document,
    Frontmatter {
        /// Absent when the document has no frontmatter block at all.
        line_range: Option<RenderedLineRange>,
        /// `Some("")` is the root JSON Pointer; `None` is no pointer at all.
        pointer: Option<String>,
    },
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct RenderedLineRange {
    pub(crate) start_line: u64,
    pub(crate) end_line: u64,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct RenderedLocation {
    pub(crate) path: String,
    pub(crate) line: u64,
    pub(crate) column: u64,
}

/// The rendering of [`SchemaNode`], in the §11.3 `kind` declaration order.
///
/// The variant order is load-bearing twice over: it is the order §11.3 lists
/// the `kind` spellings in, and the derived [`Ord`] is what the JSON total
/// ordering compares schema nodes by. New variants belong where §11.3 puts
/// them, never appended for convenience.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum RenderedSchemaNode {
    Title,
    Frontmatter,
    FrontmatterSchemaDeclaration,
    FrontmatterSchemaDocument,
    Rule {
        scope: Vec<usize>,
        index: usize,
    },
    /// A rule capture: its owning rule's coordinates plus the capture name.
    Capture {
        scope: Vec<usize>,
        index: usize,
        name: String,
    },
    /// A frontmatter capture, which has a name and no rule coordinates: its
    /// named scope is rooted at `fm` rather than at any rule.
    FrontmatterCapture {
        name: String,
    },
    /// One `order` entry: its owning rule's coordinates plus its position.
    OrderEntry {
        scope: Vec<usize>,
        index: usize,
        order_index: usize,
    },
    Constraint {
        scope: Vec<usize>,
        index: usize,
    },
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct RenderedInvolvedHeader {
    pub(crate) header_path: Vec<String>,
    pub(crate) line: u64,
    pub(crate) column: u64,
}

/// The rendering of [`DiagnosticReference`], compatibility forms first.
///
/// [`Self::Rule`] and [`Self::Frontmatter`] render the pre-Typed-Values
/// references validation still emits; the three that follow render the final
/// §11.3 reference kinds and are unreachable until constraint binding cuts
/// over. Keeping them apart is what stops a half-migrated reference from
/// being emitted as if it were complete. The variant order is the derived
/// [`Ord`] the JSON total ordering compares references by, so the
/// compatibility pair keeps its position rather than being reshuffled.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum RenderedReference {
    Rule {
        anchor: &'static str,
        path: Vec<String>,
        matcher: RenderedMatcher,
    },
    Frontmatter {
        path: Vec<String>,
        equals: Option<RenderedScalar>,
    },
    /// §11.3 kind `rule`, with its members in declaration order.
    ResolvedRule {
        locator: String,
        anchor: &'static str,
        path: Vec<String>,
        /// Aligned with `path`, present only when some step is subscripted.
        ///
        /// Each entry is a `[i]` subscript in canonical decimal, or `None`
        /// for an unsubscripted step. Decimal text rather than an integer
        /// because §4.4 gives `i` no upper bound; §11.3 requires it to be
        /// serialized as an arbitrary-precision JSON *number*, never a
        /// quoted string.
        positions: Option<Vec<Option<String>>>,
        matcher: RenderedMatcher,
    },
    /// §11.3 kind `frontmatter_query`, with its members in declaration order.
    FrontmatterQuery {
        locator: String,
        /// The RFC 9535 query without its `fm[...]` wrapper.
        query: String,
        equals: Option<RenderedScalar>,
    },
    /// §11.3 kind `frontmatter_capture`, with its members in declaration
    /// order.
    FrontmatterCapture {
        locator: String,
        name: String,
        /// One of the §2.4 type names.
        value_type: String,
    },
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum RenderedMatcher {
    Exact(String),
    Glob(String),
    Regex(String),
    Any,
    Unknown,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum RenderedScalar {
    Null,
    Boolean(bool),
    Integer(String),
    Float(String),
    String(String),
}

pub(crate) fn render_schema_errors(
    invalid: &InvalidSchema,
    fallback_path: &str,
) -> Vec<RenderedDiagnostic> {
    invalid
        .errors
        .iter()
        .map(|error| render_schema_error(error, &invalid.sources, fallback_path))
        .collect()
}

fn render_schema_error(
    error: &SchemaError,
    sources: &SchemaSources,
    fallback_path: &str,
) -> RenderedDiagnostic {
    let location = source_location(sources, error.range, fallback_path);
    RenderedDiagnostic {
        id: error.kind.as_str().to_owned(),
        message: error.message.clone(),
        source_path: location.path.clone(),
        line: location.line,
        column: location.column,
        target: None,
        schema_node: None,
        schema_location: Some(location),
        involved_headers: Vec::new(),
        references: Vec::new(),
    }
}

pub(crate) fn render_document_diagnostic(
    document_path: &str,
    diagnostic: &Diagnostic,
    loaded: &LoadedSchema,
) -> RenderedDiagnostic {
    let schema_location = diagnostic.schema_node.as_ref().and_then(|node| {
        schema_node_location(node, &loaded.locations)
            .map(|range| source_location(&loaded.sources, range, "<schema>"))
    });
    RenderedDiagnostic {
        id: diagnostic.id.as_str().to_owned(),
        message: diagnostic.message.clone(),
        source_path: document_path.to_owned(),
        line: diagnostic.location.line,
        column: diagnostic.location.column,
        target: Some(render_target(&diagnostic.target)),
        schema_node: diagnostic.schema_node.as_ref().map(render_schema_node),
        schema_location,
        involved_headers: diagnostic
            .involved_headers
            .iter()
            .map(|header| RenderedInvolvedHeader {
                header_path: header.path.0.clone(),
                line: header.location.line,
                column: header.location.column,
            })
            .collect(),
        references: diagnostic.references.iter().map(render_reference).collect(),
    }
}

fn render_target(target: &DiagnosticTarget) -> RenderedTarget {
    match target {
        DiagnosticTarget::Header(path) => RenderedTarget::Header {
            path: path.0.clone(),
        },
        DiagnosticTarget::MissingHeader { parent, matcher } => RenderedTarget::MissingHeader {
            parent: parent.0.clone(),
            matcher: matcher.clone(),
        },
        DiagnosticTarget::Document => RenderedTarget::Document,
        DiagnosticTarget::Frontmatter { block } => RenderedTarget::Frontmatter {
            line_range: block.as_ref().map(|block| RenderedLineRange {
                start_line: block.line_range.start_line,
                end_line: block.line_range.end_line,
            }),
            pointer: block.as_ref().and_then(|block| block.json_pointer.clone()),
        },
    }
}

fn render_schema_node(node: &SchemaNode) -> RenderedSchemaNode {
    match node {
        SchemaNode::Title => RenderedSchemaNode::Title,
        SchemaNode::Frontmatter => RenderedSchemaNode::Frontmatter,
        SchemaNode::FrontmatterSchemaDeclaration => {
            RenderedSchemaNode::FrontmatterSchemaDeclaration
        }
        SchemaNode::FrontmatterSchemaDocument => RenderedSchemaNode::FrontmatterSchemaDocument,
        SchemaNode::Rule(path) => RenderedSchemaNode::Rule {
            scope: path.scope.0.iter().map(|index| index.0).collect(),
            index: path.index.0,
        },
        SchemaNode::Capture(path) => RenderedSchemaNode::Capture {
            scope: path.rule.scope.0.iter().map(|index| index.0).collect(),
            index: path.rule.index.0,
            name: path.name.as_str().to_owned(),
        },
        SchemaNode::FrontmatterCapture(name) => RenderedSchemaNode::FrontmatterCapture {
            name: name.as_str().to_owned(),
        },
        SchemaNode::OrderEntry(path) => RenderedSchemaNode::OrderEntry {
            scope: path.rule.scope.0.iter().map(|index| index.0).collect(),
            index: path.rule.index.0,
            order_index: path.order_index.0,
        },
        SchemaNode::Constraint(path) => RenderedSchemaNode::Constraint {
            scope: path.scope.0.iter().map(|index| index.0).collect(),
            index: path.index.0,
        },
    }
}

fn render_reference(reference: &DiagnosticReference) -> RenderedReference {
    match reference {
        DiagnosticReference::Rule { reference, matcher } => RenderedReference::Rule {
            anchor: render_anchor(reference.anchor),
            path: non_empty_rule_path(reference),
            matcher: render_matcher(matcher),
        },
        DiagnosticReference::Frontmatter(reference) => RenderedReference::Frontmatter {
            path: non_empty_frontmatter_path(reference),
            equals: reference.equals.as_ref().map(render_scalar),
        },
        DiagnosticReference::ResolvedRule { locator, matcher } => {
            let steps = locator.steps().iter().collect::<Vec<_>>();
            let positions = steps
                .iter()
                .map(|step| step.position_digits())
                .collect::<Vec<_>>();
            RenderedReference::ResolvedRule {
                locator: locator.locator().to_owned(),
                anchor: render_anchor(locator.anchor()),
                path: steps
                    .iter()
                    .map(|step| step.id().as_str().to_owned())
                    .collect(),
                // §11.3: the array is present only when some step carries a
                // subscript, and is then aligned with `path` throughout.
                positions: positions.iter().any(Option::is_some).then_some(positions),
                matcher: render_matcher(matcher),
            }
        }
        DiagnosticReference::FrontmatterQuery(reference) => RenderedReference::FrontmatterQuery {
            locator: reference.locator().to_owned(),
            query: reference.query().to_owned(),
            equals: reference.equals().map(render_scalar),
        },
        DiagnosticReference::FrontmatterCapture(reference) => {
            RenderedReference::FrontmatterCapture {
                locator: reference.locator().to_owned(),
                name: reference.name().as_str().to_owned(),
                value_type: reference.type_name().to_owned(),
            }
        }
    }
}

fn render_anchor(anchor: RefAnchor) -> &'static str {
    match anchor {
        RefAnchor::CurrentScope => "current_scope",
        RefAnchor::SchemaRoot => "schema_root",
    }
}

fn non_empty_rule_path(reference: &RuleRef) -> Vec<String> {
    reference
        .path
        .iter()
        .map(|id| id.as_str().to_owned())
        .collect()
}

fn non_empty_frontmatter_path(reference: &FrontmatterRef) -> Vec<String> {
    reference
        .path
        .iter()
        .map(|key| key.as_str().to_owned())
        .collect()
}

fn render_matcher(matcher: &Matcher) -> RenderedMatcher {
    match matcher {
        Matcher::Exact(value) => RenderedMatcher::Exact(value.0.clone()),
        Matcher::Glob(value) => RenderedMatcher::Glob(value.as_str().to_owned()),
        Matcher::Regex(value) => RenderedMatcher::Regex(value.as_str().to_owned()),
        Matcher::Any => RenderedMatcher::Any,
        _ => RenderedMatcher::Unknown,
    }
}

fn render_scalar(scalar: &FrontmatterScalar) -> RenderedScalar {
    match scalar {
        FrontmatterScalar::Null => RenderedScalar::Null,
        FrontmatterScalar::Boolean(value) => RenderedScalar::Boolean(*value),
        FrontmatterScalar::Integer(value) => RenderedScalar::Integer(value.as_str().to_owned()),
        FrontmatterScalar::Float(value) => RenderedScalar::Float(value.as_str().to_owned()),
        FrontmatterScalar::String(value) => RenderedScalar::String(value.clone()),
    }
}

fn schema_node_location(node: &SchemaNode, locations: &SchemaLocations) -> Option<SourceRange> {
    locations.nodes.get(node).copied()
}

fn source_location(
    sources: &SchemaSources,
    range: SourceRange,
    fallback_path: &str,
) -> RenderedLocation {
    let Some(source) = sources.documents.get(&range.source) else {
        return RenderedLocation {
            path: fallback_path.to_owned(),
            line: 1,
            column: 1,
        };
    };
    let path = source
        .label
        .as_ref()
        .map_or_else(|| fallback_path.to_owned(), |label| label.0.clone());
    let (line, column) = line_column(&source.text, range.range.start.0);
    RenderedLocation { path, line, column }
}

fn line_column(source: &str, byte_offset: usize) -> (u64, u64) {
    let offset = byte_offset.min(source.len());
    let bytes = source.as_bytes();
    let mut index = 0;
    let mut line = 1_u64;
    let mut line_start = 0;

    while index < offset {
        match bytes.get(index).copied() {
            Some(b'\r') => {
                let next = index.saturating_add(1);
                let terminator_width =
                    usize::from(next < offset && bytes.get(next).copied() == Some(b'\n')) + 1;
                index = index.saturating_add(terminator_width).min(offset);
                line = line.saturating_add(1);
                line_start = index;
            }
            Some(b'\n') => {
                index = index.saturating_add(1);
                line = line.saturating_add(1);
                line_start = index;
            }
            Some(_) => index = index.saturating_add(1),
            None => break,
        }
    }

    let column =
        u64::try_from(offset.saturating_sub(line_start).saturating_add(1)).unwrap_or(u64::MAX);
    (line, column)
}

/// The total per-file ordering key; [`sort_diagnostics`] documents each tier.
type DiagnosticSortKey<'a> = (
    u64,
    u64,
    &'a str,
    Option<&'a RenderedLocation>,
    Option<&'a RenderedTarget>,
    &'a str,
    Option<&'a RenderedSchemaNode>,
    &'a [RenderedInvolvedHeader],
    &'a [RenderedReference],
    &'a str,
);

fn diagnostic_sort_key(diagnostic: &RenderedDiagnostic) -> DiagnosticSortKey<'_> {
    (
        diagnostic.line,
        diagnostic.column,
        diagnostic.id.as_str(),
        diagnostic.schema_location.as_ref(),
        diagnostic.target.as_ref(),
        diagnostic.message.as_str(),
        diagnostic.schema_node.as_ref(),
        &diagnostic.involved_headers,
        &diagnostic.references,
        diagnostic.source_path.as_str(),
    )
}

/// Sorts one file's diagnostics into the order the JSON contract promises.
///
/// The key is **total**: it compares every rendered field, so the emitted
/// order is a pure function of the diagnostic set and can never depend on the
/// order the validator happened to produce them in. The tiers, most
/// significant first:
///
/// 1. source `line`, then byte `column`;
/// 2. diagnostic `id`, lexicographically;
/// 3. `schema_location` as `(path, line, column)`, absent first;
/// 4. `target`, by kind in the §6.1 order (`header`, `missing_header`,
///    `document`, `frontmatter`), then by its members in declaration order
///    (path segments; parent then matcher; line range then pointer), absent
///    first — schema errors have no target;
/// 5. `message`, lexicographically by bytes;
/// 6. `schema_node`, `involved_headers`, `references`, and `source_path`, in
///    that order, purely so no two distinct diagnostics ever compare equal.
///
/// The target outranks the message so that key-equal lines group by what they
/// are about rather than alphabetizing prose, and because for the one tie
/// family that occurs in practice — `frontmatter-schema` findings sharing a
/// fallback anchor — the frontmatter target orders by JSON Pointer first,
/// which matches the `(instance_path, message)` normalization the validator
/// already applies to those errors.
pub(crate) fn sort_diagnostics(diagnostics: &mut [RenderedDiagnostic]) {
    diagnostics.sort_by(|left, right| diagnostic_sort_key(left).cmp(&diagnostic_sort_key(right)));
}

#[cfg(test)]
mod tests {
    use super::{
        diagnostic_sort_key, line_column, sort_diagnostics, RenderedDiagnostic, RenderedLineRange,
        RenderedLocation, RenderedMatcher, RenderedReference, RenderedTarget,
    };

    #[test]
    fn source_positions_are_one_based_byte_columns() {
        assert_eq!(line_column("one\nåx", 6), (2, 3));
        assert_eq!(line_column("one\råx", 6), (2, 3));
        assert_eq!(line_column("one\r\n😀x", 9), (2, 5));
    }

    /// Builds one diagnostic tying the pre-total sort key `(line, column, id,
    /// schema_location)`; the varying fields are exactly the tiebreakers.
    fn key_tied_diagnostic(
        target: Option<RenderedTarget>,
        message: &str,
        references: Vec<RenderedReference>,
    ) -> RenderedDiagnostic {
        RenderedDiagnostic {
            id: "too-few-sections".into(),
            message: message.into(),
            source_path: "document.md".into(),
            line: 3,
            column: 1,
            target,
            schema_node: None,
            schema_location: Some(RenderedLocation {
                path: "schema.outlint.yml".into(),
                line: 2,
                column: 5,
            }),
            involved_headers: Vec::new(),
            references,
        }
    }

    /// Diagnostics that all tie under `(line, column, id, schema_location)`,
    /// listed in the order the JSON total key promises: target kind, then target
    /// members, then message, with references as a final backstop.
    fn key_tied_fixture() -> Vec<RenderedDiagnostic> {
        let frontmatter = |pointer: &str| RenderedTarget::Frontmatter {
            line_range: Some(RenderedLineRange {
                start_line: 1,
                end_line: 3,
            }),
            pointer: Some(pointer.into()),
        };
        vec![
            key_tied_diagnostic(
                Some(RenderedTarget::Header {
                    path: vec!["Alpha".into()],
                }),
                "m",
                Vec::new(),
            ),
            key_tied_diagnostic(
                Some(RenderedTarget::Header {
                    path: vec!["Alpha".into(), "Beta".into()],
                }),
                "m",
                Vec::new(),
            ),
            key_tied_diagnostic(
                Some(RenderedTarget::MissingHeader {
                    parent: Vec::new(),
                    matcher: "Step *".into(),
                }),
                "m",
                Vec::new(),
            ),
            key_tied_diagnostic(Some(RenderedTarget::Document), "matched 0", Vec::new()),
            key_tied_diagnostic(Some(RenderedTarget::Document), "matched 1", Vec::new()),
            key_tied_diagnostic(Some(frontmatter("/a")), "m", Vec::new()),
            key_tied_diagnostic(Some(frontmatter("/b")), "m", Vec::new()),
            key_tied_diagnostic(
                Some(frontmatter("/b")),
                "m",
                vec![RenderedReference::Rule {
                    anchor: "/",
                    path: vec!["a".into()],
                    matcher: RenderedMatcher::Exact("A".into()),
                }],
            ),
        ]
    }

    fn key_strings(diagnostics: &[RenderedDiagnostic]) -> Vec<String> {
        diagnostics
            .iter()
            .map(|diagnostic| format!("{:?}", diagnostic_sort_key(diagnostic)))
            .collect()
    }

    #[test]
    fn diagnostics_tied_on_the_old_key_sort_into_the_json_total_order() {
        let canonical = key_tied_fixture();
        // The key is total on the fixture: every adjacent pair is strictly
        // ordered, so no two distinct diagnostics compare equal.
        for pair in canonical.windows(2) {
            assert!(
                diagnostic_sort_key(&pair[0]) < diagnostic_sort_key(&pair[1]),
                "fixture entries compare equal or reversed: {pair:#?}"
            );
        }
        // Reversal simulates the worst emission-order flip a validator-walk
        // refactor could produce; a merely stable sort on the old partial key
        // would preserve it and fail here.
        let mut reversed = key_tied_fixture();
        reversed.reverse();
        sort_diagnostics(&mut reversed);
        assert_eq!(key_strings(&reversed), key_strings(&canonical));
    }

    #[test]
    fn every_emission_order_sorts_to_the_same_sequence() {
        let size = key_tied_fixture().len();
        let mut indices = (0..size).collect::<Vec<_>>();
        let mut permutations = Vec::new();
        heap_permutations(&mut indices, size, &mut permutations);
        for permutation in permutations {
            let mut slots = key_tied_fixture().into_iter().map(Some).collect::<Vec<_>>();
            let mut shuffled = permutation
                .iter()
                .map(|&index| slots[index].take().expect("each index appears once"))
                .collect::<Vec<_>>();
            sort_diagnostics(&mut shuffled);
            // Strict order under a total key means the sorted arrangement of
            // this multiset is unique, so every permutation lands on it.
            for pair in shuffled.windows(2) {
                assert!(
                    diagnostic_sort_key(&pair[0]) < diagnostic_sort_key(&pair[1]),
                    "permutation {permutation:?} did not sort strictly"
                );
            }
        }
    }

    fn heap_permutations(indices: &mut Vec<usize>, size: usize, output: &mut Vec<Vec<usize>>) {
        if size <= 1 {
            output.push(indices.clone());
            return;
        }
        for step in 0..size {
            heap_permutations(indices, size - 1, output);
            let last = size - 1;
            if size % 2 == 0 {
                indices.swap(step, last);
            } else {
                indices.swap(0, last);
            }
        }
    }
}
