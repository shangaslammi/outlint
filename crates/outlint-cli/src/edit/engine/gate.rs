//! Compare validator findings on subjects retained by structural verification.
use super::batch::Candidate;
use outlint_core::*;
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

impl Candidate<'_> {
    pub(in crate::edit) fn introduced_diagnostics(
        &self,
        schema: &Schema,
    ) -> Result<Vec<Diagnostic>, GateError> {
        let validator = PreparedValidator::new(schema).map_err(GateError::Prepare)?;
        let options = MarkdownOptions {
            strip_inline_markup: schema.options.strip_inline_markup,
        };
        let edited = self.clean_text();
        let alternate;
        let (old_doc, new_doc) = if options == MarkdownOptions::default() {
            (&self.original_doc, &self.edited_doc)
        } else {
            alternate = (
                parse_markdown(self.original, options).map_err(GateError::Parse)?,
                parse_markdown(edited, options).map_err(GateError::Parse)?,
            );
            (&alternate.0, &alternate.1)
        };
        let mut before = validator.validate(old_doc).map_err(GateError::Validate)?;
        if edited == self.original {
            return Ok(Vec::new());
        }
        let after = validator.validate(new_doc).map_err(GateError::Validate)?;
        let same_root = match &schema.document {
            DocumentShape::Title(
                TitleSlot::Spelled { .. } | TitleSlot::ImpliedBySections { .. },
            ) => {
                // A single title's missing children and constraints use the document
                // as their public target. Keep the physical owner across the edit.
                let titles = |doc: &Document| {
                    doc.sections
                        .iter()
                        .filter(|s| s.heading.level == HeaderLevel::H1)
                        .map(|s| s.heading.location.line_range.start.0)
                        .collect::<Vec<_>>()
                };
                match (titles(old_doc).as_slice(), titles(new_doc).as_slice()) {
                    ([], []) => true,
                    ([a], [b]) => self.surviving_offset(*a) == Some(*b),
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
        Ok(after
            .into_iter()
            .filter(|new| {
                if let Some(i) = before.iter().position(|old| {
                    self.same(old, new, same_root)
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
    fn anchor(&self, a: &DiagnosticLocation, b: &DiagnosticLocation) -> bool {
        self.surviving_offset(a.range.start.0) == Some(b.range.start.0)
    }
    fn surviving_offset(&self, old: usize) -> Option<usize> {
        self.subjects.anchors.get(&old).copied()
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
        let same_list =
            self.subjects.lists.get(&a.location.range.start.0) == Some(&b.location.range.start.0);
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
