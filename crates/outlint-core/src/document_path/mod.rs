//! Addressing one concrete node of a parsed Markdown [`Document`](crate::Document).
//!
//! A document path names a node of the tree that
//! [`parse_markdown`](crate::parse_markdown) produces: the document root, a
//! section opened by a heading, one visible direct block of the root's or a
//! section's preamble, or one direct item of such a list. It is a
//! *document-side* address — it describes what was actually parsed, not what a
//! schema declares — so a tool can point at a heading or block in a concrete
//! file and later find the same node again, or enumerate every addressable
//! node of a document with [`document_paths`].
//!
//! This API is provisional and may change in a minor release before 1.0.
//!
//! # Grammar
//!
//! ```text
//! doc-path     = "$" *( section-step ) [ "/" terminal ]
//! section-step = "." ( slug [ "[" index "]" ] / "[" index "]" )
//!              / ".." slug [ "[" index "]" ]
//! terminal     = block-kind [ "[" index "]" ]
//!              / "list" [ "[" index "]" ] [ "/item" [ "[" index "]" ] ]
//! block-kind   = "p" | "code" | "quote" | "html" | "break"
//! index        = "0" / [1-9][0-9]*
//! slug         = [a-z0-9]+(-[a-z0-9]+)*
//! ```
//!
//! Every path starts at the root, `$`. Each `.` descends into a child section
//! of the node reached so far, each `..` into a section anywhere below it, and
//! an optional `/` terminal selects a direct preamble block of the node reached
//! (or of the root when no section step precedes it), and optionally one of a
//! direct list's items.
//!
//! A `.` step is either *named* or *positional*. A named step `slug` selects
//! the sibling whose heading slugs to `slug`; when several siblings share the
//! slug the step is ambiguous and `slug[i]` must pick the `i`-th of them in
//! document order. A positional step `[i]` selects the `i`-th sibling section
//! regardless of its heading, which is the only way to reach a heading whose
//! slug is empty (for example one consisting solely of punctuation or emoji).
//! Slugs are derived from [`Heading::text`](crate::Heading::text) by
//! [`heading_slug`].
//!
//! A *descendant* step `..slug` selects, among every section below the node
//! reached so far — at any depth, in document order, excluding that node
//! itself — the one whose heading slugs to `slug`; `..slug[i]` picks the
//! `i`-th such section and a bare `..slug` requires the match to be unique.
//! There is no positional descendant step. `..` is an input convenience for
//! addressing a well-known heading without spelling its ancestors: the
//! canonical spelling of a node, which [`document_paths`] produces and
//! [`Display`](std::fmt::Display) preserves, only ever uses `.` steps.
//!
//! A direct block terminal `kind[i]` selects the `i`-th block *of that kind*
//! among the parent's direct preamble blocks, so `p[1]` is the second paragraph
//! even when a list sits between the two paragraphs. The index may be omitted
//! and then defaults to `0`; the canonical spelling produced by
//! [`Display`](std::fmt::Display) always writes it. An `item[i]` segment is
//! only valid directly after a `list` terminal and selects the list's `i`-th
//! direct item. A terminal always ends the path, because the document model
//! exposes neither nested blocks nor item contents. Indices everywhere are
//! zero-based.
//!
//! # A sole H1 is the document root
//!
//! When a document has exactly one top-level section and its heading is an
//! H1, that heading names the document rather than opening a section inside
//! it, mirroring the `title` slot of a schema. The section is therefore merged
//! into the root: `$` still resolves to the root, the merged heading has no
//! address of its own, its child sections are addressed as `$.child`, and the
//! per-kind ordinals of `$/kind[i]` run over the root's preamble blocks
//! followed by the merged heading's own blocks. [`merged_title`] returns the
//! merged section so a consumer can still obtain the title text. Documents
//! with zero or several top-level sections, or whose sole top-level section is
//! not an H1, are addressed as parsed.
//!
//! # Examples
//!
//! ```text
//! $                                   the document root
//! $.deployment.rollback-plan          a nested section by slug
//! $.faq.question[2]                   the third sibling section slugging to `question`
//! $.[2]                               the third top-level section, whatever its heading
//! $.setup/p[0]                        the first paragraph under "Setup"
//! $.setup/list[0]/item[3]             the fourth item of the first list under "Setup"
//! $/p[0]                              the first paragraph before any heading
//! $..rollback-plan                    the only "Rollback plan" heading, wherever it sits
//! $.faq..question[1]/p[0]             a paragraph under the second `question` below "FAQ"
//! ```

mod enumeration;
mod model;
mod resolution;
mod syntax;

pub use crate::slug::{heading_slug, HeadingSlug};
pub use enumeration::document_paths;
pub use model::{
    CanonicalDocumentPath, CanonicalSectionStep, DocumentNode, DocumentPath, DocumentPathError,
    DocumentPathSyntaxError, DocumentPathTerminal, SectionStep,
};
pub use resolution::merged_title;

#[cfg(test)]
mod tests;
