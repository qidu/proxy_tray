#!/usr/bin/env bash
#
# Copies the built model-proxy-v3 SEA binary into src-tauri/binaries/ under the
# target-triple name Tauri's `externalBin` requires.
#
# This is not optional busywork: tauri_build::build() copies externalBin during
# build.rs and errors if the file is missing, so `cargo check` fails without it.
#
# Usage: bash scripts/stage-sidecar.sh [path-to-sea-binary]

set -euo pipefail

cd "$(dirname "$0")/.."

TRIPLE="$(rustc --print host-tuple)"
DEST="src-tauri/binaries/model-proxy-v3-$TRIPLE"

if [ "$#" -ge 1 ]; then
  SRC="$1"
else
  # Prefer the submodule's own build output, then the sibling checkout's — the
  # submodule is a fresh clone, so its dist/ is usually empty. build-sea.js
  # names the binary with the host triple already, so no rename is needed.
  SRC=""
  for candidate in \
    "model_proxy_v3/dist/model-proxy-v3-$TRIPLE" \
    "../model_proxy_v3/dist/model-proxy-v3-$TRIPLE"
  do
    if [ -f "$candidate" ]; then
      SRC="$candidate"
      break
    fi
  done
fi

if [ -z "$SRC" ] || [ ! -f "$SRC" ]; then
  echo "stage-sidecar: no SEA binary found." >&2
  echo "Build one first:" >&2
  echo "  cd model_proxy_v3 && npm ci && npx --yes --package=node@26 node scripts/build-sea.js" >&2
  echo "or pass a path: bash scripts/stage-sidecar.sh /path/to/model-proxy-v3-$TRIPLE" >&2
  exit 1
fi

mkdir -p "$(dirname "$DEST")"
cp "$SRC" "$DEST"
chmod +x "$DEST"

echo "staged $SRC -> $DEST"
