//! Pure byte-splice editing against a single pre-edit document.
use outlint_core::*;
use std::ops::Range;

mod batch;
mod diff;
mod model;
mod plan;
mod script;
mod splice;

pub use batch::{candidate, Candidate, SubjectMap};
pub use diff::diff;
pub use plan::{Op, Placement};
pub use script::{operation, parse_script, Input, Instruction, ScriptError};

/// A selector and its inputs, resolved against the original document.
#[derive(Debug)]
pub struct Edit {
    /// Document path evaluated in the original snapshot.
    pub path: String,
    /// Operation with all payloads already loaded.
    pub op: Op,
}

/// Why an edit could not produce a candidate.
#[derive(Debug)]
pub enum Error {
    /// The old literal has no match in the selected extent.
    TextNotFound,
    /// The old literal matches at the reported number of start positions.
    TextAmbiguous(usize),
    /// A malformed or unresolved selector with the standard read context.
    Path(Box<DocumentReadFailure>),
    /// The requested operation or position is unavailable for the selected node.
    Target,
    /// The payload cannot represent the requested node or content.
    Payload,
    /// The assembled document does not have the requested physical structure.
    Structure,
    /// The operation overlaps another one, identified by zero-based index.
    Conflict(usize),
    /// An internal Markdown or byte-range invariant failed.
    Range,
    /// A payload or candidate contains an unsupported bare carriage return.
    BareCr,
    /// The original input contains an unsupported bare carriage return.
    BareCrSource,
}

/// A failure with original-operation context when one operation is responsible.
#[derive(Debug)]
pub struct Refusal {
    /// Typed explanation for the failure.
    pub reason: Error,
    /// Zero-based index into the original edit list, when attributable.
    pub operation: Option<usize>,
    /// Canonical original path when the target was resolved.
    pub path: Option<String>,
}

impl From<Error> for Refusal {
    fn from(reason: Error) -> Self {
        Self {
            reason,
            operation: None,
            path: None,
        }
    }
}

fn parse(source: &str) -> Result<Document, Error> {
    parse_markdown(source, MarkdownOptions::default()).map_err(|_| Error::Range)
}

fn cut(source: &str, range: Range<usize>) -> Result<&str, Error> {
    source.get(range).ok_or(Error::Range)
}

fn convert(source: &str, ending: &str) -> Result<String, Error> {
    let lf = source.replace("\r\n", "\n");
    if lf.contains('\r') {
        return Err(Error::BareCr);
    }
    Ok(lf.replace('\n', ending))
}

fn has_bare_cr(source: &str) -> bool {
    source
        .match_indices('\r')
        .any(|(offset, _)| source.as_bytes().get(offset + 1) != Some(&b'\n'))
}

#[cfg(test)]
mod tests;
