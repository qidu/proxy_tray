#!/usr/bin/env bash
#
# Downloads the latest model-proxy-v3 release asset for this host and stages it
# under the target-triple name Tauri's externalBin requires. Windows releases
# also provide keytar.node and its license for Credential Vault support.

set -euo pipefail

cd "$(dirname "$0")/.."

if [ "$#" -ne 0 ]; then
  echo "Usage: bash scripts/stage-sidecar.sh" >&2
  exit 2
fi

TRIPLE="$(rustc --print host-tuple)"
NAME="model-proxy-v3-$TRIPLE"
BASE_URL="https://github.com/qidu/model_proxy_v3/releases/latest/download"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

case "$TRIPLE" in
  *-pc-windows-msvc)
    ASSET="$NAME.zip"
    URL="$BASE_URL/$ASSET"
    ARCHIVE="$WORK/$ASSET"
    echo "Downloading $URL"
    curl --fail --silent --show-error --location --http1.1 --retry 3 --retry-all-errors \
      --output "$ARCHIVE" "$URL"
    TAR="tar"
    if [ -x "C:/Windows/System32/tar.exe" ]; then
      TAR="C:/Windows/System32/tar.exe"
    fi
    "$TAR" -xf "$ARCHIVE" -C "$WORK" "$NAME.exe" keytar.node keytar-LICENSE.md
    SRC="$WORK/$NAME.exe"
    DEST="src-tauri/binaries/$NAME.exe"
    ;;
  *)
    ASSET="$NAME"
    URL="$BASE_URL/$ASSET"
    SRC="$WORK/$ASSET"
    DEST="src-tauri/binaries/$NAME"
    echo "Downloading $URL"
    curl --fail --silent --show-error --location --http1.1 --retry 3 --retry-all-errors \
      --output "$SRC" "$URL"
    ;;
esac

if [ ! -s "$SRC" ]; then
  echo "stage-sidecar: downloaded executable is missing or empty: $SRC" >&2
  exit 1
fi

mkdir -p "$(dirname "$DEST")"
cp "$SRC" "$DEST"
chmod +x "$DEST"

if [[ "$TRIPLE" == *-pc-windows-msvc ]]; then
  cp "$WORK/keytar.node" src-tauri/binaries/keytar.node
  cp "$WORK/keytar-LICENSE.md" src-tauri/binaries/keytar-LICENSE.md
  echo "staged $WORK/keytar.node -> src-tauri/binaries/keytar.node"
fi

echo "staged $SRC -> $DEST"
