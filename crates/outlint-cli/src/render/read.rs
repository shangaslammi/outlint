//! Read output in human, JSON, and compact forms.

use serde_json::json;

use crate::{
    args::ReadSearchFormat,
    read::{ReadFailure, ReadOutput, ReadTree, TreeNode, TreeNodeData},
};

use super::{
    escape_compact, escape_human, format_bytes,
    json::{finish_json_line, write_ordered_json_array, write_ordered_json_object},
};

const READ_ENVELOPE_VERSION: u64 = 1;

/// Renders a resolved read result in the selected format.
pub(crate) fn output(value: &ReadOutput, format: ReadSearchFormat) -> String {
    match (value, format) {
        (ReadOutput::Content(content), ReadSearchFormat::Human | ReadSearchFormat::Compact) => {
            content.content.clone()
        }
        (ReadOutput::Content(content), ReadSearchFormat::Json) => json_content(content),
        (ReadOutput::Tree(tree), ReadSearchFormat::Human) => human_tree(&tree.nodes),
        (ReadOutput::Tree(tree), ReadSearchFormat::Json) => json_tree(tree),
        (ReadOutput::Tree(tree), ReadSearchFormat::Compact) => compact_tree(&tree.nodes),
    }
}

/// Renders a path failure. JSON is a stdout object; the other forms are
/// plain-text error lines for stderr.
pub(crate) fn error(value: &ReadFailure, format: ReadSearchFormat) -> String {
    match format {
        ReadSearchFormat::Human => human_error(value),
        ReadSearchFormat::Json => json_error(value),
        ReadSearchFormat::Compact => compact_error(value),
    }
}

pub(crate) fn human_tree(nodes: &[TreeNode]) -> String {
    let rows: Vec<_> = nodes
        .iter()
        .map(|node| {
            (
                escape_human(&node.mdpath),
                escape_human(&node.label),
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

pub(crate) fn human_error(value: &ReadFailure) -> String {
    match value {
        ReadFailure::Syntax {
            path,
            message,
            offset,
        } => format!(
            "outlint: invalid document path '{}': {} at byte {offset}\n",
            escape_human(path),
            escape_human(message)
        ),
        ReadFailure::Unresolved {
            file,
            path,
            step,
            resolved,
            nodes,
        } => format!(
            "outlint: cannot resolve {} in {}: {} not found under {}\n{}",
            escape_human(path),
            escape_human(file),
            escape_human(step),
            escape_human(resolved),
            human_tree(nodes)
        ),
        ReadFailure::Ambiguous {
            file,
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
            for candidate in candidates {
                output.push_str(&escape_human(&candidate.mdpath));
                output.push('\n');
            }
            output
        }
        ReadFailure::Other {
            file,
            path,
            message,
        } => format!(
            "outlint: cannot resolve {} in {}: {}\n",
            escape_human(path),
            escape_human(file),
            escape_human(message)
        ),
    }
}

fn json_tree(tree: &ReadTree) -> String {
    let mut output = Vec::new();
    // Read objects follow their documented wire order; do not route these
    // through `serde_json::Map`, whose default representation sorts keys.
    write_ordered_json_object(&mut output, |object| {
        object.value("version", &json!(READ_ENVELOPE_VERSION));
        object.value("file", &json!(tree.file));
        object.value("mdpath", &json!(tree.mdpath));
        object.member("nodes", |output| {
            write_ordered_json_array(output, &tree.nodes, write_node_json)
        });
    });
    finish_json_line(output)
}

fn json_content(content: &crate::read::ReadContent) -> String {
    let mut output = Vec::new();
    write_ordered_json_object(&mut output, |object| {
        object.value("version", &json!(READ_ENVELOPE_VERSION));
        object.value("file", &json!(content.file));
        object.value("mdpath", &json!(content.mdpath));
        object.value("bytes", &json!(content.bytes));
        object.value("content", &json!(content.content));
    });
    finish_json_line(output)
}

fn write_node_json(output: &mut Vec<u8>, node: &TreeNode) {
    write_ordered_json_object(output, |object| {
        object.value("mdpath", &json!(node.mdpath));
        object.value("kind", &json!(node.kind));
        match &node.data {
            TreeNodeData::Heading { level, text } => {
                object.value("level", &json!(level));
                object.value("text", &json!(text));
            }
            TreeNodeData::List { items } => {
                object.value("items", &json!(items));
            }
            TreeNodeData::Preview { preview } => {
                object.value("preview", &json!(preview));
            }
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
    write_ordered_json_object(output, |object| match value {
        ReadFailure::Syntax {
            path,
            message,
            offset,
        } => {
            object.value("kind", &json!("syntax"));
            object.value("path", &json!(path));
            object.value("message", &json!(message));
            object.value("offset", &json!(offset));
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
            object.value("resolved", &json!(resolved));
            object.member("nodes", |output| {
                write_ordered_json_array(output, nodes, write_node_json)
            });
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
            object.value("resolved", &json!(resolved));
            object.member("candidates", |output| {
                write_ordered_json_array(output, candidates, write_node_json)
            });
        }
        ReadFailure::Other { path, message, .. } => {
            object.value("kind", &json!("other"));
            object.value("path", &json!(path));
            object.value("message", &json!(message));
        }
    });
}

fn compact_tree(nodes: &[TreeNode]) -> String {
    let mut output = String::new();
    for node in nodes {
        output.push_str(&escape_compact(&node.mdpath));
        output.push('\t');
        output.push_str(&format_bytes(node.bytes));
        output.push('\t');
        output.push_str(&escape_compact(&node.label));
        output.push('\n');
    }
    output
}

fn compact_error(value: &ReadFailure) -> String {
    match value {
        ReadFailure::Syntax {
            path,
            message,
            offset,
        } => format!(
            "outlint: invalid document path '{}': {} at byte {offset}\n",
            escape_compact(path),
            escape_compact(message)
        ),
        ReadFailure::Unresolved {
            file,
            path,
            step,
            resolved,
            nodes,
        } => format!(
            "outlint: cannot resolve {} in {}: {} not found under {}\n{}",
            escape_compact(path),
            escape_compact(file),
            escape_compact(step),
            escape_compact(resolved),
            compact_tree(nodes)
        ),
        ReadFailure::Ambiguous {
            file,
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
            compact_tree(candidates)
        ),
        ReadFailure::Other {
            file,
            path,
            message,
        } => format!(
            "outlint: cannot resolve {} in {}: {}\n",
            escape_compact(path),
            escape_compact(file),
            escape_compact(message)
        ),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;

    fn nodes() -> Vec<TreeNode> {
        vec![
            TreeNode {
                mdpath: "$.setup".into(),
                kind: "section".into(),
                bytes: 3_140,
                label: "Setup".into(),
                data: TreeNodeData::Heading {
                    level: Some(2),
                    text: Some("Setup".into()),
                },
            },
            TreeNode {
                mdpath: "$.setup/p[0]".into(),
                kind: "p".into(),
                bytes: 24,
                label: "p: Install first.".into(),
                data: TreeNodeData::Preview {
                    preview: "Install first.".into(),
                },
            },
        ]
    }

    #[test]
    fn tree_formats_share_the_same_rows() {
        assert_eq!(
            human_tree(&nodes()),
            "$.setup       3.1kB  Setup\n$.setup/p[0]  24B  p: Install first.\n"
        );
        assert_eq!(
            compact_tree(&nodes()),
            "$.setup\t3.1kB\tSetup\n$.setup/p[0]\t24B\tp: Install first.\n"
        );
        let tree = ReadTree {
            file: "docs/guide.md".into(),
            mdpath: "$.setup".into(),
            nodes: nodes(),
        };
        let rendered = json_tree(&tree);
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
            file: "docs/guide.md".into(),
            path: "$.setp".into(),
            step: "setp".into(),
            resolved: "$".into(),
            nodes: nodes(),
        };
        let rendered = json_error(&failure);
        assert_eq!(
            rendered,
            "{\"version\":1,\"error\":{\"kind\":\"unresolved\",\"path\":\"$.setp\",\"step\":\"setp\",\"resolved\":\"$\",\"nodes\":[{\"mdpath\":\"$.setup\",\"kind\":\"section\",\"level\":2,\"text\":\"Setup\",\"bytes\":3140},{\"mdpath\":\"$.setup/p[0]\",\"kind\":\"p\",\"preview\":\"Install first.\",\"bytes\":24}]}}\n"
        );
        let value: Value = serde_json::from_str(&rendered).expect("renderer emits valid JSON");
        assert_eq!(value["error"]["kind"], "unresolved");
        assert_eq!(value["error"]["nodes"][0]["mdpath"], "$.setup");
        assert!(compact_error(&failure).contains("$.setup\t3.1kB\tSetup\n"));
    }

    #[test]
    fn compact_tree_and_errors_escape_control_characters_and_backslashes() {
        let controls = "tab\tcr\rlf\nesc\u{1b}slash\\";
        let nodes = vec![
            TreeNode {
                mdpath: format!("$.heading-{controls}"),
                kind: "section".into(),
                bytes: 10,
                label: format!("Heading {controls}"),
                data: TreeNodeData::Heading {
                    level: Some(2),
                    text: Some(format!("Heading {controls}")),
                },
            },
            TreeNode {
                mdpath: "$.heading/p[0]".into(),
                kind: "p".into(),
                bytes: 8,
                label: format!("p: Preview {controls}"),
                data: TreeNodeData::Preview {
                    preview: format!("Preview {controls}"),
                },
            },
        ];
        assert_eq!(
            compact_tree(&nodes),
            "$.heading-tab\\tcr\\u{d}lf\\nesc\\u{1b}slash\\\\\t10B\tHeading tab\\tcr\\u{d}lf\\nesc\\u{1b}slash\\\\\n\
             $.heading/p[0]\t8B\tp: Preview tab\\tcr\\u{d}lf\\nesc\\u{1b}slash\\\\\n"
        );

        let failure = ReadFailure::Unresolved {
            file: format!("docs/{controls}.md"),
            path: format!("$.path-{controls}"),
            step: format!("step-{controls}"),
            resolved: format!("$.resolved-{controls}"),
            nodes,
        };
        let rendered = compact_error(&failure);
        assert_eq!(rendered.lines().count(), 3, "{rendered:?}");
        assert!(rendered.starts_with(
            "outlint: cannot resolve $.path-tab\\tcr\\u{d}lf\\nesc\\u{1b}slash\\\\ in docs/tab\\tcr\\u{d}lf\\nesc\\u{1b}slash\\\\.md: step-tab\\tcr\\u{d}lf\\nesc\\u{1b}slash\\\\ not found under $.resolved-tab\\tcr\\u{d}lf\\nesc\\u{1b}slash\\\\\n"
        ));
    }

    #[test]
    fn compact_syntax_and_other_errors_escape_all_values() {
        let syntax = ReadFailure::Syntax {
            path: "$\t.bad\\path".into(),
            message: "bad\r\npath\u{1b}".into(),
            offset: 2,
        };
        assert_eq!(
            compact_error(&syntax),
            "outlint: invalid document path '$\\t.bad\\\\path': bad\\u{d}\\npath\\u{1b} at byte 2\n"
        );

        let other = ReadFailure::Other {
            file: "docs/bad\nfile.md".into(),
            path: "$.bad\tpath".into(),
            message: "failure\u{1b}\\detail".into(),
        };
        assert_eq!(
            compact_error(&other),
            "outlint: cannot resolve $.bad\\tpath in docs/bad\\nfile.md: failure\\u{1b}\\\\detail\n"
        );
        assert_eq!(
            human_error(&other),
            "outlint: cannot resolve $.bad\\tpath in docs/bad\\nfile.md: failure\\x1b\\detail\n"
        );
        assert_eq!(
            json_error(&other),
            "{\"version\":1,\"error\":{\"kind\":\"other\",\"path\":\"$.bad\\tpath\",\"message\":\"failure\\u001b\\\\detail\"}}\n"
        );
    }

    #[test]
    fn human_tree_escapes_terminal_controls() {
        let mut values = nodes();
        values[0].mdpath = "$.bad\npath".into();
        values[0].label = "Heading\tlabel\u{202e}".into();
        assert_eq!(
            human_tree(&values),
            "$.bad\\npath   3.1kB  Heading\\tlabel\\u{202e}\n$.setup/p[0]  24B  p: Install first.\n"
        );
    }
}
