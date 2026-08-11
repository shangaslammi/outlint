<!--
Thanks for the pull request. Please read CONTRIBUTING.md if you have not
already — the spec-first rule and the testdata/ corpus contract are the two
things that are easy to get wrong here.
-->

## What and why

<!-- What changes, and what problem it solves. Link the issue if there is one. -->

Closes #

## Specification

<!-- Pick one and delete the rest. -->

- [ ] No observable behavior change (refactor, docs, tests, tooling).
- [ ] Behavior changed and `spec/` is updated **in this pull request**.
      Sections touched:
- [ ] Behavior matches what `spec/` already says; the code was the bug.
      Section that already required it:

Diagnostic ids, schema-error ids, the JSON output shape, and exit codes are a
public contract. If any of them are added, renamed, or repurposed, the
specification change above must cover it.

## Verification

All four were run locally:

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps`

<!-- If any failed or was skipped, say which and why. -->

## Tests

- [ ] New or changed user-visible behavior has a `testdata/` fixture, and its
      `expected.json` names every and only the Markdown files in that
      directory, with diagnostics in observed order.
- [ ] Loader/matcher invariants that are not user-visible have unit tests next
      to the code.
- [ ] No existing test was weakened or deleted. (If an `expected.json` was
      genuinely wrong, explain why below, citing the specification.)

## Design

- [ ] Core logic is pure: no `fs`, `io`, `std::env`, `println!`/`eprintln!`,
      clock, or randomness in a function that computes a result. IO stays in
      the CLI shell.
- [ ] No panics reachable from user input — no `unwrap`/`expect`/indexing on
      it, and no `unsafe`. Errors are collected, not short-circuited.
- [ ] Every new public item is documented with its invariant or rationale.
- [ ] No new dependency, or the pull request justifies it.
- [ ] MSRV is still Rust 1.86.

## Housekeeping

- [ ] `CHANGELOG.md` `## [Unreleased]` updated, if this is user-visible.
- [ ] If release packaging changed, `npm/outlint/package.json` `version`
      matches `[workspace.package] version` in `Cargo.toml`.

## Notes for the reviewer

<!-- Anything you are unsure about, alternatives you rejected, or follow-up work. -->
