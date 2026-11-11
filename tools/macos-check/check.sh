#!/usr/bin/env bash
# Type-check (or clippy) the workspace for Apple Silicon macOS from a Windows dev machine.
#
#   tools/macos-check/check.sh check  -p engine-app
#   tools/macos-check/check.sh clippy -p engine-renderer --all-targets -- -D warnings
#
# Needs: `rustup target add aarch64-apple-darwin` and `python -m pip install ziglang`.
# zig stands in for the C compiler a few dependencies' build scripts need (ring; blake3 is
# switched to its pure-Rust path). Nothing is linked, so no macOS SDK is required. This
# verifies that the macOS code compiles; it does not replace building and running on a Mac.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
win="$(cygpath -w "$here" 2>/dev/null || echo "$here")"
sub="$1"; shift
export CARGO_FEATURE_PURE=1
export CC_aarch64_apple_darwin="$win\zigcc.cmd"
export AR_aarch64_apple_darwin="$win\zigar.cmd"
exec cargo "$sub" --target aarch64-apple-darwin "$@"
