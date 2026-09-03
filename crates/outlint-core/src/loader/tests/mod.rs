//! Shared helpers for the loader's unit tests.

mod constraints;
mod frontmatter_schema;
mod rules;
mod yaml;

use crate::loader::load_schema;
use crate::{InvalidSchema, Schema, SchemaErrorKind, SourceRange};

fn valid(source: &str) -> Schema {
    match load_schema(source) {
        Ok(loaded) => loaded.schema,
        Err(invalid) => panic!("unexpected errors: {:#?}", invalid.errors),
    }
}

fn error_kinds(source: &str) -> Vec<SchemaErrorKind> {
    match load_schema(source) {
        Ok(loaded) => panic!("unexpected valid schema: {:#?}", loaded.schema),
        Err(invalid) => invalid.errors.iter().map(|error| error.kind).collect(),
    }
}

fn invalid(source: &str) -> InvalidSchema {
    match load_schema(source) {
        Ok(loaded) => panic!("unexpected valid schema: {:#?}", loaded.schema),
        Err(invalid) => invalid,
    }
}

fn source_slice(source: &str, range: SourceRange) -> &str {
    source
        .get(range.range.start.0..range.range.end.0)
        .unwrap_or("<invalid range>")
}
