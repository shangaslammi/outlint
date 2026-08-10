# Outlint conformance fixtures

Each child directory is one reusable, implementation-independent fixture.
It contains:

- `schema.outlint.yml`, the schema to load;
- one or more Markdown documents (`*.md`) to validate;
- `expected.json`, an object mapping every Markdown filename in the directory
  to its ordered list of expected diagnostics; and
- optional JSON Schema resources used by `frontmatter.schema`. These MAY be
  nested in subdirectories; the declared root and its file-relative `$ref`
  graph determine which resources are loaded.

Each expected diagnostic has the portable shape:

```json
{ "id": "missing-section", "path": "Overview > Goals" }
```

`id` is the public diagnostic id from specification §6. `path` is the
case-preserving visible header path joined with ` > `; document-root
diagnostics use the empty string. The expected object must name every and only
the Markdown files in its directory. Diagnostic array order is observable and
must match validator output.

The Rust conformance runner discovers all child directories automatically and
loads linked JSON Schema resources through the same filesystem traversal as the
CLI. Other implementations should apply the same fixture contract without
depending on Rust-specific APIs, source locations, or a flat resource layout.
