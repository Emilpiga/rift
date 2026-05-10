#!/usr/bin/env bash
# Generate `dist/THIRD_PARTY.txt` listing every Rust crate the
# rift-client binary links against, along with their license
# text. Output ships next to the binary in the redistributable
# bundle so we satisfy attribution requirements (MIT, Apache-2.0,
# BSD-*, etc.) for every dependency we pull in.
#
# One-time setup on the build machine:
#   cargo install cargo-about
#
# Usage:
#   ./scripts/gen-third-party.sh           # writes dist/THIRD_PARTY.txt
#   ./scripts/gen-third-party.sh --fail    # also fails on unknown licenses
#
# Re-run any time `Cargo.lock` changes.

set -euo pipefail

cd "$(dirname "$0")/.."

if ! command -v cargo-about >/dev/null 2>&1; then
    echo "error: cargo-about not installed." >&2
    echo "       run \`cargo install cargo-about\` and try again." >&2
    exit 1
fi

mkdir -p dist

# Scope the report to the rift-client binary's dependency graph
# (--manifest-path) so we don't list server-only deps in the
# client-facing notices file.
echo "==> generating dist/THIRD_PARTY.txt"
cargo about generate \
    --config about.toml \
    --manifest-path crates/rift-client/Cargo.toml \
    "$@" \
    about.hbs > dist/THIRD_PARTY.txt

echo "done: dist/THIRD_PARTY.txt"
