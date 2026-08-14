#!/usr/bin/env bash
set -Eeuo pipefail

REPO="${DSH_REMOTE_VPS_REPO:-AlexisDevos/dsh-remote-vps}"
VERSION="latest"
PREFIX="${DSH_CONTROLLER_PREFIX:-$HOME/.local/bin}"

fail() {
  printf 'install-controller: %s\n' "$*" >&2
  exit 1
}

usage() {
  printf '%s\n' \
    'Usage: install-controller.sh [--version VERSION] [--prefix PATH] [--repo OWNER/REPO]'
}

while (($#)); do
  case "$1" in
    --version) [[ $# -ge 2 ]] || fail '--version requires a value'; VERSION="$2"; shift 2 ;;
    --prefix) [[ $# -ge 2 ]] || fail '--prefix requires a value'; PREFIX="$2"; shift 2 ;;
    --repo) [[ $# -ge 2 ]] || fail '--repo requires a value'; REPO="$2"; shift 2 ;;
    --help) usage; exit 0 ;;
    *) fail "unknown option: $1" ;;
  esac
done

case "$(uname -s):$(uname -m)" in
  Linux:x86_64|Linux:amd64) ASSET='dsh-remote-vps-linux-x86_64.tar.gz' ;;
  Darwin:x86_64) ASSET='dsh-remote-vps-macos-x86_64.tar.gz' ;;
  Darwin:arm64) ASSET='dsh-remote-vps-macos-aarch64.tar.gz' ;;
  *) fail "unsupported platform: $(uname -s) $(uname -m)" ;;
esac
command -v curl >/dev/null || fail 'curl is required'
command -v tar >/dev/null || fail 'tar is required'
if command -v sha256sum >/dev/null; then
  CHECKSUM_CMD=(sha256sum)
elif command -v shasum >/dev/null; then
  CHECKSUM_CMD=(shasum -a 256)
else
  fail 'sha256sum or shasum is required'
fi

if [[ "$VERSION" == latest ]]; then
  BASE_URL="https://github.com/${REPO}/releases/latest/download"
else
  BASE_URL="https://github.com/${REPO}/releases/download/v${VERSION#v}"
fi
TMP_DIR="$(mktemp -d /tmp/dsh-controller-install.XXXXXX)"
cleanup() { rm -rf "$TMP_DIR"; }
trap cleanup EXIT

curl --fail --location --proto '=https' --tlsv1.2 \
  --output "$TMP_DIR/$ASSET" "$BASE_URL/$ASSET"
curl --fail --location --proto '=https' --tlsv1.2 \
  --output "$TMP_DIR/SHA256SUMS" "$BASE_URL/SHA256SUMS"
EXPECTED="$(awk -v asset="$ASSET" '$2 == asset || $2 == "*" asset { print $1; exit }' "$TMP_DIR/SHA256SUMS")"
[[ "$EXPECTED" =~ ^[0-9a-fA-F]{64}$ ]] || fail "checksum for $ASSET not found"
printf '%s  %s\n' "$EXPECTED" "$TMP_DIR/$ASSET" | "${CHECKSUM_CMD[@]}" -c - >/dev/null

tar -xzf "$TMP_DIR/$ASSET" -C "$TMP_DIR"
[[ -x "$TMP_DIR/dsh-controller" ]] || fail 'release does not contain dsh-controller'
install -d -m 0755 "$PREFIX"
install -m 0755 "$TMP_DIR/dsh-controller" "$PREFIX/dsh-controller"
printf 'dsh-controller installed at %s/dsh-controller\n' "$PREFIX"
