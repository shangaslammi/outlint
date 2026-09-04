# JSONPath Compliance Test Suite — provenance

These files are the official JSONPath (RFC 9535) Compliance Test Suite,
vendored verbatim. They are test fixtures only; no Outlint code links against
them.

## What this suite is, and is not

It is **secondary evidence for the Outlint core profile**. Specification §4.6
defines a guaranteed core of the RFC 9535 grammar — root plus child name,
index, and wildcard segments — and states that "only this core is covered by
Outlint's self-verification corpus; vendor-tier query outcomes are not an
Outlint conformance or release gate."

The primary release gate is therefore Outlint's own authored corpus,
`outlint-core.json`, run by `tests/jsonpath_core.rs`. This suite corroborates
it from an independent source.

All 703 cases are classified; **only the generated-manifest core cases are
evaluated.** For the other cases the JSONPath provider is never invoked at all.
This suite is not a full RFC 9535 compliance gate, and Outlint makes no claim
of conformance across all 703 cases.

Membership is decided by `tests/support/jsonpath_core_recognizer.rs`, which
reads raw query text and never consults the provider, a case name, or a tag.
Its decision for every case is recorded in `core-manifest.json`, which
`tests/jsonpath_cts_core.rs` recomputes and compares exactly.

## Source

- Repository: <https://github.com/jsonpath-standard/jsonpath-compliance-test-suite>
- Commit: `7be7c1fc28057c91e8eefaf197060fba7ed43acd`
- Upstream commit date: 2026-05-21
- Retrieved: 2026-09-04
- Retrieved from:
  `https://raw.githubusercontent.com/jsonpath-standard/jsonpath-compliance-test-suite/7be7c1fc28057c91e8eefaf197060fba7ed43acd/<path>`

## Files copied

Each file was copied verbatim from the repository root of the pinned commit. No
file was edited, reformatted, or regenerated locally; in particular `cts.json`
is the generated suite as published upstream and was **not** produced by running
the upstream generator here.

| Upstream path     | Local path        | SHA-256                                                            |
| ----------------- | ----------------- | ------------------------------------------------------------------ |
| `cts.json`        | `cts.json`        | `a85db53fba1f675be48b534baec5a754dc685ad08c550d8927f609c7708f365a` |
| `cts.schema.json` | `cts.schema.json` | `4c6d539f94952a293c8be3cdc14dba31bb8d64ae43e08f0d19db86d54eb1c552` |
| `LICENSE`         | `LICENSE`         | `0a76d5e15eeff92346a8783de64d5164c4d527a163f8599733e4e0ab941b59c0` |
| `NOTICE`          | `NOTICE`          | `e34cdb81d4ace9bfc808641845e63aa63fbc17ec4433256e6b887cb1eeb5fb70` |

Verify with:

```sh
cd crates/outlint-core/tests/fixtures/jsonpath
sha256sum -c <<'EOF'
a85db53fba1f675be48b534baec5a754dc685ad08c550d8927f609c7708f365a  cts.json
4c6d539f94952a293c8be3cdc14dba31bb8d64ae43e08f0d19db86d54eb1c552  cts.schema.json
0a76d5e15eeff92346a8783de64d5164c4d527a163f8599733e4e0ab941b59c0  LICENSE
e34cdb81d4ace9bfc808641845e63aa63fbc17ec4433256e6b887cb1eeb5fb70  NOTICE
EOF
```

## Suite breakdown at this pin

**703 cases total**, comprising:

- 447 deterministic cases (`result` plus `result_paths`);
- 9 nondeterministic cases (`results` plus `results_paths`);
- 247 invalid-selector cases.

Of those 703, the generated manifest recognizes **83** as Outlint core: 81
deterministic and 2 nondeterministic. **620** are classified non-core and are
never evaluated. **0** invalid-selector cases are recognized as valid core
syntax — a nonzero count there would mean the recognizer accepts a query the
RFC rejects, and is an escalation.

> An earlier plan stated 723 cases for this revision. The pinned commit
> actually contains 703. The pin was kept and the count corrected to the
> observed value rather than moving to a revision that matched the number.

## Reviewed exclusions

`core-manifest.json` carries an `exclusions` array for cases that are
recognized as core but deliberately not evaluated. **It is empty at this pin
and must stay empty** unless a maintainer records an entry.

An entry requires maintainer approval, the exact case name, and a written
normative reason. A stale exclusion — one matching no recognized case — fails
the suite, as does a duplicate. Every recognized core case must appear exactly
once in either `included` or `exclusions`.

**A failing core case is an escalation.** It must not be resolved by adding an
exclusion, by reclassifying the construct as vendor tier, or by narrowing the
recognizer.

## Nondeterminism

The suite expresses queries whose result order RFC 9535 does not fix — object
member iteration, for instance — through `results` and `results_paths`, each
listing complete acceptable outcomes. The runner accepts exactly one complete
alternative and never pairs the values of one alternative with the paths of
another.

Outlint compares results as unordered node sets keyed by path identity, because
§4.6 makes the order at the `fm[...]` boundary unobservable. Nothing here
asserts RFC nodelist order.

## Path rendering

Expected paths are compared against Outlint's own renderer in
`tests/support/jsonpath_path.rs`, never against the provider's `Display`. §4.6
requires this: "Outlint owns path rendering at this boundary [...] A JSONPath
provider's rendered path is not authoritative."

## License

The compliance test suite is licensed BSD-2-Clause. `LICENSE` and `NOTICE` are
preserved here verbatim beside the fixtures.

`cargo-deny` inspects Cargo packages, not repository fixture files, so
BSD-2-Clause is deliberately **not** added to the Cargo license allowlist in
`deny.toml`. The license is honoured by preserving these files instead.

## Why in-tree rather than a submodule

The fixtures are committed directly so an ordinary CI checkout always contains
the suite and the secondary gate cannot be skipped by a missing or unfetched
submodule. The tests load `cts.json` with `include_str!`, so a missing fixture
is a compile error rather than a silently empty run.

## Updating

See `UPDATING.md`. Never edit these files in place.
