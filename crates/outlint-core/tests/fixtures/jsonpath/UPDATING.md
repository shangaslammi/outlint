# Updating the vendored JSONPath compliance suite

The suite is pinned to one upstream commit. Moving it is a deliberate act, not
a routine refresh, and it is never done to make a failing test pass.

Read `PROVENANCE.md` first for what this suite is: secondary evidence for the
§4.6 Outlint core profile, not a full RFC 9535 conformance gate. The primary
gate is `outlint-core.json`, which is authored from the specification and is
**not** updated by this procedure.

## Procedure

### 1. Deliberately select a new full commit

Choose a specific upstream commit and record its full 40-character SHA. Never
track a branch, a tag, or `main`. Note why the move is being made.

### 2. Copy all four upstream files verbatim

From the repository root of that commit, replacing the local copies:

```sh
BASE=https://raw.githubusercontent.com/jsonpath-standard/jsonpath-compliance-test-suite/<commit>
cd crates/outlint-core/tests/fixtures/jsonpath
for file in cts.json cts.schema.json LICENSE NOTICE; do
  curl -fsSL -o "$file" "$BASE/$file"
done
```

Copy them byte for byte. Do not edit, reformat, prune, or regenerate
`cts.json` with the upstream generator. `LICENSE` and `NOTICE` move with the
suite even when unchanged.

### 3. Recalculate hashes and the total suite breakdown

```sh
sha256sum cts.json cts.schema.json LICENSE NOTICE
```

Update the table in `PROVENANCE.md`, along with the commit, the upstream commit
date, and the retrieval date. Recount the breakdown — total cases, deterministic,
nondeterministic, invalid-selector — and update it there too. Also update
`SUITE_COMMIT` in `tests/support/jsonpath_core_manifest.rs`.

### 4. Regenerate the manifest

```sh
cargo run -q -p outlint-core \
  --example generate_jsonpath_core_manifest --locked \
  > crates/outlint-core/tests/fixtures/jsonpath/core-manifest.json
```

The generator only prints; redirecting its output is what updates the file. It
uses the same recognizer as the test, so the two cannot disagree.

### 5. Review every added, removed, or reclassified case

Read the manifest diff case by case. The test reports each of these, and each
needs an answer before the update lands:

- **Newly recognized as core.** Does the case genuinely fall inside §4.6, and
  does Outlint actually guarantee its outcome?
- **No longer recognized as core.** Did upstream change the selector, or did
  the recognizer regress? A case silently leaving the core is the easiest way
  to lose coverage.
- **Changed name, selector, or ordinal.** A changed ordinal alone is
  reordering; a changed selector under an unchanged name deserves scrutiny.
- **Count changes.** Reconcile every one against the case-level diff.
- **Stale or duplicate exclusions.** An exclusion matching no recognized case
  must be removed, with its removal reviewed.

`invalid_recognized_as_core` must stay `0`. Anything else means the recognizer
accepts a query RFC 9535 rejects — a recognizer bug, and an escalation.

### 6. Run the primary core corpus first

```sh
cargo test -p outlint-core --test jsonpath_core --locked
```

This is the release gate. It must pass before the secondary suite is worth
looking at, and it is unaffected by the vendored suite.

### 7. Run the filtered secondary corpus

```sh
cargo test -p outlint-core --test jsonpath_cts_core --locked
cargo +1.86.0 test -p outlint-core --test jsonpath_cts_core --locked
```

Then confirm the checked-in manifest is exactly what the generator emits:

```sh
tmp_manifest="$(mktemp)"
cargo run -q -p outlint-core \
  --example generate_jsonpath_core_manifest --locked > "$tmp_manifest"
diff -u crates/outlint-core/tests/fixtures/jsonpath/core-manifest.json "$tmp_manifest"
rm "$tmp_manifest"
```

Finish with the workspace gate: `cargo fmt --all -- --check`, `cargo clippy
--workspace --all-targets --locked -- -D warnings`, `cargo test --workspace
--locked`, `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
--locked`, `cargo +1.86.0 test --workspace --locked`, and `cargo deny check`.

### 8. Escalate any core failure

If a case inside the core fails, **stop and escalate to a maintainer.** Do not:

- move the construct to the vendor tier;
- narrow the recognizer so the case stops being core;
- add an exclusion;
- adjust the expected result to match what the implementation produced;
- skip, ignore, or filter the case.

A failing core case means Outlint's guarantee and its provider disagree. That
is a finding about the guarantee or the provider, and the answer is a decision,
not a suppression. Adding an exclusion requires maintainer approval, the exact
case name, and a written normative reason recorded in `core-manifest.json`.

## Changing the core profile itself

Widening or narrowing the §4.6 core is a specification change first. Amend the
specification, then the recognizer, then `outlint-core.json`, then regenerate
the manifest — in that order. The recognizer is test infrastructure and must
never reject a query the provider would otherwise accept: §4.6 requires that a
non-core query "MUST NOT be rejected merely for falling outside the guaranteed
core".
