#!/usr/bin/env bash
set -euo pipefail

# cargo-dist calls this once per release target, on that platform's runner.
release_target="${CARGO_DIST_TARGET:?cargo-dist must select a release target}"
package_dir="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
repo_dir="$(CDPATH= cd -- "$package_dir/../.." && pwd)"
build_dir="$repo_dir/target/dist-full"
executable=outlint

# Generic dist builds can run before its native Cargo build installs targets.
rustup target add "$release_target"

case "$release_target" in
  *-windows-msvc)
    executable=outlint.exe
    # Match cargo-dist's statically linked Windows CRT for the core edition.
    export RUSTFLAGS="${RUSTFLAGS:-} -Ctarget-feature=+crt-static"
    ;;
  x86_64-unknown-linux-musl)
    # The release runner installs musl-tools for the native Cargo app too.
    export CC_x86_64_unknown_linux_musl=musl-gcc
    export CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc
    ;;
esac

# All features includes future optional commands without another packaging edit.
cargo build --manifest-path "$repo_dir/Cargo.toml" --package outlint \
  --profile dist --locked --no-default-features --all-features \
  --target "$release_target" --target-dir "$build_dir"

mkdir -p "$package_dir/bin"
cp "$build_dir/$release_target/dist/$executable" "$package_dir/bin/$executable"
