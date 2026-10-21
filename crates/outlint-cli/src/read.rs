//! The `read` subcommand (behind the `read` feature): prints the node of a
//! Markdown file addressed by a document path, or lists the structure under
//! it. Computation lives in core; [`execute_read`] is the IO shell.

use std::path::Path;

use outlint_core::{
    document_content, document_tree, parse_markdown, DocumentContent, DocumentTree, MarkdownOptions,
};

use crate::{
    args::{ReadOptions, ReadSearchFormat},
    render::{self, escape_compact, escape_human},
    schema_loading::read_utf8_file,
    write_stderr, write_stdout,
};

/// Exit 0 when the node was printed, 1 on a path syntax or resolution error,
/// 2 on an operational error.
pub(crate) fn execute_read(options: &ReadOptions) -> u8 {
    let source = match read_utf8_file(Path::new(&options.file), "Markdown input") {
        Ok(source) => source,
        Err(message) => return operational_error(&message, options.format),
    };
    let document = match parse_markdown(&source, MarkdownOptions::default()) {
        Ok(document) => document,
        Err(error) => {
            return operational_error(
                &format!("cannot parse {}: {error}", options.file),
                options.format,
            );
        }
    };
    let path = path_spelling(options.path.as_deref().unwrap_or("$"));
    let result = if options.tree {
        document_tree(&source, &document, &path, options.depth, options.blocks)
            .map(ReadOutput::Tree)
    } else {
        document_content(&source, &document, &path, options.depth).map(ReadOutput::Content)
    };
    match result {
        Ok(output) => write_stdout(&render::read::output(
            &output,
            &options.file,
            options.format,
        )),
        Err(error) => {
            let rendered = render::read::error(&error, &options.file, options.format);
            if options.format == ReadSearchFormat::Json {
                if write_stdout(&rendered) == 2 {
                    2
                } else {
                    1
                }
            } else {
                write_stderr(&rendered);
                1
            }
        }
    }
}

fn operational_error(message: &str, format: ReadSearchFormat) -> u8 {
    let message = match format {
        ReadSearchFormat::Compact => escape_compact(message),
        ReadSearchFormat::Human => escape_human(message),
        ReadSearchFormat::Json => message.to_owned(),
    };
    write_stderr(&format!("outlint: {message}\n"));
    2
}

/// Structured successful output shared by all read renderers.
pub(crate) enum ReadOutput {
    Content(DocumentContent),
    Tree(DocumentTree),
}

/// Adds the root prefix so `.a.b` and `/p[0]` work unquoted in a shell.
pub(crate) fn path_spelling(argument: &str) -> String {
    if argument.starts_with('$') {
        argument.to_owned()
    } else {
        format!("${argument}")
    }
}
