//! Read output in human, JSON, and compact forms.

use serde_json::json;

use crate::{
    args::OutputFormat,
    read::{ReadFailure, ReadOutput, ReadTree, TreeNode, TreeNodeData},
};

use super::{
    escape_compact, format_bytes,
    json::{finish_json_line, write_ordered_json_array, write_ordered_json_object},
};

const VERSION: u64 = 1;

/// Renders a resolved read result in the selected format.
pub(crate) fn output(value: &ReadOutput, format: OutputFormat) -> String {
    match (value, format) {
        (ReadOutput::Content(content), OutputFormat::Human | OutputFormat::Compact) => {
            content.content.clone()
        }
        (ReadOutput::Content(content), OutputFormat::Json) => json_content(content),
        (ReadOutput::Tree(tree), OutputFormat::Human) => human_tree(&tree.nodes),
        (ReadOutput::Tree(tree), OutputFormat::Json) => json_tree(tree),
        (ReadOutput::Tree(tree), OutputFormat::Compact) => compact_tree(&tree.nodes),
    }
}

/// Renders a path failure. JSON is a stdout object; the other forms retain
/// the prototype's plain-text error line for stderr.
pub(crate) fn error(value: &ReadFailure, format: OutputFormat) -> String {
    match format {
        OutputFormat::Human => human_error(value),
        OutputFormat::Json => json_error(value),
        OutputFormat::Compact => compact_error(value),
    }
}

pub(crate) fn human_tree(nodes: &[TreeNode]) -> String {
    let width = nodes
        .iter()
        .map(|node| node.mdpath.len())
        .max()
        .unwrap_or(0);
    let mut output = String::new();
    for node in nodes {
        output.push_str(&format!(
            "{:<width$}  {}  {}\n",
            node.mdpath,
            format_bytes(node.bytes),
            node.label
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
        } => format!("outlint: invalid document path '{path}': {message} at byte {offset}\n"),
        ReadFailure::Unresolved {
            file,
            path,
            step,
            resolved,
            nodes,
        } => format!(
            "outlint: cannot resolve {path} in {file}: {step} not found under {resolved}\n{}",
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
                "outlint: ambiguous document path {path} in {file}: {} sections match '{step}'\n",
                candidates.len()
            );
            for candidate in candidates {
                output.push_str(&candidate.mdpath);
                output.push('\n');
            }
            output
        }
        ReadFailure::Other {
            file,
            path,
            message,
        } => format!("outlint: cannot resolve {path} in {file}: {message}\n"),
    }
}

fn json_tree(tree: &ReadTree) -> String {
    let mut output = Vec::new();
    // Read objects follow their documented wire order; do not route these
    // through `serde_json::Map`, whose default representation sorts keys.
    write_ordered_json_object(&mut output, |object| {
        object.value("version", &json!(VERSION));
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
        object.value("version", &json!(VERSION));
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
        object.value("version", &json!(VERSION));
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
            object.value("kind", &json!("unresolved"));
            object.value("path", &json!(path));
            object.value("step", &json!(message));
            object.value("resolved", &json!(path));
            object.member("nodes", |output| output.extend_from_slice(b"[]"));
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
        ReadFailure::Unresolved {
            file,
            path,
            step,
            resolved,
            nodes,
        } => format!(
            "outlint: cannot resolve {path} in {file}: {step} not found under {resolved}\n{}",
            compact_tree(nodes)
        ),
        ReadFailure::Ambiguous {
            file,
            path,
            step,
            candidates,
            ..
        } => format!(
            "outlint: ambiguous document path {path} in {file}: {} sections match '{step}'\n{}",
            candidates.len(),
            compact_tree(candidates)
        ),
        ReadFailure::Syntax { .. } | ReadFailure::Other { .. } => human_error(value),
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
}
