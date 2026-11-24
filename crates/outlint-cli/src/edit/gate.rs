//! Compare validator findings on subjects retained by structural verification.
use outlint_core::*;
use outlint_edit::Candidate;
use std::fmt;

#[derive(Debug)]
pub(in crate::edit) enum GateError {
    Parse(MarkdownParseError),
    Prepare(PrepareValidationError),
    Validate(ValidationOperationalError),
}

impl fmt::Display for GateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse(e) => e.fmt(f),
            Self::Prepare(e) => e.fmt(f),
            Self::Validate(e) => e.fmt(f),
        }
    }
}
impl std::error::Error for GateError {}

pub(super) fn introduced_diagnostics(
    candidate: &Candidate<'_>,
    schema: &Schema,
) -> Result<Vec<Diagnostic>, GateError> {
    let validator = PreparedValidator::new(schema).map_err(GateError::Prepare)?;
    let options = MarkdownOptions {
        strip_inline_markup: schema.options.strip_inline_markup,
    };
    let edited = candidate.clean_text();
    let alternate;
    let (old_doc, new_doc) = if options == MarkdownOptions::default() {
        (&candidate.original_doc, &candidate.edited_doc)
    } else {
        alternate = (
            parse_markdown(candidate.original, options).map_err(GateError::Parse)?,
            parse_markdown(edited, options).map_err(GateError::Parse)?,
        );
        (&alternate.0, &alternate.1)
    };
    let mut before = validator.validate(old_doc).map_err(GateError::Validate)?;
    if edited == candidate.original {
        return Ok(Vec::new());
    }
    let after = validator.validate(new_doc).map_err(GateError::Validate)?;
    let same_root = match &schema.document {
        DocumentShape::Title(TitleSlot::Spelled { .. } | TitleSlot::ImpliedBySections { .. }) => {
            // A single title's missing children and constraints use the document
            // as their public target. Keep the physical owner across the edit.
            let titles = |doc: &Document| {
                doc.sections
                    .iter()
                    .filter(|s| s.heading.level == HeaderLevel::H1)
                    .map(|s| s.heading.location.line_range.start)
                    .collect::<Vec<_>>()
            };
            match (titles(old_doc).as_slice(), titles(new_doc).as_slice()) {
                ([], []) => true,
                ([a], [b]) => candidate.subjects.anchors.get(a).copied() == Some(*b),
                _ => false,
            }
        }
        _ => true,
    };
    // The validator exposes the instance pointer but not the failed JSON Schema
    // keyword, or the concrete nodes returned by a frontmatter query.
    let same_fm = match (&old_doc.frontmatter, &new_doc.frontmatter) {
        (
            DocumentFrontmatter::Mapping { value: a, .. },
            DocumentFrontmatter::Mapping { value: b, .. },
        ) => a == b,
        (a, b) => a == b,
    };
    let comparison = Comparison { candidate };
    Ok(after
        .into_iter()
        .filter(|new| {
            if let Some(i) = before.iter().position(|old| {
                comparison.same(old, new, same_root)
                    && (same_fm
                        || (new.id != DiagnosticId::FrontmatterSchema
                            && !new.references.iter().any(|r| {
                                matches!(
                                    r,
                                    DiagnosticReference::FrontmatterQuery(_)
                                        | DiagnosticReference::FrontmatterCapture(_)
                                )
                            })))
            }) {
                before.swap_remove(i);
                false
            } else {
                true
            }
        })
        .collect())
}

pub(super) fn added_suppressions<'a>(
    candidate: &'a Candidate<'_>,
) -> Vec<&'a SuppressionDirective> {
    if candidate.original == candidate.clean_text() {
        return Vec::new();
    }
    let mut before: Vec<_> = candidate
        .original_doc
        .suppression_directives
        .iter()
        .collect();
    candidate
        .edited_doc
        .suppression_directives
        .iter()
        .filter(|new| {
            let previous = before.iter().position(|old| {
                old.file_wide == new.file_wide
                    && old.ids == new.ids
                    && (old.file_wide
                        || candidate.subjects.directives.get(&old.range.start)
                            == Some(&new.range.start))
            });
            if let Some(previous) = previous {
                before.swap_remove(previous);
                false
            } else {
                true
            }
        })
        .collect()
}

struct Comparison<'a> {
    candidate: &'a Candidate<'a>,
}

impl Comparison<'_> {
    fn anchor(&self, a: &DiagnosticLocation, b: &DiagnosticLocation) -> bool {
        self.candidate.subjects.anchors.get(&a.range.start) == Some(&b.range.start)
    }
    fn same(&self, a: &Diagnostic, b: &Diagnostic, same_root: bool) -> bool {
        use DiagnosticTarget::*;
        if a.id != b.id
            || a.schema_node != b.schema_node
            || a.references != b.references
            || a.involved_headers.len() != b.involved_headers.len()
            || !a
                .involved_headers
                .iter()
                .zip(&b.involved_headers)
                .all(|(x, y)| self.anchor(&x.location, &y.location))
        {
            return false;
        }
        let anchored = self.anchor(&a.location, &b.location);
        let same_list = self.candidate.subjects.lists.get(&a.location.range.start)
            == Some(&b.location.range.start);
        match (&a.target, &b.target) {
            (Document, Document) => same_root,
            (Frontmatter { block: x }, Frontmatter { block: y }) => {
                x.as_ref().map(|v| &v.json_pointer) == y.as_ref().map(|v| &v.json_pointer)
            }
            (Header(_), Header(_)) | (Item { .. }, Item { .. }) => anchored,
            (Block { block: x, .. }, Block { block: y, .. }) => {
                x == y
                    && if *x == BlockKind::List {
                        same_list
                    } else {
                        anchored
                    }
            }
            (MissingItem { matcher: x, .. }, MissingItem { matcher: y, .. }) => x == y && same_list,
            (
                MissingHeader {
                    parent: p,
                    matcher: x,
                },
                MissingHeader {
                    parent: q,
                    matcher: y,
                },
            ) => x == y && owner(p, q, anchored, same_root),
            (
                MissingBlock {
                    parent: p,
                    matcher: x,
                },
                MissingBlock {
                    parent: q,
                    matcher: y,
                },
            ) => x == y && owner(p, q, anchored, same_root),
            _ => false,
        }
    }
}
fn owner(p: &HeaderPath, q: &HeaderPath, anchored: bool, same_root: bool) -> bool {
    match (p.0.is_empty(), q.0.is_empty()) {
        (true, true) => same_root,
        (false, false) => anchored,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use outlint_edit::{candidate, Edit, Op};

    fn op(path: &str, op: Op) -> Edit {
        Edit {
            path: path.into(),
            op,
        }
    }

    #[test]
    fn an_identical_candidate_keeps_its_suppression_occurrences() {
        let suppressed = "<!-- outlint-disable unexpected-block -->\n> old";
        let source = format!("lead\n\n{suppressed}\n");
        let edited = candidate(
            &source,
            &[
                op("$/quote", Op::Delete),
                op("$/quote", Op::InsertBefore(suppressed.into())),
            ],
        )
        .unwrap();
        assert_eq!(edited.text, source);
        assert!(added_suppressions(&edited).is_empty());
    }

    #[test]
    fn gate_matches_surviving_anchors_once() {
        let schema=load_schema("version: 1\ntitle: null\nsections:\n  - match: Same\n    repeat: '0..n'\n    content:\n      - block: list\n        repeat: '0..1'\n        items: [{match: ok, repeat: '0..n'}]\n").unwrap().schema;
        let s = "## Same\n- bad\n\n## Same\n- other\n";
        let gate = |edits: &[Edit]| {
            introduced_diagnostics(&candidate(s, edits).unwrap(), &schema).unwrap()
        };
        assert!(gate(&[op(
            "$.same[0]/list",
            Op::InsertItem {
                index: 0,
                payload: "ok".into()
            }
        )])
        .is_empty());
        let new = gate(&[
            op("$.same[0]/list/item", Op::Delete),
            op("$.same[1]/list", Op::AppendItem("third".into())),
        ]);
        assert_eq!(new.len(), 1);
        assert_eq!(new[0].id.as_str(), "unexpected-item");
        let both = "# Bad\n# Bad\n";
        let schema = load_schema("version: 1\noutline: []\n").unwrap().schema;
        assert!(introduced_diagnostics(
            &candidate(both, &[op("$.bad[1]", Op::Rename("Other".into()))]).unwrap(),
            &schema
        )
        .unwrap()
        .is_empty());
        let bad = "# Bad\n";
        let c = candidate(
            bad,
            &[
                op(
                    "$",
                    Op::ReplaceText {
                        old: "# Bad".into(),
                        new: "# Bad".into(),
                    },
                ),
                op("$", Op::AppendBlock("New paragraph.".into())),
            ],
        )
        .unwrap();
        assert_ne!(c.text, bad);
        assert!(introduced_diagnostics(&c, &schema).unwrap().is_empty());
    }
    #[test]
    fn document_voiced_absence_tracks_the_title() {
        let schema = load_schema("version: 1\ntitle: '*'\nsections: [{match: Needed}]\n")
            .unwrap()
            .schema;
        let c = candidate("intro\n\n# A\n", &[op("$", Op::Delete)]).unwrap();
        assert!(introduced_diagnostics(&c, &schema)
            .unwrap()
            .iter()
            .any(|d| d.id == DiagnosticId::MissingSection));
    }
    #[test]
    fn missing_item_tracks_a_list_when_its_first_marker_changes() {
        let schema = load_schema(
            "version: 1\ntitle: T\ncontent:\n  - block: list\n    items: [{match: needed}]\n",
        )
        .unwrap()
        .schema;
        let source = "# T\n\n- first\n- second\n";
        let before = validate(
            &schema,
            &parse_markdown(source, MarkdownOptions::default()).unwrap(),
        )
        .unwrap();
        assert!(before.iter().any(|d| d.id == DiagnosticId::MissingItem));

        let edited = candidate(source, &[op("$/list/item[0]", Op::Delete)]).unwrap();
        assert_eq!(edited.text, "# T\n\n- second\n");
        assert!(introduced_diagnostics(&edited, &schema).unwrap().is_empty());
    }
    #[test]
    fn literal_gate_uses_the_same_heading_and_title_for_equivalent_candidates() {
        let schema = load_schema("version: 1\ntitle: '*'\nsections: [{match: Good}]\n")
            .unwrap()
            .schema;
        let source = "# Title\n\n## Bad\n";
        let original = parse_markdown(source, MarkdownOptions::default()).unwrap();
        let original_diagnostics = validate(&schema, &original).unwrap();
        assert!(original_diagnostics
            .iter()
            .any(|d| d.id == DiagnosticId::UnexpectedSection));
        assert!(original_diagnostics
            .iter()
            .any(|d| d.id == DiagnosticId::MissingSection));
        for (old, new) in [
            ("## Bad", "## Worse"),
            ("Bad", "Worse"),
            ("# Title", "# New"),
            ("Title", "New"),
        ] {
            let edits = [op(
                "$",
                Op::ReplaceText {
                    old: old.into(),
                    new: new.into(),
                },
            )];
            let candidate = candidate(source, &edits).unwrap();
            assert!(
                introduced_diagnostics(&candidate, &schema)
                    .unwrap()
                    .is_empty(),
                "{old:?}"
            );
        }
    }
}
