//! Structured search results and deterministic hit selection.

use std::cmp::Ordering;
use std::num::NonZeroU16;

/// Largest result limit accepted by the search API and CLI.
pub const MAX_SEARCH_LIMIT: usize = 1_000;

/// A positive search result limit bounded by [`MAX_SEARCH_LIMIT`].
///
/// Keeping the bound in the type prevents callers from passing an allocation
/// size derived directly from untrusted command-line input.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchLimit(NonZeroU16);

impl SearchLimit {
    /// Validates a raw limit against the inclusive supported range.
    pub fn new(value: usize) -> Option<Self> {
        if value > MAX_SEARCH_LIMIT {
            return None;
        }
        u16::try_from(value)
            .ok()
            .and_then(NonZeroU16::new)
            .map(Self)
    }

    /// Returns the validated limit as an indexing and collection size.
    pub fn get(self) -> usize {
        usize::from(self.0.get())
    }
}

/// One matching index unit, as loaded back from the index.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    /// File path relative to the search root, with forward slashes.
    pub path: String,
    /// Rendered document path of the unit inside the file.
    pub mdpath: String,
    /// Length in bytes of the unit's full extent.
    pub bytes: u64,
    /// Extent length of the enclosing section, for a block or item unit.
    pub section_bytes: Option<u64>,
    /// The engine's relevance score; higher is better.
    pub score: f32,
    /// An excerpt of the unit's text, at most a few lines' worth, chosen
    /// around the query words when they occur in it and otherwise taken
    /// from its start, with `…` marking a cut; empty when the unit has no
    /// text to excerpt, such as a section holding only subsections.
    pub snippet: String,
}

/// Ranked hits together with the number of matching blocks before the display
/// limit. A matching item is its own result; its aggregate list is excluded.
/// A list remains a result only when its query words are spread across items.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchResults {
    /// Best matches after aggregate-list exclusion and the requested limit.
    pub hits: Vec<Hit>,
    /// Number of units in the same filtered universe from which `hits` came.
    pub total: usize,
}

/// One user-spelled query term and the number of indexed non-item units whose
/// own text matches it under the search analyzer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TermCount {
    /// The term exactly as supplied in the query.
    pub term: String,
    /// Number of matching units inside the active search root.
    pub blocks: usize,
}

/// Orders hits by descending score, then path, then document path, so output
/// is stable across runs with equal scores.
pub(crate) fn sort_hits(hits: &mut [Hit]) {
    hits.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.path.cmp(&right.path))
            .then_with(|| left.mdpath.cmp(&right.mdpath))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(path: &str, mdpath: &str, score: f32) -> Hit {
        Hit {
            path: path.into(),
            mdpath: mdpath.into(),
            bytes: 0,
            section_bytes: None,
            score,
            snippet: String::new(),
        }
    }

    #[test]
    fn sort_hits_orders_by_score_then_path_then_mdpath() {
        let mut hits = vec![
            hit("b.md", "$.a", 1.0),
            hit("a.md", "$.b", 1.0),
            hit("a.md", "$.a", 1.0),
            hit("z.md", "$", 5.0),
        ];
        sort_hits(&mut hits);
        let order: Vec<(&str, &str)> = hits
            .iter()
            .map(|hit| (hit.path.as_str(), hit.mdpath.as_str()))
            .collect();
        assert_eq!(
            order,
            [
                ("z.md", "$"),
                ("a.md", "$.a"),
                ("a.md", "$.b"),
                ("b.md", "$.a")
            ]
        );
    }

    #[test]
    fn search_limit_enforces_its_non_zero_upper_bound() {
        assert!(SearchLimit::new(0).is_none());
        assert_eq!(SearchLimit::new(1).map(SearchLimit::get), Some(1));
        assert_eq!(
            SearchLimit::new(MAX_SEARCH_LIMIT).map(SearchLimit::get),
            Some(MAX_SEARCH_LIMIT)
        );
        assert!(SearchLimit::new(MAX_SEARCH_LIMIT.saturating_add(1)).is_none());
    }
}
