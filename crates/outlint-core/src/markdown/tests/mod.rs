//! Unit tests for the Markdown module, grouped by subject.

mod body;
mod frontmatter_anchors;
mod frontmatter_scalars;
mod frontmatter_yaml;
mod properties;

use super::model::FrontmatterAnchors;

/// No byte-order mark was taken off the head of the bodies below, so the
/// builder's own positions need no counting back. See
/// [`parse_exact_yaml`](super::frontmatter::yaml::parse_exact_yaml).
const NO_MARK: usize = 0;

/// No two entries may name one position.
///
/// A borrowed marker shows up here: the entry that has no text of its own
/// is reported at a later entry's, and both then claim it. Nesting is the
/// one legitimate sharing — a block mapping inside a sequence begins at its
/// own first key, so `/items/0` and `/items/0/key` coincide by design — so
/// pairs where one pointer is a prefix of the other are exempt.
fn assert_distinct_anchors(source: &str, anchors: &FrontmatterAnchors) {
    let mut placed: Vec<_> = anchors
        .0
        .iter()
        .map(|(pointer, anchor)| (anchor.line, anchor.column, pointer.as_str()))
        .collect();
    placed.sort_unstable();
    for pair in placed.windows(2) {
        let (line, column, earlier) = pair[0];
        let (other_line, other_column, later) = pair[1];
        if (line, column) != (other_line, other_column) {
            continue;
        }
        assert!(
            is_pointer_prefix(earlier, later),
            "{earlier} and {later} both claim {line}:{column} in {source:?}"
        );
    }
}

/// Whether one JSON Pointer names an ancestor of what another names.
///
/// Tokens are compared whole so that `/a` is not read as a prefix of `/ab`.
fn is_pointer_prefix(ancestor: &str, descendant: &str) -> bool {
    descendant
        .strip_prefix(ancestor)
        .is_some_and(|rest| ancestor.is_empty() || rest.is_empty() || rest.starts_with('/'))
}
