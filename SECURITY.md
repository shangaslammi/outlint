# Security Policy

## Supported versions

The current release is `0.1.0`, distributed through GitHub Releases, crates.io,
and npm.

| Version | Supported |
| --- | --- |
| `0.1.0` | Yes — fixes land on `main` and ship in the next release |
| Anything else | No |

While the project is pre-1.0 there are no maintained release branches and no
backported patches. A security fix is released as a new `0.x` version, and the
only supported upgrade path is to move to it.

## Reporting a vulnerability

**Please do not open a public issue for a security problem.**

Report it privately, by either route:

- **Email** <sami@infer.fi>. Put "outlint security" in the subject. If you
  want an encrypted channel, say so in a first message with no details and one
  can be arranged.
- **GitHub private security advisories**: use the "Report a vulnerability"
  button on the Security tab of
  <https://github.com/shangaslammi/outlint/security>.

Helpful things to include, roughly in order of usefulness:

- the schema (`*.outlint.yml`) and Markdown or JSON Schema input that trigger
  the problem, as files or inline text;
- the exact command line and the outlint version (`outlint --version`) or
  commit;
- what happens (crash, hang, unbounded memory, unexpected file access) and
  what you expected;
- whether the behavior contradicts [`spec/outlint-spec.md`](spec/outlint-spec.md),
  including its command-line contract in §11, if you know.

### What to expect

This is a single-maintainer, hobby-scale project. There is no on-call rotation
and no service-level agreement. Realistically:

- **Acknowledgement**: usually within about a week. If you have heard nothing
  after two weeks, send a follow-up — the first message was probably lost.
- **Assessment and fix**: as soon as is practical after that, prioritized by
  severity. A serious, easily triggered issue gets worked on immediately; a
  theoretical one may take considerably longer.
- **Disclosure**: coordinated. Once a fix is on `main`, the issue is described
  in [CHANGELOG.md](CHANGELOG.md) and, where a GitHub advisory was used, in
  that advisory. Reporters are credited unless they prefer otherwise.

Please give a reasonable window before public disclosure, and tell the
maintainer the date you intend to publish.

## Threat model

The native outlint command-line tool and library are offline. The CLI reads
files, writes diagnostics to stdout/stderr, and exits. It **never modifies the
documents it checks** ([`spec/outlint-spec.md`](spec/outlint-spec.md) §11.6)
and performs no implicit network access (also §11.6). JSON Schema `$ref`
resolution is file-local only: remote retrieval is refused and a remote `$ref`
is reported as the schema error `invalid-frontmatter-schema`
([`spec/outlint-spec.md`](spec/outlint-spec.md) §2.3).

The npm launcher is a distribution bootstrap rather than part of document
validation. On its first invocation it downloads the matching binary and
SHA-256 sidecar from the same-version GitHub Release, verifies the archive,
and caches the executable for later runs. Document, Outlint schema, and JSON
Schema contents do not influence that request.

The realistic exposure is that outlint runs in CI over content from a pull
request, so all three of its inputs may be attacker-controlled:

- the **Markdown documents** being checked;
- the **YAML schema**, if the schema is itself part of the reviewed tree;
- the **JSON Schema resources** reachable from `frontmatter.schema` and its
  `$ref` graph.

### In scope

- Memory-unsafe behavior, panics, or aborts on any input. The library is
  specified not to panic on malformed input: schema loading returns
  `InvalidSchema` with positioned errors, and validation returns diagnostics.
  A panic reachable from untrusted Markdown, YAML, or JSON Schema is a bug,
  and one that is remotely triggerable in CI is a security bug.
- Denial of service in the parsers: unbounded memory, stack exhaustion from
  deeply nested YAML/JSON or heading structures, quadratic or worse blowup in
  Markdown scanning, `$ref` cycles that fail to terminate, or any input that
  makes outlint hang.
- **Regex behavior.** Schema `match: "/…/"` patterns are user-supplied and are
  compiled with the Rust `regex` crate, anchored as `\A(?:body)\z`
  (`crates/outlint-core/src/matcher.rs`). That engine's linear-time guarantee
  is what makes untrusted patterns tolerable at all, and the specification
  makes the RE2 dialect normative for exactly this reason
  ([`spec/outlint-spec.md`](spec/outlint-spec.md) §2.2) — backreferences and
  lookaround are not in the dialect and are rejected as `invalid-matcher`, so
  catastrophic backtracking is not reachable. What is *not*
  currently bounded is compilation: outlint uses the crate's default size and
  DFA limits and does not impose its own, so a large or highly expansive
  pattern is a resource-consumption concern. Report a pattern that causes
  disproportionate compile time or memory.
- Path handling: a schema or `$ref` graph that reads files outside the
  intended tree in a way the specification does not sanction.
- Anything that causes outlint to write to, or delete, a file it is checking.

### Out of scope

- Wrong diagnostics, missed violations, or false positives. Those are ordinary
  correctness bugs — please open a normal issue with a `testdata/` reproduction
  (see [CONTRIBUTING.md](CONTRIBUTING.md)).
- Vulnerabilities in dependencies with no reachable path from outlint's own
  API. Report those upstream; a note here is welcome if outlint's usage does
  make them reachable.
- Resource use that is simply proportional to a very large input.
- Consequences of pointing outlint at files you did not intend to check.

## Hardening notes for CI

- Run outlint over pull-request content in a job without repository write
  credentials. Its only outputs are diagnostics and an exit code
  (`0` clean, `1` diagnostics, `2` operational failure), so nothing more is
  needed.
- Prefer an explicit `--schema` over discovery when checking untrusted
  branches, so a contributed `.outlint.yml` cannot select the rules applied to
  it.
