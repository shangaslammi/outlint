# outlint

Lint the header structure (outline) of Markdown documents against a
declarative schema.

Status: pre-alpha. The normative specification lives in
[spec/outlint-spec.md](spec/outlint-spec.md).

## Usage

    outlint check README.md --schema .outlint.yml

## Layout

- `crates/outlint-core` — schema model, parser, validator (library)
- `crates/outlint-cli` — the `outlint` command-line tool
- `spec/` — the specification (normative)
- `testdata/` — conformance corpus shared by all implementations
- `npm/` — npm distribution packaging
