//! Pure rendering of search hits.

use std::cmp::Ordering;

/// One matching index unit, as loaded back from the index.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    /// Workspace-relative file path with forward slashes.
    pub path: String,
    /// Rendered document path of the unit inside the file.
    pub mdpath: String,
    /// Length in bytes of the unit's full extent.
    pub bytes: u64,
    /// Extent length of the enclosing section, for a block unit.
    pub section_bytes: Option<u64>,
    /// The engine's relevance score; higher is better.
    pub score: f32,
    /// The unit's source slice.
    pub raw: String,
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

/// Renders hits in the given order: a `<path> <mdpath> <size>` header — with
/// ` (section <size>)` appended for a block, so the reader can judge how much
/// context surrounds it — the source slice indented by two spaces, then a
/// blank line.
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
        for line in hit.raw.lines() {
            output.push_str("  ");
            output.push_str(line);
            output.push('\n');
        }
        output.push('\n');
    }
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
    fn render_hits_formats_header_indented_source_and_blank_line() {
        let hits = [
            Hit {
                path: "docs/a.md".into(),
                mdpath: "$.setup/p[0]".into(),
                bytes: 412,
                section_bytes: Some(3_140),
                score: 2.0,
                raw: "First line.\nSecond line.\n".into(),
            },
            Hit {
                path: "b.md".into(),
                mdpath: "$.intro".into(),
                bytes: 8,
                section_bytes: None,
                score: 1.0,
                raw: "# Intro\n".into(),
            },
        ];
        assert_eq!(
            render_hits(&hits),
            "docs/a.md $.setup/p[0] 412B (section 3.1kB)\n  First line.\n  Second line.\n\nb.md $.intro 8B\n  # Intro\n\n"
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
            raw: String::new(),
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
}
