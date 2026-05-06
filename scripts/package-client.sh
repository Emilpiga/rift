#!/usr/bin/env bash
# Pack a redistributable rift-client bundle for the current host
# triple. Produces `dist/rift-client-<os>-<arch>.zip` containing
# the release binary, the assets folder, and a tiny README so a
# playtester can run it without touching Cargo.

set -euo pipefail

cd "$(dirname "$0")/.."

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
cat > "$STAGE/README.txt" <<EOF
Rift Crawler — playtest build ($HOST)

Run the game:
    ./$BIN

Connect to a server:
    ./$BIN --connect HOST:PORT

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
