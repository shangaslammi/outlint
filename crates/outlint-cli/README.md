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
sections:                   # rules for h2 headings, in document order
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
design.md:1:1 [ordered] sections are out of the declared order: `Overview` must precede `Design`
  observed order:
    design.md:3:1 "Widget Redesign > Design"
    design.md:7:1 "Widget Redesign > Overview"
  schema: .outlint.yml:2:8

design.md:5:1 [unexpected-section] the section is not permitted in this closed scope
  section: "Widget Redesign > Design > Implementation Notes"
  rule: .outlint.yml:7:5

2 diagnostics in 1 file
```

This example shows the current human presentation only. It is not a stable or
parseable output grammar; use `--format json` for scripts and integrations.

Without `--schema`, each input file discovers its schema by walking up:
`<stem>.outlint.yml` (its file name, extension removed) is preferred over
`.outlint.yml` in each directory.

`--format json` emits a versioned, machine-readable object on stdout. For a
`rollout.md` that has `# Widget Redesign`, `## Overview`, and `## Rollout`
but no `## Design`:

```sh
outlint check rollout.md --format json
```

```json
{"results":[{"diagnostics":[{"id":"missing-section","location":{"column":1,"line":1},"message":"matched 0 sections, but at least 1 are required","schema_location":{"column":5,"line":7,"path":".outlint.yml"},"schema_node":{"index":1,"kind":"rule","scope":[]},"target":{"kind":"missing_header","matcher":"Design","parent":[]}}],"kind":"document","path":"rollout.md","schema":".outlint.yml"}],"summary":{"diagnostics":1,"documents":1,"files":1,"schemas":0},"version":3}
```

The envelope is exactly version `3`. There is no version 2 and no
compatibility mode: consumers must read `version` and reject a value they do
not support instead of assuming the older reference shape.

Every diagnostic carries a `target` object tagged by `kind`: `header`,
`missing_header`, `document`, or `frontmatter`. They stay distinct because
the text they carry has different provenance — a `missing_header` matcher is
schema text that may occur nowhere in the document.

Where a diagnostic names the schema locators it evaluated, it also carries
`references`. Each entry is tagged by `kind` and keeps the locator's
original spelling in `locator`:

- `rule` — `anchor` (`current_scope` or `schema_root`), the `path` of
  declared names, an optional `positions` array aligned with `path` giving
  each step's `[i]` or null, and the rule's `matcher`. Position values are
  JSON integers of arbitrary size and must not be assumed to fit 64 bits.
- `frontmatter_query` — the `query` inside `fm[...]` without the wrapper,
  plus an `equals` object of `type` and `value` when the locator spelled an
  equality literal.
- `frontmatter_capture` — the capture's `name` and its declared `type`.

Typed Values diagnostics are located in the schema through `schema_node`,
which adds the `capture`, `frontmatter_capture`, and `order_entry` kinds to
the existing ones. Section 11.3 of the specification is the normative shape;
every member listed there appears only when the corresponding semantic data
exists.

In human output, an `invalid-value` names the responsible capture or
frontmatter query and the type that was expected, and an `order-violation`
names the capture, the declared direction, and both values of the offending
adjacent pair. As with all human output, that wording is presentation and
may change between releases; parse `--format json` instead.

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

Validate individual Markdown files. Without --schema, each file discovers its
schema separately: the nearest <stem>.outlint.yml (file name, extension
removed) or .outlint.yml, specific name first in each ancestor directory.
Standard input (-) requires --schema.

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
`--schema`, it searches upward from each file, checking each ancestor
directory first for `<stem>.outlint.yml` — the file's name with its final
extension removed, so `CHANGELOG.md` discovers `CHANGELOG.outlint.yml` —
and then for `.outlint.yml`; the nearest match wins, and files in one
invocation may use different schemas. Other schema filenames are never
discovered automatically. `-s, --schema <SCHEMA>` selects one schema for
every input and disables discovery.

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
Outlint reports both and exits with status 2. A frontmatter `fm[...]` query
whose result cannot be evaluated in full is such an operational error, not a
diagnostic: the document has no verdict, and Outlint reports that rather than
a partial diagnostic set that would read like a cleaner document than it is.

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
