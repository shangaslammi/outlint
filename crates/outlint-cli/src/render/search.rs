//! Search output in human, JSON, and compact forms.

#[cfg(test)]
use outlint_search::FiniteScore;
use outlint_search::{Hit, TermCount};
use serde_json::json;

use super::{
    escape_compact, format_bytes,
    json::{finish_json_line, write_ordered_json_array, write_ordered_json_object},
};

const VERSION: u64 = 1;

/// Renders successful search hits in the selected machine or human format.
pub(crate) fn hits(query: &str, values: &[Hit], format: crate::args::AgentFormat) -> String {
    match format {
        crate::args::AgentFormat::Human => human_hits(values),
        crate::args::AgentFormat::Json => json_hits(query, values),
        crate::args::AgentFormat::Compact => compact_hits(values),
    }
}

/// Renders an empty search result. Human and compact output are stderr notes;
/// JSON remains the invocation's single stdout object.
pub(crate) fn no_hits(
    query: &str,
    counts: Option<&[TermCount]>,
    format: crate::args::AgentFormat,
) -> String {
    match format {
        crate::args::AgentFormat::Human => human_no_hits(query, counts),
        crate::args::AgentFormat::Json => json_no_hits(query, counts),
        crate::args::AgentFormat::Compact => compact_no_hits(counts),
    }
}

fn human_hits(hits: &[Hit]) -> String {
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

fn human_no_hits(query: &str, counts: Option<&[TermCount]>) -> String {
    let Some(counts) = counts.filter(|counts| !counts.is_empty()) else {
        return format!("outlint: no search hits for: {query}\n");
    };
    let mut output = format!("outlint: no block contains all of: {query}\n  ");
    append_counts(&mut output, counts, ", ");
    output.push_str("\n  (a block must contain every word; drop or change the rarest words)\n");
    output
}

fn json_hits(query: &str, hits: &[Hit]) -> String {
    let mut output = Vec::new();
    // Search objects follow their documented wire order; do not route these
    // through `serde_json::Map`, whose default representation sorts keys.
    write_ordered_json_object(&mut output, |object| {
        object.value("version", &json!(VERSION));
        object.value("query", &json!(query));
        object.member("hits", |output| {
            write_ordered_json_array(output, hits, write_hit_json)
        });
    });
    finish_json_line(output)
}

fn json_no_hits(query: &str, counts: Option<&[TermCount]>) -> String {
    let mut output = Vec::new();
    write_ordered_json_object(&mut output, |object| {
        object.value("version", &json!(VERSION));
        object.value("query", &json!(query));
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
        object.value("score", &json!(hit.score.get()));
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
        output.push_str(&count.term);
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
                score: FiniteScore::new(7.31).expect("fixture score is finite"),
                snippet: "Run the setup script, then…".into(),
            },
            Hit {
                path: "docs/notes.md".into(),
                mdpath: "$.decision-outcome".into(),
                bytes: 900,
                section_bytes: None,
                score: FiniteScore::new(1.0).expect("fixture score is finite"),
                snippet: "Chosen option: keep the current layout.".into(),
            },
            Hit {
                path: "docs/notes.md".into(),
                mdpath: "$.options".into(),
                bytes: 300,
                section_bytes: None,
                score: FiniteScore::new(0.5).expect("fixture score is finite"),
                snippet: String::new(),
            },
        ]
    }

    #[test]
    fn human_output_preserves_the_prototype_layout() {
        assert_eq!(
            human_hits(&fixture_hits()),
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
        let rendered = json_hits("setup", &hits[..1]);
        assert_eq!(
            rendered,
            "{\"version\":1,\"query\":\"setup\",\"hits\":[{\"path\":\"docs/guide.md\",\"mdpath\":\"$.setup/p[0]\",\"bytes\":412,\"section_bytes\":3140,\"score\":7.31,\"snippet\":\"Run the setup script, then…\"}]}\n"
        );
        let value: Value = serde_json::from_str(&rendered).expect("renderer emits JSON");
        assert_eq!(value["version"], 1);
        assert_eq!(value["hits"][0]["section_bytes"], 3_140);
        assert!(value.get("term_counts").is_none());

        let counts = [TermCount {
            term: "missing".into(),
            blocks: 0,
        }];
        let rendered = json_no_hits("missing", Some(&counts));
        assert_eq!(
            rendered,
            "{\"version\":1,\"query\":\"missing\",\"hits\":[],\"term_counts\":[{\"term\":\"missing\",\"blocks\":0}]}\n"
        );
        let value: Value = serde_json::from_str(&rendered).expect("renderer emits JSON");
        assert_eq!(value["hits"], json!([]));
        assert_eq!(value["term_counts"][0]["term"], "missing");

        let value: Value =
            serde_json::from_str(&json_no_hits("missing", None)).expect("renderer emits JSON");
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
}
