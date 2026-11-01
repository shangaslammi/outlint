//! Read output in human, JSON, and compact forms.

use std::borrow::Cow;

use outlint_core::{
    BlockKind, CanonicalDocumentPath, DocumentContent, DocumentReadFailure as ReadFailure,
    DocumentTree as ReadTree, DocumentTreeNode as TreeNodeData, DocumentTreeRow as TreeNode,
};
use serde_json::json;

use crate::{args::ReadSearchFormat, read::ReadOutput};

use super::{
    escape_compact, escape_human, format_bytes,
    json::{finish_json_line, write_ordered_json_array, write_ordered_json_object},
};

const READ_ENVELOPE_VERSION: u64 = 1;

/// Renders a resolved read result in the selected format.
pub(crate) fn output(value: &ReadOutput, file: &str, format: ReadSearchFormat) -> String {
    match (value, format) {
        (ReadOutput::Content(content), ReadSearchFormat::Human | ReadSearchFormat::Compact) => {
            content.content.clone()
        }
        (ReadOutput::Content(content), ReadSearchFormat::Json) => json_content(content, file),
        (ReadOutput::Tree(tree), ReadSearchFormat::Human) => human_tree(&tree.nodes, file),
        (ReadOutput::Tree(tree), ReadSearchFormat::Json) => json_tree(tree, file),
        (ReadOutput::Tree(tree), ReadSearchFormat::Compact) => compact_tree(&tree.nodes, file),
    }
}

/// Renders a path failure. JSON is a stdout object; the other forms are
/// plain-text error lines for stderr.
pub(crate) fn error(value: &ReadFailure, file: &str, format: ReadSearchFormat) -> String {
    match format {
        ReadSearchFormat::Human => human_error(value, file),
        ReadSearchFormat::Json => json_error(value),
        ReadSearchFormat::Compact => compact_error(value, file),
    }
}

fn node_kind(node: &TreeNodeData) -> &'static str {
    match node {
        TreeNodeData::Root { .. } => "root",
        TreeNodeData::Section { .. } => "section",
        TreeNodeData::List { .. } => "list",
        TreeNodeData::Paragraph { .. } => "p",
        TreeNodeData::Item { .. } => "item",
        TreeNodeData::Block(kind) => match kind {
            BlockKind::Paragraph => "p",
            BlockKind::List => "list",
            BlockKind::Code => "code",
            BlockKind::Quote => "quote",
            BlockKind::Html => "html",
            BlockKind::Break => "break",
        },
        _ => "block",
    }
}

fn node_label<'a>(node: &'a TreeNodeData, file: &'a str) -> Cow<'a, str> {
    match node {
        TreeNodeData::Root { title } => title.as_ref().map_or(file, |(_, text)| text).into(),
        TreeNodeData::Section { text, .. } => text.into(),
        TreeNodeData::List { items } => format!("list ({items} items)").into(),
        TreeNodeData::Paragraph { preview } | TreeNodeData::Item { preview } => {
            format!("{}: {preview}", node_kind(node)).into()
        }
        TreeNodeData::Block(_) => node_kind(node).into(),
        _ => "".into(),
    }
}

fn human_tree(nodes: &[TreeNode], file: &str) -> String {
    let rows: Vec<_> = nodes
        .iter()
        .map(|node| {
            (
                escape_human(&node.mdpath.to_string()),
                escape_human(&node_label(&node.node, file)),
                node.bytes,
            )
        })
        .collect();
    let width = rows
        .iter()
        .map(|(path, _, _)| path.len())
        .max()
        .unwrap_or(0);
    let mut output = String::new();
    for (path, label, bytes) in rows {
        output.push_str(&format!(
            "{:<width$}  {}  {}\n",
            path,
            format_bytes(bytes),
            label
        ));
    }
    output
}

fn human_error(value: &ReadFailure, file: &str) -> String {
    let mut output = match value {
        ReadFailure::Syntax {
            path,
            message,
            offset,
            ..
        } => format!(
            "outlint: invalid document path '{}': {} at byte {}\n",
            escape_human(path),
            escape_human(message),
            offset.0
        ),
        ReadFailure::Unresolved {
            path,
            step,
            resolved,
            nodes,
            ..
        } => format!(
            "outlint: cannot resolve {} in {}: {} not found under {}\n{}",
            escape_human(path),
            escape_human(file),
            escape_human(step),
            escape_human(&resolved.to_string()),
            human_tree(nodes, file)
        ),
        ReadFailure::Ambiguous {
            path,
            step,
            candidates,
            ..
        } => {
            let mut output = format!(
                "outlint: ambiguous document path {} in {}: {} sections match '{}'\n",
                escape_human(path),
                escape_human(file),
                candidates.len(),
                escape_human(step)
            );
            output.push_str(&human_tree(candidates, file));
            output
        }
        ReadFailure::Other { path, message, .. } => format!(
            "outlint: cannot resolve {} in {}: {}\n",
            escape_human(path),
            escape_human(file),
            escape_human(message)
        ),
        other => format!("outlint: {}\n", escape_human(&other.to_string())),
    };
    append_suggestions(&mut output, failure_suggestions(value), escape_human);
    output
}

fn json_tree(tree: &ReadTree, file: &str) -> String {
    let mut output = Vec::new();
    // Read objects follow their documented wire order; do not route these
    // through `serde_json::Map`, whose default representation sorts keys.
    write_ordered_json_object(&mut output, |object| {
        object.value("version", &json!(READ_ENVELOPE_VERSION));
        object.value("file", &json!(file));
        object.value("mdpath", &json!(tree.mdpath.to_string()));
        object.member("nodes", |output| {
            write_ordered_json_array(output, &tree.nodes, write_node_json)
        });
    });
    finish_json_line(output)
}

fn json_content(content: &DocumentContent, file: &str) -> String {
    let mut output = Vec::new();
    write_ordered_json_object(&mut output, |object| {
        object.value("version", &json!(READ_ENVELOPE_VERSION));
        object.value("file", &json!(file));
        object.value("mdpath", &json!(content.mdpath.to_string()));
        object.value("bytes", &json!(content.bytes));
        object.value("content", &json!(content.content));
    });
    finish_json_line(output)
}

fn write_node_json(output: &mut Vec<u8>, node: &TreeNode) {
    write_ordered_json_object(output, |object| {
        object.value("mdpath", &json!(node.mdpath.to_string()));
        object.value("kind", &json!(node_kind(&node.node)));
        match &node.node {
            TreeNodeData::Root { title } => {
                object.value(
                    "level",
                    &json!(title.as_ref().map(|(level, _)| *level as u8)),
                );
                object.value("text", &json!(title.as_ref().map(|(_, text)| text)));
            }
            TreeNodeData::Section { level, text } => {
                object.value("level", &json!(*level as u8));
                object.value("text", &json!(text));
            }
            TreeNodeData::List { items } => {
                object.value("items", &json!(items));
            }
            TreeNodeData::Paragraph { preview } | TreeNodeData::Item { preview } => {
                object.value("preview", &json!(preview));
            }
            _ => object.value("preview", &json!("")),
        }
        object.value("bytes", &json!(node.bytes));
    });
}

fn json_error(value: &ReadFailure) -> String {
    let mut output = Vec::new();
    write_ordered_json_object(&mut output, |object| {
        object.value("version", &json!(READ_ENVELOPE_VERSION));
        object.member("error", |output| write_error_json(output, value));
    });
    finish_json_line(output)
}

fn write_error_json(output: &mut Vec<u8>, value: &ReadFailure) {
    let suggestions = serde_json::Value::Array(
        failure_suggestions(value)
            .iter()
            .map(|path| serde_json::Value::String(path.to_string()))
            .collect(),
    );
    write_ordered_json_object(output, |object| match value {
        ReadFailure::Syntax {
            path,
            message,
            offset,
            ..
        } => {
            object.value("kind", &json!("syntax"));
            object.value("path", &json!(path));
            object.value("message", &json!(message));
            object.value("offset", &json!(offset.0));
            object.value("suggestions", &suggestions);
        }
        ReadFailure::Unresolved {
            path,
            step,
            resolved,
            nodes,
            ..
        } => {
            object.value("kind", &json!("unresolved"));
            object.value("path", &json!(path));
            object.value("step", &json!(step));
            object.value("resolved", &json!(resolved.to_string()));
            object.member("nodes", |output| {
                write_ordered_json_array(output, nodes, write_node_json)
            });
            object.value("suggestions", &suggestions);
        }
        ReadFailure::Ambiguous {
            path,
            step,
            resolved,
            candidates,
            ..
        } => {
            object.value("kind", &json!("ambiguous"));
            object.value("path", &json!(path));
            object.value("step", &json!(step));
            object.value("resolved", &json!(resolved.to_string()));
            object.member("candidates", |output| {
                write_ordered_json_array(output, candidates, write_node_json)
            });
            object.value("suggestions", &suggestions);
        }
        ReadFailure::Other { path, message, .. } => {
            object.value("kind", &json!("other"));
            object.value("path", &json!(path));
            object.value("message", &json!(message));
            object.value("suggestions", &suggestions);
        }
        other => {
            object.value("kind", &json!("other"));
            object.value("message", &json!(other.to_string()));
            object.value("suggestions", &suggestions);
        }
    });
}

fn compact_tree(nodes: &[TreeNode], file: &str) -> String {
    let mut output = String::new();
    for node in nodes {
        output.push_str(&escape_compact(&node.mdpath.to_string()));
        output.push('\t');
        output.push_str(&format_bytes(node.bytes));
        output.push('\t');
        output.push_str(&escape_compact(&node_label(&node.node, file)));
        output.push('\n');
    }
    output
}

fn compact_error(value: &ReadFailure, file: &str) -> String {
    let mut output = match value {
        ReadFailure::Syntax {
            path,
            message,
            offset,
            ..
        } => format!(
            "outlint: invalid document path '{}': {} at byte {}\n",
            escape_compact(path),
            escape_compact(message),
            offset.0
        ),
        ReadFailure::Unresolved {
            path,
            step,
            resolved,
            nodes,
            ..
        } => format!(
            "outlint: cannot resolve {} in {}: {} not found under {}\n{}",
            escape_compact(path),
            escape_compact(file),
            escape_compact(step),
            escape_compact(&resolved.to_string()),
            compact_tree(nodes, file)
        ),
        ReadFailure::Ambiguous {
            path,
            step,
            candidates,
            ..
        } => format!(
            "outlint: ambiguous document path {} in {}: {} sections match '{}'\n{}",
            escape_compact(path),
            escape_compact(file),
            candidates.len(),
            escape_compact(step),
            compact_tree(candidates, file)
        ),
        ReadFailure::Other { path, message, .. } => format!(
            "outlint: cannot resolve {} in {}: {}\n",
            escape_compact(path),
            escape_compact(file),
            escape_compact(message)
        ),
        other => format!("outlint: {}\n", escape_compact(&other.to_string())),
    };
    append_suggestions(&mut output, failure_suggestions(value), escape_compact);
    output
}

fn failure_suggestions(value: &ReadFailure) -> &[CanonicalDocumentPath] {
    match value {
        ReadFailure::Syntax { suggestions, .. } | ReadFailure::Unresolved { suggestions, .. } => {
            suggestions
        }
        _ => &[],
    }
}

fn append_suggestions(
    output: &mut String,
    suggestions: &[CanonicalDocumentPath],
    escape: fn(&str) -> String,
) {
    for suggestion in suggestions {
        output.push_str("did you mean ");
        output.push_str(&escape(&suggestion.to_string()));
        output.push_str("?\n");
    }
}

#[cfg(test)]
mod tests {
    use crate::read::path_spelling;
    use outlint_core::{
        document_content, document_tree, parse_markdown, ByteOffset, Document, DocumentPath,
        HeaderLevel, MarkdownOptions,
    };
    use serde_json::Value;

    use super::*;

    fn canonical_path(spelling: &str) -> CanonicalDocumentPath {
        let document = outlint_core::parse_markdown(
            "## Setup\n\nInstall first.\n\n## Heading\n\nPreview.\n\n## Resolved\n\n## Bad Path\n",
            outlint_core::MarkdownOptions::default(),
        )
        .expect("fixture parses");
        outlint_core::document_paths(&document)
            .into_iter()
            .map(|(path, _)| path)
            .find(|path| path.to_string() == spelling)
            .expect("fixture path exists")
    }

    fn nodes() -> Vec<TreeNode> {
        vec![
            TreeNode {
                mdpath: canonical_path("$.setup"),
                bytes: 3_140,
                node: TreeNodeData::Section {
                    level: HeaderLevel::H2,
                    text: "Setup".into(),
                },
            },
            TreeNode {
                mdpath: canonical_path("$.setup/p[0]"),
                bytes: 24,
                node: TreeNodeData::Paragraph {
                    preview: "Install first.".into(),
                },
            },
        ]
    }

    #[test]
    fn tree_formats_share_the_same_rows() {
        assert_eq!(
            human_tree(&nodes(), "docs/guide.md"),
            "$.setup       3.1kB  Setup\n$.setup/p[0]  24B  p: Install first.\n"
        );
        assert_eq!(
            compact_tree(&nodes(), "docs/guide.md"),
            "$.setup\t3.1kB\tSetup\n$.setup/p[0]\t24B\tp: Install first.\n"
        );
        let tree = ReadTree {
            mdpath: canonical_path("$.setup"),
            nodes: nodes(),
        };
        let rendered = json_tree(&tree, "docs/guide.md");
        assert_eq!(
            rendered,
            "{\"version\":1,\"file\":\"docs/guide.md\",\"mdpath\":\"$.setup\",\"nodes\":[{\"mdpath\":\"$.setup\",\"kind\":\"section\",\"level\":2,\"text\":\"Setup\",\"bytes\":3140},{\"mdpath\":\"$.setup/p[0]\",\"kind\":\"p\",\"preview\":\"Install first.\",\"bytes\":24}]}\n"
        );
        let value: Value = serde_json::from_str(&rendered).expect("renderer emits valid JSON");
        assert_eq!(value["version"], 1);
        assert_eq!(value["nodes"][0]["level"], 2);
        assert_eq!(value["nodes"][1]["preview"], "Install first.");
    }

    #[test]
    fn path_errors_have_versioned_json_and_compact_rows() {
        let failure = ReadFailure::Unresolved {
            path: "$.setp".into(),
            step: "setp".into(),
            resolved: canonical_path("$"),
            nodes: nodes(),
            suggestions: vec![canonical_path("$.setup")],
        };
        let rendered = json_error(&failure);
        assert_eq!(
            rendered,
            "{\"version\":1,\"error\":{\"kind\":\"unresolved\",\"path\":\"$.setp\",\"step\":\"setp\",\"resolved\":\"$\",\"nodes\":[{\"mdpath\":\"$.setup\",\"kind\":\"section\",\"level\":2,\"text\":\"Setup\",\"bytes\":3140},{\"mdpath\":\"$.setup/p[0]\",\"kind\":\"p\",\"preview\":\"Install first.\",\"bytes\":24}],\"suggestions\":[\"$.setup\"]}}\n"
        );
        let value: Value = serde_json::from_str(&rendered).expect("renderer emits valid JSON");
        assert_eq!(value["error"]["kind"], "unresolved");
        assert_eq!(value["error"]["nodes"][0]["mdpath"], "$.setup");
        assert!(compact_error(&failure, "docs/guide.md").contains("$.setup\t3.1kB\tSetup\n"));
    }

    #[test]
    fn compact_tree_and_errors_escape_control_characters_and_backslashes() {
        let controls = "tab\tcr\rlf\nesc\u{1b}slash\\";
        let nodes = vec![
            TreeNode {
                mdpath: canonical_path("$.heading"),
                bytes: 10,
                node: TreeNodeData::Section {
                    level: HeaderLevel::H2,
                    text: format!("Heading {controls}"),
                },
            },
            TreeNode {
                mdpath: canonical_path("$.heading/p[0]"),
                bytes: 8,
                node: TreeNodeData::Paragraph {
                    preview: format!("Preview {controls}"),
                },
            },
        ];
        assert_eq!(
            compact_tree(&nodes, "docs/guide.md"),
            "$.heading\t10B\tHeading tab\\tcr\\u{d}lf\\nesc\\u{1b}slash\\\\\n\
             $.heading/p[0]\t8B\tp: Preview tab\\tcr\\u{d}lf\\nesc\\u{1b}slash\\\\\n"
        );

        let failure = ReadFailure::Unresolved {
            path: format!("$.path-{controls}"),
            step: format!("step-{controls}"),
            resolved: canonical_path("$.resolved"),
            nodes,
            suggestions: Vec::new(),
        };
        let rendered = compact_error(&failure, &format!("docs/{controls}.md"));
        assert_eq!(rendered.lines().count(), 3, "{rendered:?}");
        assert!(rendered.starts_with(
            "outlint: cannot resolve $.path-tab\\tcr\\u{d}lf\\nesc\\u{1b}slash\\\\ in docs/tab\\tcr\\u{d}lf\\nesc\\u{1b}slash\\\\.md: step-tab\\tcr\\u{d}lf\\nesc\\u{1b}slash\\\\ not found under $.resolved\n"
        ));
    }

    #[test]
    fn compact_syntax_and_other_errors_escape_all_values() {
        let syntax = ReadFailure::Syntax {
            path: "$\t.bad\\path".into(),
            message: "bad\r\npath\u{1b}".into(),
            offset: ByteOffset(2),
            suggestions: Vec::new(),
        };
        assert_eq!(
            compact_error(&syntax, "docs/guide.md"),
            "outlint: invalid document path '$\\t.bad\\\\path': bad\\u{d}\\npath\\u{1b} at byte 2\n"
        );

        let other = ReadFailure::Other {
            path: "$.bad\tpath".into(),
            message: "failure\u{1b}\\detail".into(),
        };
        assert_eq!(
            compact_error(&other, "docs/bad\nfile.md"),
            "outlint: cannot resolve $.bad\\tpath in docs/bad\\nfile.md: failure\\u{1b}\\\\detail\n"
        );
        assert_eq!(
            human_error(&other, "docs/bad\nfile.md"),
            "outlint: cannot resolve $.bad\\tpath in docs/bad\\nfile.md: failure\\x1b\\detail\n"
        );
        assert_eq!(
            json_error(&other),
            "{\"version\":1,\"error\":{\"kind\":\"other\",\"path\":\"$.bad\\tpath\",\"message\":\"failure\\u001b\\\\detail\",\"suggestions\":[]}}\n"
        );
    }

    #[test]
    fn human_tree_escapes_terminal_controls() {
        let mut values = nodes();
        values[0].mdpath = canonical_path("$.bad-path");
        values[0].node = TreeNodeData::Section {
            level: HeaderLevel::H2,
            text: "Heading\tlabel\u{202e}".into(),
        };
        assert_eq!(
            human_tree(&values, "docs/guide.md"),
            "$.bad-path    3.1kB  Heading\\tlabel\\u{202e}\n$.setup/p[0]  24B  p: Install first.\n"
        );
    }

    const FIXTURE: &str = "---\ntitle: Guide\n---\n\nRoot preamble paragraph.\n\n# Guide\n\nGuide intro.\n\n## Setup\n\nInstall the tool first.\n\n- first item\n- second item\n- third item\n\n```sh\noutlint check README.md\n```\n\n## FAQ\n\n### Question\n\nWhy?\n\n### Question\n\nHow?\n";

    fn document() -> Document {
        parse_markdown(FIXTURE, MarkdownOptions::default()).expect("fixture parses")
    }

    fn path(spelling: &str) -> DocumentPath {
        DocumentPath::parse(&path_spelling(spelling)).expect("fixture path parses")
    }

    fn render_content(
        source: &str,
        document: &Document,
        path: &DocumentPath,
        depth: Option<usize>,
    ) -> Result<String, Box<ReadFailure>> {
        document_content(source, document, &path.to_string(), depth).map(|value| value.content)
    }

    fn content(spelling: &str, depth: Option<usize>) -> Result<String, Box<ReadFailure>> {
        document_content(FIXTURE, &document(), &path_spelling(spelling), depth)
            .map(|value| value.content)
    }

    fn render_tree(
        file: &str,
        source: &str,
        document: &Document,
        path: &DocumentPath,
        depth: Option<usize>,
        blocks: bool,
    ) -> Result<String, Box<ReadFailure>> {
        document_tree(source, document, &path.to_string(), depth, blocks)
            .map(|tree| human_tree(&tree.nodes, file))
    }

    #[test]
    fn tree_with_blocks_lists_blocks_and_items_with_labels() {
        let listing = render_tree(
            "guide.md",
            FIXTURE,
            &document(),
            &path("$.setup"),
            None,
            true,
        )
        .expect("resolves");
        assert_eq!(
            listing,
            "$.setup                  109B  Setup\n\
             $.setup/p[0]             24B  p: Install the tool first.\n\
             $.setup/list[0]          40B  list (3 items)\n\
             $.setup/list[0]/item[0]  13B  item: first item\n\
             $.setup/list[0]/item[1]  14B  item: second item\n\
             $.setup/list[0]/item[2]  13B  item: third item\n\
             $.setup/code[0]          33B  code\n"
        );
    }

    #[test]
    fn unresolved_step_lists_the_deepest_resolved_node() {
        let error = content("$.setp", None).expect_err("does not resolve");
        assert_eq!(
            human_error(&error, "guide.md"),
            "outlint: cannot resolve $.setp in guide.md: setp not found under $\n\
             $        229B  Guide\n\
             $.setup  109B  Setup\n\
             $.faq    47B  FAQ\n\
             did you mean $.setup?\n"
        );
    }

    #[test]
    fn ambiguous_step_lists_the_candidates() {
        let error = content("$.faq.question", None).expect_err("is ambiguous");
        assert_eq!(
            human_error(&error, "guide.md"),
            "outlint: ambiguous document path $.faq.question in guide.md: 2 sections match 'question'\n\
             $.faq.question[0]  19B  Question\n\
             $.faq.question[1]  19B  Question\n"
        );
    }

    #[test]
    fn descendant_steps_report_matches_across_the_subtree() {
        let ambiguous = content("$..question", None).expect_err("is ambiguous");
        assert_eq!(
            human_error(&ambiguous, "guide.md"),
            "outlint: ambiguous document path $..question in guide.md: 2 sections match '..question'\n\
             $.faq.question[0]  19B  Question\n\
             $.faq.question[1]  19B  Question\n"
        );
        assert_eq!(
            content("$..question[1]", None).as_deref(),
            Ok("### Question\n\nHow?\n")
        );
        let unresolved = content("$.setup..question", None).expect_err("does not resolve");
        assert!(human_error(&unresolved, "guide.md").starts_with(
            "outlint: cannot resolve $.setup..question in guide.md: ..question not found under $.setup\n"
        ));
    }

    #[test]
    fn error_paths_are_spelled_canonically() {
        // An H2 as the only top-level section is not merged into the root.
        const DUPLICATES: &str = "## Setup\n\n### Question\n\nWhy?\n\n### Question\n\nHow?\n";
        let document =
            parse_markdown(DUPLICATES, MarkdownOptions::default()).expect("fixture parses");
        let ambiguous = render_content(DUPLICATES, &document, &path("$.setup[0].question"), None)
            .expect_err("is ambiguous");
        assert_eq!(
            human_error(&ambiguous, "setup.md"),
            "outlint: ambiguous document path $.setup[0].question in setup.md: 2 sections match 'question'\n\
             $.setup.question[0]  19B  Question\n\
             $.setup.question[1]  19B  Question\n"
        );
        let unresolved = render_content(DUPLICATES, &document, &path("$.setup[0].missing"), None)
            .expect_err("does not resolve");
        assert!(
            human_error(&unresolved, "setup.md").starts_with(
                "outlint: cannot resolve $.setup[0].missing in setup.md: missing not found under $.setup\n\
                 $.setup  "
            )
        );
    }

    #[test]
    fn syntax_error_names_the_offset() {
        let error = content(".Guide", None).expect_err("uppercase is not a slug");
        assert!(human_error(&error, "guide.md")
            .starts_with("outlint: invalid document path '$.Guide': "));
    }
}
