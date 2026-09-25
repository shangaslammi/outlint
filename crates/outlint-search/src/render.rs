//! Pure rendering of search hits.

use std::cmp::Ordering;

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
            .partial_cmp(&left.score)
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

/// Renders hits in the given order: a `<path> <mdpath> <size>` header — with
/// ` (section <size>)` appended for a block, so the reader can judge how much
/// context surrounds it — one snippet line indented by two spaces, omitted
/// when the snippet is empty, then a blank line. The full content is
/// `outlint read`'s job.
pub fn render_hits(hits: &[Hit]) -> String {
    let mut output = String::new();
    for hit in hits {
        output.push_str(&format!(
            "{} {} {}",
            hit.path,
            hit.mdpath,
            format_bytes(hit.bytes)
        ));
        if let Some(section_bytes) = hit.section_bytes {
            output.push_str(&format!(" (section {})", format_bytes(section_bytes)));
        }
        output.push('\n');
        if !hit.snippet.is_empty() {
            output.push_str("  ");
            output.push_str(&hit.snippet);
            output.push('\n');
        }
        output.push('\n');
    }
    output
}

/// Renders the stderr guidance for an empty result. Simple word queries carry
/// per-term counts; queries using parser syntax get a short generic note
/// because a word-by-word explanation would misrepresent their meaning.
pub fn render_no_hits(words: &str, counts: Option<&[TermCount]>) -> String {
    let Some(counts) = counts.filter(|counts| !counts.is_empty()) else {
        return format!("outlint: no search hits for: {words}\n");
    };
    let mut output = format!("outlint: no block contains all of: {words}\n  ");
    for (index, count) in counts.iter().enumerate() {
        if index > 0 {
            output.push_str(", ");
        }
        output.push_str(&format!("{} {}", count.term, count.blocks));
    }
    output.push_str("\n  (a block must contain every word; drop or change the rarest words)\n");
    output
}

/// A byte count as `<n>B` below 1000, else `<n.n>kB` below 1,000,000, else
/// `<n.n>MB`, each with one decimal and rounded half up.
fn format_bytes(bytes: u64) -> String {
    let tenths = |unit: u64| (bytes + unit / 20) / (unit / 10);
    if bytes < 1_000 {
        format!("{bytes}B")
    } else if bytes < 1_000_000 {
        let tenths = tenths(1_000);
        format!("{}.{}kB", tenths / 10, tenths % 10)
    } else {
        let tenths = tenths(1_000_000);
        format!("{}.{}MB", tenths / 10, tenths % 10)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_bytes_picks_the_unit_and_keeps_one_decimal() {
        assert_eq!(format_bytes(412), "412B");
        assert_eq!(format_bytes(3_140), "3.1kB");
        assert_eq!(format_bytes(2_450_000), "2.5MB");
    }

    #[test]
    fn render_hits_formats_header_indented_snippet_and_blank_line() {
        let hits = [
            Hit {
                path: "docs/a.md".into(),
                mdpath: "$.setup/p[0]".into(),
                bytes: 412,
                section_bytes: Some(3_140),
                score: 2.0,
                snippet: "Run the setup script, then…".into(),
            },
            Hit {
                path: "b.md".into(),
                mdpath: "$.decision-outcome".into(),
                bytes: 900,
                section_bytes: None,
                score: 1.0,
                snippet: "Chosen option: \"Section after outcome\", because it keeps…".into(),
            },
            Hit {
                path: "b.md".into(),
                mdpath: "$.options".into(),
                bytes: 300,
                section_bytes: None,
                score: 0.5,
                snippet: String::new(),
            },
        ];
        assert_eq!(
            render_hits(&hits),
            "docs/a.md $.setup/p[0] 412B (section 3.1kB)\n  Run the setup script, then…\n\n\
             b.md $.decision-outcome 900B\n  Chosen option: \"Section after outcome\", because it keeps…\n\n\
             b.md $.options 300B\n\n"
        );
    }

    #[test]
    fn sort_hits_orders_by_score_then_path_then_mdpath() {
        let hit = |path: &str, mdpath: &str, score| Hit {
            path: path.into(),
            mdpath: mdpath.into(),
            bytes: 0,
            section_bytes: None,
            score,
            snippet: String::new(),
        };
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
    fn no_hit_message_explains_simple_terms_and_falls_back_for_syntax() {
        let counts = [
            TermCount {
                term: "how".into(),
                blocks: 0,
            },
            TermCount {
                term: "codes".into(),
                blocks: 9,
            },
        ];
        assert_eq!(
            render_no_hits("how codes", Some(&counts)),
            "outlint: no block contains all of: how codes\n  how 0, codes 9\n  \
             (a block must contain every word; drop or change the rarest words)\n"
        );
        assert_eq!(
            render_no_hits("\"exit status\"", None),
            "outlint: no search hits for: \"exit status\"\n"
        );
    }

    #[test]
    fn smallest_hit_selection_replaces_lists_deduplicates_and_then_limits() {
        let hit = |mdpath: &str, score| Hit {
            path: "a.md".into(),
            mdpath: mdpath.into(),
            bytes: 0,
            section_bytes: None,
            score,
            snippet: String::new(),
        };
        let item = hit("$/list[0]/item[1]", 2.0);
        let mut candidates = vec![
            CandidateHit {
                hit: hit("$/list[0]", 8.0),
                narrower: Some(item.clone()),
            },
            CandidateHit {
                hit: item,
                narrower: None,
            },
            CandidateHit {
                hit: hit("$/list[1]", 1.5),
                narrower: None,
            },
        ];
        for index in 0..10 {
            candidates.push(CandidateHit {
                hit: hit(&format!("$.higher-{index}"), 10.0 - index as f32 / 10.0),
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
                hit: hit("$/list[0]", 20.0),
                narrower: Some(hit("$/list[0]/item[1]", 0.1)),
            }]
            .into_iter()
            .chain((0..10).map(|index| CandidateHit {
                hit: hit(&format!("$.higher-{index}"), 10.0 - index as f32 / 10.0),
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
