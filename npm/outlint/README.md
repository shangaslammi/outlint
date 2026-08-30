# outlint

Lint the header structure of Markdown documents against a declarative schema.
The schema language and command-line contract are documented in the
[outlint repository](https://github.com/shangaslammi/outlint).

## Install

```sh
npm install --global outlint
```

The package has no install-time lifecycle script. The first `outlint` command
downloads the matching pre-built binary from the same-version GitHub Release,
verifies its cargo-dist SHA-256 sidecar, and stores it in the current user's
cache. Later commands use the cached binary. The first command therefore needs
access to GitHub Releases.

Pre-built binaries are available for macOS on Arm and x64, glibc Linux on Arm
and x64, musl Linux on x64, and x64 Windows. Building from source with
`cargo install outlint` may support additional Rust targets.

This is a 0.x release: expect breaking changes before 1.0.
