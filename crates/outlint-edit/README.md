# outlint-edit

Pure Markdown editing by
[Outlint document path](https://github.com/shangaslammi/outlint/blob/main/spec/document-path-spec.md).
Parse an edit script, supply its inputs, then compute a candidate against one
original document. A successful candidate includes the resulting source, both
parsed documents, and correspondence for surviving diagnostic subjects.
Callers handle file access and any schema gate.

The crate implements the
[experimental edit specification](https://github.com/shangaslammi/outlint/blob/main/spec/edit-spec.md).
Its API may change before 1.0.
