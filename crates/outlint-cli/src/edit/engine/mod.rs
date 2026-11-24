//! Pure byte-splice editing against a single pre-edit document.
use outlint_core::*;
use std::ops::Range;

mod batch;
mod gate;
mod model;
mod plan;

pub(super) use batch::candidate;
pub(super) use plan::{Op, Placement};

/// A selector and its inputs, resolved against the original document.
pub(super) struct Edit {
    pub path: String,
    pub op: Op,
}

#[derive(Debug)]
pub(super) enum Error {
    TextNotFound,
    TextAmbiguous(usize),
    Path(Box<DocumentReadFailure>),
    Target,
    Payload,
    Structure,
    Conflict(usize),
    Range,
    BareCr,
    BareCrSource,
}

#[derive(Debug)]
pub(super) struct Refusal {
    pub reason: Error,
    // Batch-wide failures may have no single responsible operation.
    pub operation: Option<usize>,
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
