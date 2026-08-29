# outlint

Lint the header structure (outline) of Markdown documents against a
declarative schema.

This is a 0.x release: expect breaking changes to the CLI surface, the
schema language, and the JSON output before 1.0. The normative
specification is
[`spec/outlint-spec.md`](https://github.com/shangaslammi/outlint/blob/main/spec/outlint-spec.md);
where the two disagree, the specification wins.

## Install

```sh
cargo install outlint
```

## Example

`.outlint.yml`:

```yaml
version: 1
title: "*"                  # exactly one h1, any text
sections:                   # rules for h2 headings
  - id: overview
    match: "Overview"
    required: true
  - id: design
    match: "Design"
    required: true
    strict: true            # only the child rules below are allowed
    sections:
      - match: "Alternatives"
  - id: rollout
    match: "Rollout"
    required: false
constraints:
  - ordered: [overview, design]
```

`design.md`:

```markdown
# Widget Redesign

## Design

### Implementation Notes

## Overview
```

```sh
outlint check design.md
```

```text
design.md:1:1 [ordered] sections are not in the required order
  expected order (among sections that are present):
    1. overview (exact "Overview")
    2. design (exact "Design")
  observed order:
    design.md:3:1 "Widget Redesign > Design"
    design.md:7:1 "Widget Redesign > Overview"
  constraint: .outlint.yml:17:5

design.md:5:1 [unexpected-section] the section is not permitted in this closed scope
  section: "Widget Redesign > Design > Implementation Notes"
  rule: .outlint.yml:7:5

2 diagnostics in 1 file
```

This example shows the current human presentation only. It is not a stable or
parseable output grammar; use `--format json` for scripts and integrations.

Without `--schema`, the nearest `.outlint.yml` is discovered by walking up
from each input file.

`--format json` emits a versioned, machine-readable object on stdout. For a
`rollout.md` that has `# Widget Redesign`, `## Overview`, and `## Rollout`
but no `## Design`:

```sh
outlint check rollout.md --format json
```

```json
{"results":[{"diagnostics":[{"id":"missing-section","location":{"column":1,"line":1},"message":"matched 0 sections, but at least 1 are required","schema_location":{"column":7,"line":7,"path":".outlint.yml"},"schema_node":{"index":1,"kind":"rule","scope":[]},"target":{"kind":"missing_header","matcher":"Design","parent":[]}}],"kind":"document","path":"rollout.md","schema":".outlint.yml"}],"summary":{"diagnostics":1,"documents":1,"files":1,"schemas":0},"version":2}
```

Every diagnostic carries a `target` object tagged by `kind`: `header`,
`missing_header`, `document`, or `frontmatter`. They stay distinct because
the text they carry has different provenance — a `missing_header` matcher is
schema text that may occur nowhere in the document.

Schemas can be checked on their own:

```sh
outlint schema check .outlint.yml
```

## Commands and options

Run `outlint --help` for the command summary, or the help option on either
validation command for its complete flags:

```text
Usage: outlint <command> [options]

Commands:
  check          Validate Markdown documents
  schema check   Validate Outlint schema files

Options:
  -h, --help     Show help
  -V, --version  Show version
```

`outlint check --help`:

```text
Usage: outlint check <FILE>... [options]

Validate individual Markdown files. Without --schema, the nearest .outlint.yml
is discovered separately for each file. Standard input (-) requires --schema.

Options:
  -s, --schema <SCHEMA>       Use one schema for every input
      --format human|json     Select output format (default: human)
      --color auto|always|never
                              Control human-output color (default: auto)
  -h, --help                  Show help

Exit codes: 0 valid, 1 validation diagnostics, 2 usage or operational error.
```

`outlint schema check --help`:

```text
Usage: outlint schema check <SCHEMA>... [options]

Validate schema syntax, normalization, ids, matchers, cardinalities, constraints,
and all other schema-load-time checks.

Options:
      --format human|json     Select output format (default: human)
      --color auto|always|never
                              Control human-output color (default: auto)
  -h, --help                  Show help

Exit codes: 0 valid, 1 validation diagnostics, 2 usage or operational error.
```

`outlint --version` (or `-V`) prints the package version. The version option
is top-level; the validation subcommands accept `--help` but not `--version`.

### Input and schema selection

`outlint check` accepts one or more individual Markdown file paths. Without
`--schema`, it searches upward from each file for the nearest `.outlint.yml`,
so files in one invocation may use different schemas. Other schema filenames
are never discovered automatically. `-s, --schema <SCHEMA>` selects one
schema for every input and disables discovery.

The file path `-` reads standard input and requires an explicit `--schema`.
Outlint never reads stdin merely because no file was supplied. Directories
are not traversed; expand them with your shell or `find`.

`outlint schema check` accepts one or more schema paths and performs all
load-time checks, including linked frontmatter JSON Schema loading, without
checking a Markdown document.

Both commands require UTF-8 input. A leading UTF-8 byte-order mark is
accepted. The argument `--` ends option parsing, allowing a later path to
begin with `-`.

### Output options

`--format human|json` selects reader-oriented diagnostics or one versioned
JSON object for the invocation. Human output is the default and is quiet on
success. Its wording, layout, and ordering may change between releases and
must not be parsed as machine input. JSON is the specified machine-readable
interface and is always written without ANSI escapes.

`--color auto|always|never` controls ANSI color in human output. `auto`, the
default, enables it only when standard output is a terminal; `always` forces
it and `never` disables it. This option does not add color to JSON.

JSON results follow input order. Within a JSON result, diagnostics have a
fixed total order beginning with source line, byte column, diagnostic id,
schema location, and target. See specification Section 11.4 for the complete
key. Human output may order or group diagnostics differently for readability.

Validation output goes to stdout. Usage errors and failures to read or locate
inputs go to stderr. If both diagnostics and an operational error occur,
Outlint reports both and exits with status 2.

## Exit codes

| Code | Meaning |
| ---: | --- |
| `0` | All checked documents and schemas are valid |
| `1` | Validation completed and at least one diagnostic was emitted |
| `2` | An invocation or operational failure prevented normal validation |

## Related

- [`outlint-core`](https://crates.io/crates/outlint-core) — the pure,
  IO-free library this tool is built on.
- [Outlint specification, Section 11](https://github.com/shangaslammi/outlint/blob/main/spec/outlint-spec.md#11-command-line-interface)
  — the normative behavior contract. This README describes how to use the
  reference CLI; help layout and every aspect of human formatting are
  presentation, not a portable or machine-readable contract.

## License

Licensed under either of Apache License, Version 2.0 or MIT license at your
option (`MIT OR Apache-2.0`).
