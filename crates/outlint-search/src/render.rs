//! Pure rendering of search hits.

use std::cmp::Ordering;

/// One matching index unit, as loaded back from the index.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    /// Workspace-relative file path with forward slashes.
    pub path: String,
    /// Rendered document path of the unit inside the file.
    pub mdpath: String,
    /// One-based anchor line of the unit.
    pub line: u64,
    /// The engine's relevance score; higher is better.
    pub score: f32,
    /// The unit's source slice.
    pub raw: String,
}

/// Orders hits by descending score, then path, then line, so output is stable
/// across runs with equal scores.
pub(crate) fn sort_hits(hits: &mut [Hit]) {
    hits.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.path.cmp(&right.path))
            .then_with(|| left.line.cmp(&right.line))
    });
}

/// Renders hits in the given order: a `<path> <mdpath> L<line>` header, the
/// source slice indented by two spaces, then a blank line.
pub fn render_hits(hits: &[Hit]) -> String {
    let mut output = String::new();
    for hit in hits {
        output.push_str(&format!("{} {} L{}\n", hit.path, hit.mdpath, hit.line));
        for line in hit.raw.lines() {
            output.push_str("  ");
            output.push_str(line);
            output.push('\n');
        }
        output.push('\n');
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_hits_formats_header_indented_source_and_blank_line() {
        let hits = [
            Hit {
                path: "docs/a.md".into(),
                mdpath: "$.setup/p[0]".into(),
                line: 5,
                score: 2.0,
                raw: "First line.\nSecond line.\n".into(),
            },
            Hit {
                path: "b.md".into(),
                mdpath: "$.intro".into(),
                line: 1,
                score: 1.0,
                raw: "# Intro\n".into(),
            },
        ];
        assert_eq!(
            render_hits(&hits),
            "docs/a.md $.setup/p[0] L5\n  First line.\n  Second line.\n\nb.md $.intro L1\n  # Intro\n\n"
        );
    }

    #[test]
    fn sort_hits_orders_by_score_then_path_then_line() {
        let hit = |path: &str, line, score| Hit {
            path: path.into(),
            mdpath: "$".into(),
            line,
            score,
            raw: String::new(),
        };
        let mut hits = vec![
            hit("b.md", 2, 1.0),
            hit("a.md", 9, 1.0),
            hit("a.md", 3, 1.0),
            hit("z.md", 1, 5.0),
        ];
        sort_hits(&mut hits);
        let order: Vec<(&str, u64)> = hits
            .iter()
            .map(|hit| (hit.path.as_str(), hit.line))
            .collect();
        assert_eq!(order, [("z.md", 1), ("a.md", 3), ("a.md", 9), ("b.md", 2)]);
    }
}
