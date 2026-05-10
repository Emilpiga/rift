#!/usr/bin/env bash
# Pack a redistributable rift-client bundle for the current host
# triple. Produces `dist/rift-client-<os>-<arch>.zip` containing
# the release binary, the assets folder, and a tiny README so a
# playtester can run it without touching Cargo.
#
# Usage:
#   # Bake the server address so playtesters can run with no flags:
#   ./scripts/package-client.sh 137.66.39.118:34000
#
#   # Or via env var (the script picks it up automatically):
#   RIFT_DEFAULT_SERVER=137.66.39.118:34000 ./scripts/package-client.sh
#
#   # Plain build (no default; clients must pass --connect):
#   ./scripts/package-client.sh

set -euo pipefail

cd "$(dirname "$0")/.."

# First positional arg overrides the env var. Empty string =
# ship a no-default client.
if [[ $# -ge 1 ]]; then
    if [[ -n "$1" ]]; then
        export RIFT_DEFAULT_SERVER="$1"
    else
        unset RIFT_DEFAULT_SERVER
    fi
fi

if [[ -n "${RIFT_DEFAULT_SERVER:-}" ]]; then
    echo "==> baking RIFT_DEFAULT_SERVER=$RIFT_DEFAULT_SERVER"
else
    echo "==> no server baked in; clients will need --connect"
fi

echo "==> cargo build --release -p rift-client"
cargo build --release -p rift-client

# Pick the right binary name. Cargo writes `rift` (the bin name
# inside rift-client) on every platform; on Windows it grows the
# `.exe` extension.
case "${OSTYPE:-}" in
    msys*|cygwin*|win32) BIN=rift.exe ;;
    *)                   BIN=rift ;;
esac

# Host triple for the archive name. `rustc -vV` prints `host: x`.
HOST=$(rustc -vV | awk '/host:/ {print $2}')
OUT_DIR=dist
STAGE=$OUT_DIR/rift-client
ARCHIVE=$OUT_DIR/rift-client-$HOST.zip

rm -rf "$STAGE" "$ARCHIVE"
mkdir -p "$STAGE"

echo "==> staging binary + assets"
cp "target/release/$BIN" "$STAGE/"
cp -R assets "$STAGE/assets"

# Generate (or refresh) the third-party license attribution
# file and stage it next to the binary. Required by every
# storefront we'd plausibly publish through, and by the MIT /
# Apache-2.0 / BSD-* licenses themselves. Skipped with a
# warning if `cargo-about` isn't installed so a developer
# without it can still cut a local test build — but a release
# bundle without `THIRD_PARTY.txt` is not legally
# distributable.
if command -v cargo-about >/dev/null 2>&1; then
    echo "==> regenerating THIRD_PARTY.txt"
    "$(dirname "$0")/gen-third-party.sh"
    cp dist/THIRD_PARTY.txt "$STAGE/"
else
    echo "WARNING: cargo-about not installed; THIRD_PARTY.txt will be missing"
    echo "         from this bundle. Run \`cargo install cargo-about\` and"
    echo "         re-package before distributing."
fi

if [[ -n "${RIFT_DEFAULT_SERVER:-}" ]]; then
    server_line="Connects automatically to $RIFT_DEFAULT_SERVER."
else
    server_line="Run with --connect HOST:PORT to join a server."
fi
cat > "$STAGE/README.txt" <<EOF
Rift Crawler — playtest build ($HOST)

Run the game:
    ./$BIN

$server_line

Override the baked-in server (if any):
    ./$BIN --connect HOST:PORT
or set the env var:
    RIFT_SERVER=HOST:PORT ./$BIN

Skip multiplayer entirely:
    ./$BIN --offline

Notes:
* You need a Vulkan-capable GPU + a modern driver. Every
  mainstream 2018+ GPU qualifies. If the game complains about
  "no Vulkan instance", update your graphics driver.
* All assets must stay next to the binary. Don't move "$BIN"
  out of this folder.
EOF

echo "==> creating $ARCHIVE"
( cd "$OUT_DIR" && zip -qr "$(basename "$ARCHIVE")" rift-client )

echo "done: $ARCHIVE"
