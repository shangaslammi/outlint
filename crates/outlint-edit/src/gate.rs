//! Compare validator findings using the original coordinates retained by a candidate.
use super::*;
use std::collections::BTreeMap;
/// A verified candidate with the original splice correspondence for a schema gate.
pub struct Candidate<'a> {
    original: &'a str,
    /// Exact bytes to preview or apply, including the original BOM.
    text: String,
    changes: Vec<Change>,
}
/// Plan once so preview, validation and writing use the same bytes.
pub fn candidate<'a>(source: &'a str, edits: &[Edit]) -> Result<Candidate<'a>, Error> {
    let clean = source.strip_prefix('\u{feff}').unwrap_or(source);
    let (mut text, mut changes) = prepare(clean, edits)?;
    // A byte-identical replacement leaves any diagnostic anchor inside it in place.
    changes.retain(|change| clean.get(change.range.clone()) != Some(&change.replacement));
    if clean.len() != source.len() {
        text.insert(0, '\u{feff}')
    }
    Ok(Candidate {
        original: clean,
        text,
        changes,
    })
}
impl Candidate<'_> {
    /// Exact verified source, including its original BOM when present.
    pub fn text(&self) -> &str {
        &self.text
    }
    pub(super) fn into_text(self) -> String {
        self.text
    }

    /// One-to-one multiset subtraction with unproven subject correspondence treated as new.
    pub fn introduced_diagnostics(&self, schema: &Schema) -> Result<Vec<Diagnostic>, String> {
        let v = PreparedValidator::new(schema).map_err(|e| e.to_string())?;
        let validate = |s: &str| {
            let doc = parse_markdown(
                s,
                MarkdownOptions {
                    strip_inline_markup: schema.options.strip_inline_markup,
                },
            )
            .map_err(|e| e.to_string())?;
            Ok::<_, String>((v.validate(&doc).map_err(|e| e.to_string())?, doc))
        };
        let (mut before, old_doc) = validate(self.original)?;
        let edited = self.text.strip_prefix('\u{feff}').unwrap_or(&self.text);
        if edited == self.original {
            return Ok(Vec::new());
        }
        let (after, new_doc) = validate(edited)?;
        let list_owners = |doc: &Document| {
            let mut owners = BTreeMap::new();
            for (_, node) in document_paths(doc) {
                if let DocumentNode::Block(Block::List(list)) = node {
                    for item in list.items.iter() {
                        owners.insert(
                            item.location.line_range.start.0,
                            list.location.line_range.start.0,
                        );
                    }
                }
            }
            owners
        };
        let post_lists = list_owners(&new_doc);
        let lists: BTreeMap<_, _> = list_owners(&old_doc)
            .into_iter()
            .filter_map(|(item, list)| {
                let new = self.surviving_offset(item)?;
                Some((list, *post_lists.get(&new)?))
            })
            .collect();
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
                match (titles(&old_doc).as_slice(), titles(&new_doc).as_slice()) {
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
                    self.same(old, new, &lists, same_root)
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
        if self
            .changes
            .iter()
            .any(|c| c.range.contains(&old) && c.kind != ChangeKind::Rename)
        {
            return None;
        }
        mapped(old, &self.changes)
    }
    fn same(
        &self,
        a: &Diagnostic,
        b: &Diagnostic,
        lists: &BTreeMap<usize, usize>,
        same_root: bool,
    ) -> bool {
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
        let same_list = lists.get(&a.location.range.start.0) == Some(&b.location.range.start.0);
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
