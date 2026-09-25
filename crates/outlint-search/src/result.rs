//! Structured search results and deterministic hit selection.

use std::cmp::Ordering;

/// A relevance score known not to be NaN or infinite.
///
/// Search results use this wrapper so every score can be ordered and emitted
/// as a JSON number without a renderer-side fallback.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct FiniteScore(f32);

impl FiniteScore {
    /// Accepts only finite engine scores.
    pub fn new(value: f32) -> Option<Self> {
        value.is_finite().then_some(Self(value))
    }

    /// Returns the finite floating-point value.
    pub fn get(self) -> f32 {
        self.0
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
    pub score: FiniteScore,
    /// An excerpt of the unit's text, at most a few lines' worth, chosen
    /// around the query words when they occur in it and otherwise taken
    /// from its start, with `…` marking a cut; empty when the unit has no
    /// text to excerpt, such as a section holding only subsections.
    pub snippet: String,
}

/// One broad candidate and the narrower matching item found beneath it, when
/// the candidate is a list whose one item satisfies the whole query.
pub(crate) struct CandidateHit {
    pub(crate) hit: Hit,
    pub(crate) narrower: Option<Hit>,
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
            .get()
            .partial_cmp(&left.score.get())
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.path.cmp(&right.path))
            .then_with(|| left.mdpath.cmp(&right.mdpath))
    });
}

/// Replaces list candidates with matching item candidates, removes duplicate
/// units, restores the global score order, and applies the display limit.
/// A list with no narrower match remains, representing words spread across
/// its items.
pub(crate) fn select_smallest_hits(candidates: Vec<CandidateHit>, limit: usize) -> Vec<Hit> {
    let mut hits: Vec<Hit> = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        let hit = candidate.narrower.unwrap_or(candidate.hit);
        if let Some(existing) = hits
            .iter_mut()
            .find(|existing| existing.path == hit.path && existing.mdpath == hit.mdpath)
        {
            if hit.score > existing.score {
                *existing = hit;
            }
        } else {
            hits.push(hit);
        }
    }
    sort_hits(&mut hits);
    hits.truncate(limit);
    hits
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
            score: FiniteScore::new(score).expect("fixture score is finite"),
            snippet: String::new(),
        }
    }

    #[test]
    fn finite_score_rejects_every_non_finite_value() {
        assert_eq!(FiniteScore::new(f32::NAN), None);
        assert_eq!(FiniteScore::new(f32::INFINITY), None);
        assert_eq!(FiniteScore::new(f32::NEG_INFINITY), None);
        assert_eq!(FiniteScore::new(-0.0).map(FiniteScore::get), Some(-0.0));
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
    fn smallest_hit_selection_replaces_lists_deduplicates_and_then_limits() {
        let item = hit("a.md", "$/list[0]/item[1]", 2.0);
        let mut candidates = vec![
            CandidateHit {
                hit: hit("a.md", "$/list[0]", 8.0),
                narrower: Some(item.clone()),
            },
            CandidateHit {
                hit: item,
                narrower: None,
            },
            CandidateHit {
                hit: hit("a.md", "$/list[1]", 1.5),
                narrower: None,
            },
        ];
        for index in 0..10 {
            candidates.push(CandidateHit {
                hit: hit(
                    "a.md",
                    &format!("$.higher-{index}"),
                    10.0 - index as f32 / 10.0,
                ),
                narrower: None,
            });
        }

        let selected = select_smallest_hits(candidates, 12);
        assert_eq!(selected.len(), 12);
        assert_eq!(
            selected
                .iter()
                .filter(|hit| hit.mdpath == "$/list[0]/item[1]")
                .count(),
            1,
            "the replacement and directly ranked item collapse to one hit"
        );
        assert!(!selected.iter().any(|hit| hit.mdpath == "$/list[0]"));
        assert!(selected.iter().any(|hit| hit.mdpath == "$/list[1]"));

        let selected = select_smallest_hits(
            vec![CandidateHit {
                hit: hit("a.md", "$/list[0]", 20.0),
                narrower: Some(hit("a.md", "$/list[0]/item[1]", 0.1)),
            }]
            .into_iter()
            .chain((0..10).map(|index| CandidateHit {
                hit: hit(
                    "a.md",
                    &format!("$.higher-{index}"),
                    10.0 - index as f32 / 10.0,
                ),
                narrower: None,
            }))
            .collect(),
            10,
        );
        assert!(!selected
            .iter()
            .any(|hit| hit.mdpath.starts_with("$/list[0]")));
    }
}
