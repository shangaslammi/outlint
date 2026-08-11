# outlint

Lint the header structure (outline) of Markdown documents against a
declarative schema.

Status: pre-alpha. The normative specification is
[`spec/outlint-spec.md`](https://github.com/shangaslammi/outlint/blob/main/spec/outlint-spec.md).

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
design.md:1:1 [ordered] the `ordered` constraint is not satisfied; header_path=""; schema_node=constraint(scope=[],index=0); schema_location=".outlint.yml":17:12; involved_headers=["Design"@3:1, "Overview"@7:1]; references=[overview=>exact:"Overview", design=>exact:"Design"]
design.md:5:1 [unexpected-section] the section is not permitted in this closed scope; header_path="Design > Implementation Notes"; schema_node=rule(scope=[],index=1); schema_location=".outlint.yml":7:7
2 diagnostics in 1 file
```

Without `--schema`, the nearest `.outlint.yml` is discovered by walking up
from each input file.

`--format json` emits a versioned, machine-readable object on stdout. For a
`rollout.md` that has `# Widget Redesign`, `## Overview`, and `## Rollout`
but no `## Design`:

```sh
outlint check rollout.md --format json
```

```json
{"results":[{"diagnostics":[{"header_path":["Design"],"id":"missing-section","location":{"column":1,"line":1},"message":"matched 0 sections, but at least 1 are required","schema_location":{"column":7,"line":7,"path":".outlint.yml"},"schema_node":{"index":1,"kind":"rule","scope":[]}}],"kind":"document","path":"rollout.md","schema":".outlint.yml"}],"summary":{"diagnostics":1,"documents":1,"files":1,"schemas":0},"version":1}
```

Schemas can be checked on their own:

```sh
outlint schema check .outlint.yml
```

## Commands and options

```text
outlint check <FILE>...          Validate Markdown documents
outlint schema check <SCHEMA>... Validate Outlint schema files
outlint --help | --version
```

Options for both validation commands:

```text
      --format human|json     Select output format (default: human)
      --color auto|always|never
                              Control human-output color (default: auto)
  -h, --help                  Show help
```

`check` additionally accepts `-s, --schema <SCHEMA>` to use one schema for
every input. It reads standard input when the file is `-`; that requires an
explicit `--schema`. A bare `--` ends option parsing.

## Exit codes

| Code | Meaning |
| ---: | --- |
| `0` | All checked documents and schemas are valid |
| `1` | Validation completed and at least one diagnostic was emitted |
| `2` | An invocation or operational failure prevented normal validation |

## Related

- [`outlint-core`](https://crates.io/crates/outlint-core) — the pure,
  IO-free library this tool is built on.
- [`spec/cli.md`](https://github.com/shangaslammi/outlint/blob/main/spec/cli.md)
  — the normative CLI contract.

## License

Licensed under either of Apache License, Version 2.0 or MIT license at your
option (`MIT OR Apache-2.0`).
