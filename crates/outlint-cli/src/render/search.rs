//! Search output in human, JSON, and compact forms.

use outlint_search::{Hit, TermCount};
use serde_json::json;

use super::{
    escape_compact, escape_human, format_bytes,
    json::{finish_json_line, write_ordered_json_array, write_ordered_json_object},
};

const SEARCH_ENVELOPE_VERSION: u64 = 1;

/// Renders successful search hits in the selected machine or human format.
pub(crate) fn hits(
    query: &str,
    values: &[Hit],
    total: usize,
    format: crate::args::ReadSearchFormat,
) -> String {
    match format {
        crate::args::ReadSearchFormat::Human => human_hits(values, total),
        crate::args::ReadSearchFormat::Json => json_hits(query, total, values),
        crate::args::ReadSearchFormat::Compact => compact_hits(values),
    }
}

/// Renders an empty search result. Human and compact output are stderr notes;
/// JSON remains the invocation's single stdout object.
pub(crate) fn no_hits(
    query: &str,
    total: usize,
    counts: Option<&[TermCount]>,
    format: crate::args::ReadSearchFormat,
) -> String {
    match format {
        crate::args::ReadSearchFormat::Human => human_no_hits(query, counts),
        crate::args::ReadSearchFormat::Json => json_no_hits(query, total, counts),
        crate::args::ReadSearchFormat::Compact => compact_no_hits(counts),
    }
}

fn human_hits(hits: &[Hit], total: usize) -> String {
    let mut output = String::new();
    for hit in hits {
        output.push_str(&format!(
            "{} {} {}",
            escape_human(&hit.path),
            escape_human(&hit.mdpath),
            format_bytes(hit.bytes)
        ));
        if let Some(section_bytes) = hit.section_bytes {
            output.push_str(&format!(" (section {})", format_bytes(section_bytes)));
        }
        output.push('\n');
        if !hit.snippet.is_empty() {
            output.push_str("  ");
            output.push_str(&escape_human(&hit.snippet));
            output.push('\n');
        }
        output.push('\n');
    }
    if let Some(note) = truncation_note(hits.len(), total) {
        output.push_str(&note);
    }
    output
}

/// The limit note for stderr in compact mode. Human output incorporates the
/// same line in stdout; JSON carries the counts structurally.
pub(crate) fn compact_truncation_note(shown: usize, total: usize) -> Option<String> {
    truncation_note(shown, total)
}

fn truncation_note(shown: usize, total: usize) -> Option<String> {
    (shown < total)
        .then(|| format!("({shown} of {total} matching blocks; use --limit to see more)\n"))
}

fn human_no_hits(query: &str, counts: Option<&[TermCount]>) -> String {
    let Some(counts) = counts.filter(|counts| !counts.is_empty()) else {
        return format!("outlint: no search hits for: {}\n", escape_human(query));
    };
    let mut output = format!(
        "outlint: no block contains all of: {}\n  ",
        escape_human(query)
    );
    append_counts(&mut output, counts, ", ");
    output.push_str("\n  (a block must contain every word; drop or change the rarest words)\n");
    output
}

fn json_hits(query: &str, total: usize, hits: &[Hit]) -> String {
    let mut output = Vec::new();
    // Search objects follow their documented wire order; do not route these
    // through `serde_json::Map`, whose default representation sorts keys.
    write_ordered_json_object(&mut output, |object| {
        object.value("version", &json!(SEARCH_ENVELOPE_VERSION));
        object.value("query", &json!(query));
        object.value("total", &json!(total));
        object.member("hits", |output| {
            write_ordered_json_array(output, hits, write_hit_json)
        });
    });
    finish_json_line(output)
}

fn json_no_hits(query: &str, total: usize, counts: Option<&[TermCount]>) -> String {
    let mut output = Vec::new();
    write_ordered_json_object(&mut output, |object| {
        object.value("version", &json!(SEARCH_ENVELOPE_VERSION));
        object.value("query", &json!(query));
        object.value("total", &json!(total));
        object.member("hits", |output| output.extend_from_slice(b"[]"));
        object.member("term_counts", |output| match counts {
            Some(counts) => write_ordered_json_array(output, counts, write_term_count_json),
            None => output.extend_from_slice(b"null"),
        });
    });
    finish_json_line(output)
}

fn write_hit_json(output: &mut Vec<u8>, hit: &Hit) {
    write_ordered_json_object(output, |object| {
        object.value("path", &json!(hit.path));
        object.value("mdpath", &json!(hit.mdpath));
        object.value("bytes", &json!(hit.bytes));
        object.value("section_bytes", &json!(hit.section_bytes));
        object.value("score", &json!(hit.score));
        object.value("snippet", &json!(hit.snippet));
    });
}

fn write_term_count_json(output: &mut Vec<u8>, count: &TermCount) {
    write_ordered_json_object(output, |object| {
        object.value("term", &json!(count.term));
        object.value("blocks", &json!(count.blocks));
    });
}

fn compact_hits(hits: &[Hit]) -> String {
    let mut output = String::new();
    for hit in hits {
        output.push_str(&escape_compact(&hit.path));
        output.push('\t');
        output.push_str(&escape_compact(&hit.mdpath));
        output.push('\t');
        output.push_str(&format_bytes(hit.bytes));
        if let Some(section_bytes) = hit.section_bytes {
            output.push('/');
            output.push_str(&format_bytes(section_bytes));
        }
        output.push('\t');
        output.push_str(&escape_compact(&hit.snippet));
        output.push('\n');
    }
    output
}

fn compact_no_hits(counts: Option<&[TermCount]>) -> String {
    let Some(counts) = counts.filter(|counts| !counts.is_empty()) else {
        return "outlint: no search hits\n".to_owned();
    };
    let mut output = "outlint: no block contains all words; ".to_owned();
    for (index, count) in counts.iter().enumerate() {
        if index > 0 {
            output.push_str(", ");
        }
        output.push_str(&escape_compact(&count.term));
        output.push(' ');
        output.push_str(&count.blocks.to_string());
    }
    output.push('\n');
    output
}

fn append_counts(output: &mut String, counts: &[TermCount], separator: &str) {
    for (index, count) in counts.iter().enumerate() {
        if index > 0 {
            output.push_str(separator);
        }
        output.push_str(&escape_human(&count.term));
        output.push(' ');
        output.push_str(&count.blocks.to_string());
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;

    fn fixture_hits() -> Vec<Hit> {
        vec![
            Hit {
                path: "docs/guide.md".into(),
                mdpath: "$.setup/p[0]".into(),
                bytes: 412,
                section_bytes: Some(3_140),
                score: 7.31,
                snippet: "Run the setup script, then…".into(),
            },
            Hit {
                path: "docs/notes.md".into(),
                mdpath: "$.decision-outcome".into(),
                bytes: 900,
                section_bytes: None,
                score: 1.0,
                snippet: "Chosen option: keep the current layout.".into(),
            },
            Hit {
                path: "docs/notes.md".into(),
                mdpath: "$.options".into(),
                bytes: 300,
                section_bytes: None,
                score: 0.5,
                snippet: String::new(),
            },
        ]
    }

    #[test]
    fn human_output_uses_the_documented_layout() {
        assert_eq!(
            human_hits(&fixture_hits(), 3),
            "docs/guide.md $.setup/p[0] 412B (section 3.1kB)\n  Run the setup script, then…\n\n\
             docs/notes.md $.decision-outcome 900B\n  Chosen option: keep the current layout.\n\n\
             docs/notes.md $.options 300B\n\n"
        );
        let counts = [TermCount {
            term: "missing".into(),
            blocks: 0,
        }];
        assert_eq!(
            human_no_hits("missing", Some(&counts)),
            "outlint: no block contains all of: missing\n  missing 0\n  (a block must contain every word; drop or change the rarest words)\n"
        );
        assert_eq!(
            human_no_hits("\"missing phrase\"", None),
            "outlint: no search hits for: \"missing phrase\"\n"
        );
    }

    #[test]
    fn json_has_versioned_hits_and_no_hit_counts() {
        let hits = fixture_hits();
        let rendered = json_hits("setup", 1, &hits[..1]);
        assert_eq!(
            rendered,
            "{\"version\":1,\"query\":\"setup\",\"total\":1,\"hits\":[{\"path\":\"docs/guide.md\",\"mdpath\":\"$.setup/p[0]\",\"bytes\":412,\"section_bytes\":3140,\"score\":7.31,\"snippet\":\"Run the setup script, then…\"}]}\n"
        );
        let value: Value = serde_json::from_str(&rendered).expect("renderer emits JSON");
        assert_eq!(value["version"], 1);
        assert_eq!(value["hits"][0]["section_bytes"], 3_140);
        assert!(value.get("term_counts").is_none());

        let counts = [TermCount {
            term: "missing".into(),
            blocks: 0,
        }];
        let rendered = json_no_hits("missing", 0, Some(&counts));
        assert_eq!(
            rendered,
            "{\"version\":1,\"query\":\"missing\",\"total\":0,\"hits\":[],\"term_counts\":[{\"term\":\"missing\",\"blocks\":0}]}\n"
        );
        let value: Value = serde_json::from_str(&rendered).expect("renderer emits JSON");
        assert_eq!(value["hits"], json!([]));
        assert_eq!(value["term_counts"][0]["term"], "missing");

        let value: Value =
            serde_json::from_str(&json_no_hits("missing", 0, None)).expect("renderer emits JSON");
        assert!(value["term_counts"].is_null());
    }

    #[test]
    fn compact_output_is_one_escaped_record_per_hit() {
        let mut hits = fixture_hits();
        hits.truncate(1);
        hits[0].path = "docs/tab\tcr\rlf\nesc\u{1b}slash\\.md".into();
        hits[0].mdpath = "$.path\tcr\rlf\nesc\u{1b}slash\\".into();
        hits[0].snippet = "first\tcr\rsecond\nthird\u{1b}slash\\".into();
        assert_eq!(
            compact_hits(&hits),
            "docs/tab\\tcr\\u{d}lf\\nesc\\u{1b}slash\\\\.md\t$.path\\tcr\\u{d}lf\\nesc\\u{1b}slash\\\\\t412B/3.1kB\tfirst\\tcr\\u{d}second\\nthird\\u{1b}slash\\\\\n"
        );
    }

    #[test]
    fn human_output_escapes_terminal_controls() {
        let mut hits = fixture_hits();
        hits.truncate(1);
        hits[0].path = "docs/bad\nfile.md".into();
        hits[0].mdpath = "$.bad\tpath".into();
        hits[0].snippet = "line\rbreak\u{1b}".into();
        assert_eq!(
            human_hits(&hits, 1),
            "docs/bad\\nfile.md $.bad\\tpath 412B (section 3.1kB)\n  line\\rbreak\\x1b\n\n"
        );
        assert_eq!(
            human_no_hits("bad\nquery", None),
            "outlint: no search hits for: bad\\nquery\n"
        );
    }

    #[test]
    fn truncated_results_report_the_matching_block_total() {
        let hits = fixture_hits();
        assert!(human_hits(&hits[..1], 87)
            .ends_with("(1 of 87 matching blocks; use --limit to see more)\n"));
        assert_eq!(
            compact_truncation_note(10, 87).as_deref(),
            Some("(10 of 87 matching blocks; use --limit to see more)\n")
        );
        assert_eq!(compact_truncation_note(10, 10), None);
    }
}
