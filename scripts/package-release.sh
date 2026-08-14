#!/usr/bin/env bash
set -Eeuo pipefail

VERSION="${1:-0.3.0}"
TARGET="${2:-}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DIST="$ROOT/dist"

if [[ -n "$TARGET" ]]; then
  source "$HOME/.cargo/env" 2>/dev/null || true
  cargo build --release --locked --target "$TARGET"
  BIN_DIR="$ROOT/target/$TARGET/release"
else
  source "$HOME/.cargo/env" 2>/dev/null || true
  cargo build --release --locked
  BIN_DIR="$ROOT/target/release"
fi

[[ -x "$BIN_DIR/dsh-agentd" ]] || { printf 'missing dsh-agentd\n' >&2; exit 1; }
[[ -x "$BIN_DIR/dsh-controller" ]] || { printf 'missing dsh-controller\n' >&2; exit 1; }
mkdir -p "$DIST"
if [[ -n "$TARGET" ]]; then
  case "$TARGET" in
    x86_64-unknown-linux-gnu) ASSET_TARGET='linux-x86_64' ;;
    x86_64-apple-darwin) ASSET_TARGET='macos-x86_64' ;;
    aarch64-apple-darwin) ASSET_TARGET='macos-aarch64' ;;
    *) ASSET_TARGET="$TARGET" ;;
  esac
else
  case "$(uname -s):$(uname -m)" in
    Linux:x86_64|Linux:amd64) ASSET_TARGET='linux-x86_64' ;;
    Darwin:x86_64) ASSET_TARGET='macos-x86_64' ;;
    Darwin:arm64) ASSET_TARGET='macos-aarch64' ;;
    *) ASSET_TARGET="$(uname -s | tr '[:upper:]' '[:lower:]')-$(uname -m)" ;;
  esac
fi
STAGE="$(mktemp -d "$DIST/stage.XXXXXX")"
trap 'rm -rf "$STAGE"' EXIT
install -m 0755 "$BIN_DIR/dsh-agentd" "$STAGE/dsh-agentd"
install -m 0755 "$BIN_DIR/dsh-controller" "$STAGE/dsh-controller"
install -m 0644 "$ROOT/README.md" "$STAGE/README.md"
install -m 0644 "$ROOT/LICENSE" "$STAGE/LICENSE"
install -m 0644 "$ROOT/packaging/systemd/dsh-agentd.service.in" "$STAGE/dsh-agentd.service.in"
ASSET="$DIST/dsh-remote-vps-${ASSET_TARGET}.tar.gz"
tar -czf "$ASSET" -C "$STAGE" .
( cd "$DIST" && sha256sum "$(basename "$ASSET")" > SHA256SUMS )
printf 'created %s\n' "$ASSET"
