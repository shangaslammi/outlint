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

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in this work by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.
