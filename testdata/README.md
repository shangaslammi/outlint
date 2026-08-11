# Outlint conformance fixtures

Each child directory is one reusable, implementation-independent fixture.
It contains:

- `schema.outlint.yml`, the schema to load;
- one or more Markdown documents (`*.md`) to validate;
- `expected.json`, an object mapping every Markdown filename in the directory
  to the collection of diagnostics expected for that document, written as a
  JSON array; and
- optional JSON Schema resources used by `frontmatter.schema`. These MAY be
  nested in subdirectories; the declared root and its file-relative `$ref`
  graph determine which resources are loaded.

Each expected diagnostic has the portable shape:

```json
{ "id": "missing-section", "path": "Overview > Goals" }
```

`id` is the public diagnostic id from specification §6. `path` follows the
diagnostic path rule defined normatively in specification §6 ("Diagnostic
path"); note that for absence diagnostics (`missing-section`,
`too-few-sections`) this is a synthesized matcher label, not text copied
from the document. Each entry has exactly these two members. The
expected object must name every and only the Markdown files in its directory.

Diagnostic arrays compare as multisets (bags), not as sequences. A document
conforms when the entries an implementation produces for it and the entries
`expected.json` lists for it contain the same `{id, path}` values with the same
multiplicities. Array order carries no meaning and MUST NOT be asserted on:
diagnostic emission order is an internal implementation detail, and what a tool
prints is a presentation choice it is free to make. Repetition, by contrast, is
significant — the two identical entries under `frontmatter-schema/fail.md`
require exactly two such diagnostics, so producing one or three is a failure.

The normative comparison is: sort both sides by the pair (`id`, `path`), using
plain lexicographic ordering of the JSON string values and comparing `path`
only when `id` ties, then require the two sorted sequences to be equal. Any
comparison yielding the same verdict is equally valid; reporting the symmetric
difference (expected-but-missing and produced-but-unexpected entries, with
counts) makes failures far easier to diagnose.

The Rust conformance runner discovers all child directories automatically and
drives each fixture through the built `outlint` binary with `--format json`,
rebuilding each `{id, path}` entry from the reported `id` and `header_path`
members. Other implementations should apply the same fixture contract without
depending on Rust-specific APIs, source locations, or a flat resource layout.
