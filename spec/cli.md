# Outlint CLI Surface — Initial Version Proposal

Status: Historical design proposal, since implemented with deliberate
divergences. Where this file and the shipped CLI disagree, the CLI
(`outlint --help` and `crates/outlint-cli/tests/cli.rs`) is authoritative.

## 1. Goals

The initial CLI should expose the schema language with the smallest stable surface that is useful locally, in editors, and in CI.

The CLI should:

- validate Markdown documents against an Outlint schema;
- validate schema files independently;
- support explicit schema selection and project-default schema discovery;
- produce stable human-readable and machine-readable diagnostics;
- have deterministic exit codes suitable for CI;
- avoid mutating files;
- avoid introducing a separate configuration language in v1.

The CLI should not attempt to solve project-wide file discovery, schema authoring, formatting, fixing, watching, or editor integration in the initial version.

## 2. Proposed command surface

```text
outlint check <FILE>... [options]
outlint schema check <SCHEMA>... [options]

outlint --help
outlint --version
outlint <command> --help
```

The primary workflow remains the form already used by the specification:

```sh
outlint check README.md --schema docs.outlint.yml
```

A project using the default `.outlint.yml` may omit `--schema`:

```sh
outlint check README.md
```

Multiple documents may be checked in one invocation:

```sh
outlint check README.md CONTRIBUTING.md docs/api.md
```

## 3. `outlint check`

### Synopsis

```text
outlint check <FILE>... \
  [--schema <SCHEMA>] \
  [--format human|json] \
  [--color auto|always|never]
```

`FILE` is a Markdown document path or `-` for standard input.

All command-line arguments, including paths, MUST be valid UTF-8 in v1. A
non-UTF-8 argument is a usage error. This restriction is separate from file
contents, which use the UTF-8 rules in §5.

At least one input MUST be specified. The command MUST NOT implicitly read stdin when invoked without files, because doing so can accidentally block interactive invocations.

### Options

#### `-s, --schema <SCHEMA>`

Use one explicit schema for every input document.

Example:

```sh
outlint check README.md docs/guide.md --schema docs.outlint.yml
```

If omitted, schema discovery is performed independently for each file.

`--schema` and automatic discovery are mutually exclusive by construction: supplying `--schema` disables discovery.

#### `--format human|json`

Select diagnostic output format.

Default:

```text
human
```

Supported values in v1:

- `human`
- `json`

Additional formatter-specific modes such as SARIF, GitHub annotations, JUnit, or compact output should be deferred until there is demonstrated demand.

#### `--color auto|always|never`

Control ANSI color in human output.

Default:

```text
auto
```

`json` output MUST never contain ANSI escapes regardless of this option.

## 4. Schema discovery

When `--schema` is not provided, Outlint discovers a schema separately for each document.

For a document at:

```text
/project/docs/api.md
```

Outlint searches, in order:

```text
/project/docs/.outlint.yml
/project/.outlint.yml
/.outlint.yml
```

The nearest existing `.outlint.yml` wins.

Only the project-default filename `.outlint.yml` participates in implicit discovery. Files named `*.outlint.yml` are explicit schemas and MUST be selected with `--schema`.

If no schema is found, the document cannot be validated and the invocation fails with an operational error.

This permits one invocation to validate files belonging to different nested projects:

```sh
outlint check project-a/README.md project-b/README.md
```

Each file may resolve to a different `.outlint.yml`.

### stdin

For:

```sh
cat README.md | outlint check -
```

schema discovery is impossible because stdin has no filesystem location.

Therefore stdin input MUST require an explicit schema in v1:

```sh
cat README.md | outlint check - --schema .outlint.yml
```

A future `--stdin-filename` option can be added if editor integrations need discovery and path-aware diagnostics for stdin content.

## 5. File input rules

The initial version should deliberately accept individual files rather than introduce project traversal semantics.

Accepted:

```sh
outlint check README.md docs/a.md docs/b.md
outlint check docs/*.md
find docs -name '*.md' -print0 | xargs -0 outlint check
```

Not accepted in v1:

```sh
outlint check docs/
```

Passing a directory SHOULD produce a clear operational error suggesting explicit files or shell/tool-based expansion.

This avoids prematurely specifying:

- recursive traversal rules;
- extension filtering;
- hidden-directory behavior;
- ignore-file syntax;
- `.gitignore` integration;
- symlink traversal;
- duplicate file handling across overlapping directory arguments.

Those can be added later as a separate project/discovery feature without changing the validation contract.

### Encoding

Input Markdown and schema files SHOULD be interpreted as UTF-8, with an optional UTF-8 BOM accepted.

Invalid input encoding is an operational error, not an Outlint document diagnostic.

## 6. Validation behavior

For each document, `outlint check` performs the schema-defined validation pipeline:

1. resolve or load the schema;
2. validate the schema and resolve constraint references;
3. load any referenced frontmatter JSON Schema;
4. parse Markdown into the section tree;
5. validate frontmatter;
6. validate title and structural header levels;
7. validate section rules and cardinality;
8. evaluate constraints;
9. emit diagnostics.

Relative paths in `frontmatter.schema` are resolved relative to the Outlint schema file, not the current working directory.

### Invalid schema behavior

If a schema is invalid, documents depending on that schema MUST NOT be validated against a partially loaded schema.

With an explicit `--schema`, an invalid schema prevents validation of all input documents.

With automatic discovery, an invalid discovered schema prevents validation only for documents resolving to that schema. The CLI MAY continue validating documents associated with other valid schemas so that one invocation can report all independent failures.

## 7. `outlint schema check`

### Synopsis

```text
outlint schema check <SCHEMA>... \
  [--format human|json] \
  [--color auto|always|never]
```

This command validates schema files without requiring Markdown documents.

Examples:

```sh
outlint schema check .outlint.yml
outlint schema check docs.outlint.yml api.outlint.yml
outlint schema check .outlint.yml --format json
```

It MUST perform all schema-load-time checks defined by the specification, including at least:

- version validation;
- matcher validation;
- repeat/cardinality validation;
- auto-ID assignment;
- per-scope ID uniqueness;
- reserved ID checks;
- constraint reference resolution;
- ordered-scope validation;
- frontmatter configuration validation;
- loading and compiling `frontmatter.schema`.

This command is intended for:

- CI validation of schema changes;
- editor integrations;
- pre-commit hooks;
- debugging schema-load errors separately from document violations.

No `schema init`, `schema format`, or `schema migrate` commands are proposed for the initial version.

## 8. Human-readable diagnostics

Human output should be line-oriented and stable enough to read in terminals and CI logs.

Recommended shape:

```text
README.md:14:1 [missing-section] Overview: missing required section "Goals"
README.md:28:1 [unexpected-section] API Reference > Examples: unexpected section "Examples"
README.md:41:1 [requires] Deployment: "deployment" requires "deployment.rollback-plan"
```

Schema diagnostics use the schema path:

```text
.outlint.yml:37:5 [duplicate-id] duplicate rule id "overview"
.outlint.yml:52:7 [unresolved-ref] unresolved ref "rollback-plan"
```

The exact prose may evolve, but these fields should remain identifiable:

```text
<source>:<line>:<column> [<diagnostic-id>] <message>
```

Applicable structured context follows the message as semicolon-delimited
fields. Their stable forms are:

```text
; header_path="Overview > Goals"
; schema_node=rule(scope=[0],index=1)
; schema_location=".outlint.yml":22:5
; involved_headers=["Overview > Goals"@14:1]
; references=[goals=>exact:"Goals", $.api=>glob:"API *", fm.status=true]
```

An empty header path is `header_path=""`. Schema nodes without indices use
their JSON `kind` spelling. Rule-reference matchers use `exact:`, `glob:`, or
`regex:` followed by a quoted value, or `any`; schema-root references retain
their `$.` prefix. Frontmatter string equality values are quoted. These fields
are omitted when the core diagnostic has no corresponding data.

The diagnostic ID is the stable programmatic identity. Consumers MUST NOT need to parse message prose.

### Summary

Human output SHOULD end with a compact summary when diagnostics exist:

```text
3 diagnostics in 2 files
```

A successful human-mode invocation SHOULD be quiet by default.

This makes:

```sh
outlint check README.md
```

pleasant in scripts and CI while still making failures visible.

## 9. JSON output

`--format json` should emit one JSON document for the entire invocation.

Proposed top-level structure:

```json
{
  "version": 1,
  "results": [
    {
      "kind": "document",
      "path": "README.md",
      "schema": ".outlint.yml",
      "diagnostics": [
        {
          "id": "missing-section",
          "message": "missing required section \"Goals\"",
          "location": {
            "line": 14,
            "column": 1
          },
          "header_path": [
            "Overview"
          ],
          "schema_location": {
            "path": ".outlint.yml",
            "line": 22,
            "column": 5
          }
        }
      ]
    }
  ],
  "summary": {
    "files": 1,
    "documents": 1,
    "schemas": 0,
    "diagnostics": 1
  }
}
```

### JSON stability rules

The initial CLI should treat the following as part of the compatibility contract:

- top-level `version`;
- `results`;
- document `path`;
- resolved `schema`;
- `diagnostics`;
- diagnostic `id`;
- diagnostic `message`;
- diagnostic source `location`;
- `header_path` when applicable;
- `schema_location` when available;
- summary counts.

Additional fields MAY be added compatibly.

Fields that do not apply to a diagnostic SHOULD be omitted rather than emitted as ambiguous sentinel values.

Line and byte-column values are one-based unsigned 64-bit integers. This
includes document and schema locations and `frontmatter_range` endpoints.

Every result has a `kind` field. A successfully loaded document produces a
`"document"` result whose `path` is the document argument and whose `schema`
is the explicit or discovered schema path. `schema check` produces a
`"schema"` result with `path` and `schema` both naming that schema. During
`check`, one invalid schema shared by multiple documents produces one
`"schema"` result, placed where the first dependent document would have
appeared; dependent document results are omitted because validation did not
take place. A schema is loaded and its errors are emitted only once per
resolved path in one invocation.

Document diagnostics additionally expose the normalized data supplied by the
core validator when it applies:

```json
{
  "schema_node": { "kind": "rule", "scope": [0], "index": 1 },
  "involved_headers": [
    {
      "header_path": ["Overview", "Goals"],
      "location": { "line": 14, "column": 1 }
    }
  ],
  "references": [
    {
      "kind": "rule",
      "anchor": "current_scope",
      "path": ["goals"],
      "matcher": { "kind": "exact", "value": "Goals" }
    }
  ]
}
```

`schema_node.kind` is `title`, `frontmatter`,
`frontmatter_schema_declaration`, `frontmatter_schema_document`, `rule`, or
`constraint`. Rule and constraint nodes include their zero-based `scope` rule
indices and `index`. A rule reference's anchor is `current_scope` or
`schema_root`. Matchers have kind `exact`, `glob`, `regex`, or `any`; the
first three include `value`. Frontmatter references have kind `frontmatter`,
a string `path` array, and, for equality refs, an `equals` object with `type`
(`null`, `boolean`, `integer`, `float`, or `string`) and a typed `value`.
Integer and float values are their canonical strings so arbitrary precision is
preserved; null, boolean, and string values use their corresponding JSON types.

`summary.files` counts result objects: documents actually validated plus
independently reported invalid schemas. `summary.documents` and
`summary.schemas` give those counts separately. `summary.diagnostics` counts
all emitted validation diagnostics. Operationally unreadable inputs do not
produce result objects.

Frontmatter diagnostics may additionally expose:

```json
{
  "frontmatter_range": {
    "start_line": 1,
    "end_line": 6
  },
  "json_pointer": "/status"
}
```

as applicable.

`frontmatter_range` is present for diagnostics about an existing delimited
block (`forbidden-frontmatter`, `invalid-frontmatter`, and
`frontmatter-schema`) and absent for `missing-frontmatter`. `json_pointer` is
present only for `frontmatter-schema`; the empty string denotes the mapping
root, as defined by JSON Pointer.

## 10. Output streams

The CLI should keep validation output separate from operational failures.

### stdout

Used for requested validation output:

- human diagnostics;
- JSON output;
- validation summaries.

### stderr

Used for failures that prevent normal validation/output:

- invalid command-line usage;
- unreadable input files;
- no schema found;
- unsupported encoding;
- internal/runtime errors.

Schema diagnostics produced by `outlint schema check` are validation output and therefore go to stdout.

An invalid schema encountered during `outlint check` should likewise be rendered through the selected diagnostic formatter when it can be represented as a schema diagnostic.

## 11. Exit codes

Use a minimal, stable three-state contract:

| Code | Meaning |
| ---: | --- |
| `0` | All checked documents/schemas are valid |
| `1` | Validation completed and at least one Outlint document or schema diagnostic was emitted |
| `2` | Invocation or operational failure prevented normal validation |

Examples returning `1`:

- `missing-section`;
- `unexpected-section`;
- `frontmatter-schema`;
- `duplicate-id`;
- `unresolved-ref`;
- another schema-defined diagnostic.

Examples returning `2`:

- invalid CLI arguments;
- unreadable file;
- input path is a directory in v1;
- no schema found;
- invalid text encoding;
- output serialization failure;
- unexpected internal error.

When an invocation contains both validation diagnostics and an operational failure, exit code `2` wins.

Document paths and contents are preflighted independently of schema validity.
Thus an invalid explicit schema can still be reported together with unreadable,
directory, or invalid-UTF-8 document errors; the exit code is `2`, and no
dependent document is partially validated.

This keeps CI usage simple:

```sh
outlint check README.md
case $? in
  0) echo "valid" ;;
  1) echo "Outlint violations" ;;
  2) echo "Outlint could not complete" ;;
esac
```

## 12. Ordering and determinism

For identical inputs, schema files, and CLI options, diagnostic ordering MUST be deterministic.

Recommended ordering:

1. input argument order;
2. source line;
3. source column;
4. diagnostic ID;
5. schema location as final tie-breaker.

Schema-load diagnostics SHOULD be emitted before document diagnostics that depend on that schema.

JSON result objects SHOULD preserve input argument order.

When schema grouping replaces dependent documents with one schema result, that
result occupies the position of its first dependent input. Later dependents
produce no duplicate schema result. Schema diagnostics are ordered by source
line, column, id, and related schema location.

Deterministic ordering matters for:

- snapshot tests;
- CI log diffs;
- editor integrations;
- reproducible tooling.

## 13. Suppressions

The CLI should honor the specification's inline and file-wide suppression comments automatically.

No CLI suppression flags are proposed for v1.

In particular, defer options such as:

```text
--disable
--ignore-diagnostic
--no-inline-suppressions
--baseline
```

until there is a concrete workflow requiring them.

Schema-load errors are not document diagnostics and cannot be suppressed by comments in a Markdown document.

## 14. Color and non-interactive behavior

For `--color auto`:

- enable color when human output is attached to an interactive terminal;
- disable color for redirected output;
- never colorize JSON.

The CLI MUST NOT prompt interactively.

Missing files, missing schemas, and ambiguous invocation state should fail immediately with a useful error rather than asking questions.

Human output escapes untrusted fields so that every diagnostic occupies one
physical line. Backslash, double quote, newline, carriage return, tab, ESC, and
other ASCII control characters are rendered respectively as `\\`, `\"`, `\n`,
`\r`, `\t`, `\x1b`, and `\u{hex}`. This applies to source paths, messages, header paths,
schema locations, involved headers, and reference/matcher displays. ANSI escape
sequences may only be emitted by the formatter when color is enabled; with
`--color never`, untrusted text cannot introduce ANSI bytes.

This keeps behavior safe for CI and subprocess integrations.

## 15. No implicit mutation

The initial CLI performs validation only.

It MUST NOT:

- rewrite Markdown;
- insert missing sections;
- normalize headers in the source file;
- rewrite schema files;
- generate suppressions;
- modify frontmatter.

Setext normalization described by the schema specification is an internal parsing operation only and MUST NOT modify the input document.

## 16. Network behavior

The initial CLI SHOULD perform no implicit network access.

Local schema files and local frontmatter JSON Schema dependencies should be sufficient for v1.

If JSON Schema `$ref` resolution later supports remote URIs, that should be introduced with an explicit security and caching policy rather than silently added to the initial CLI.

## 17. Help surface

Top-level help should remain compact:

```text
Usage: outlint <command> [options]

Commands:
  check          Validate Markdown documents
  schema check   Validate Outlint schema files

Options:
  -h, --help     Show help
  -V, --version  Show version
```

`outlint check --help` should document:

- accepted file inputs;
- schema discovery;
- stdin requirements;
- exit codes;
- formatter choices.

`outlint schema check --help` should document which schema-load-time failures it detects.

`--` ends option parsing for both validation commands. Consequently a later
`--help` or `-h` is a path, not a help request.

## 18. Version output

`outlint --version` SHOULD emit a single machine-friendly line:

```text
outlint 0.1.0
```

The package version and supported schema language version are separate concepts.

If useful, verbose version metadata can be added later rather than complicating the initial contract.

## 19. Examples

### Default project schema

```text
project/
├── .outlint.yml
├── README.md
└── CONTRIBUTING.md
```

```sh
outlint check README.md CONTRIBUTING.md
```

### Explicit schema

```sh
outlint check README.md --schema docs.outlint.yml
```

### Different project schemas in one invocation

```text
workspace/
├── service-a/
│   ├── .outlint.yml
│   └── README.md
└── service-b/
    ├── .outlint.yml
    └── README.md
```

```sh
outlint check service-a/README.md service-b/README.md
```

Each document uses its nearest `.outlint.yml`.

### stdin

```sh
git show HEAD:README.md | outlint check - --schema .outlint.yml
```

### Machine-readable CI output

```sh
outlint check README.md docs/api.md --format json > outlint.json
```

### Validate only the schema

```sh
outlint schema check .outlint.yml
```

## 20. Explicitly deferred from the initial version

The following should not be part of the first stable CLI surface:

```text
outlint init
outlint fix
outlint format
outlint watch
outlint explain
outlint migrate
outlint generate
outlint check <directory>
--recursive
--ignore
--exclude
--config
--stdin-filename
--format sarif
--format github
--format junit
```

These are all additive features that can be introduced later without destabilizing the core validation interface.

Deferring them also avoids creating a second configuration model before real usage reveals which project-level concerns belong in the schema, CLI flags, or a future tool configuration file.

## 21. Proposed v1 compatibility boundary

The initial stable CLI contract should consist of:

```text
outlint check <FILE>...
outlint schema check <SCHEMA>...
--schema
--format human|json
--color auto|always|never
--help
--version
```

plus:

- `.outlint.yml` upward discovery;
- explicit stdin via `-`;
- the three exit codes `0`, `1`, and `2`;
- deterministic diagnostic ordering;
- stable diagnostic IDs;
- versioned JSON output.

Everything else should remain implementation detail or be deferred until a concrete use case justifies expanding the public surface.

## 22. Rationale

This surface makes the initial implementation useful without committing Outlint to project-management behavior unrelated to its core schema language.

The separation is intentional:

- **the schema specification** defines what a valid document and schema mean;
- **`outlint check`** applies that definition to Markdown;
- **`outlint schema check`** validates the validator input itself;
- **shells, build systems, and CI** select which files to run against.

That gives v1 a small compatibility burden while preserving straightforward paths to later additions such as recursive project checking, editor protocols, SARIF, schema initialization, or autofix features.
