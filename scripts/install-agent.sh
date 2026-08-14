#!/usr/bin/env bash
set -Eeuo pipefail

REPO="${DSH_REMOTE_VPS_REPO:-AlexisDevos/dsh-remote-vps}"
VERSION="latest"
ROOT="/srv/dsh-remote-vps/project"
PORT="7443"
PREFIX="/usr/local/libexec/dsh-remote-vps"
SERVICE="dsh-agentd.service"
ALLOW_EXEC=()

usage() {
  printf '%s\n' \
    'Usage: install-agent.sh [options]' \
    '  --version VERSION       Release version without or with v (default: latest)' \
    '  --root PATH             Dedicated writable project root (default: /srv/dsh-remote-vps/project)' \
    '  --port PORT             Loopback gRPC port (default: 7443)' \
    '  --allow-exec PATH       Absolute executable allowlist entry; repeatable' \
    '  --repo OWNER/REPO       GitHub repository override' \
    '  --help'
}

fail() {
  printf 'install-agent: %s\n' "$*" >&2
  exit 1
}

while (($#)); do
  case "$1" in
    --version) [[ $# -ge 2 ]] || fail '--version requires a value'; VERSION="$2"; shift 2 ;;
    --root) [[ $# -ge 2 ]] || fail '--root requires a value'; ROOT="$2"; shift 2 ;;
    --port) [[ $# -ge 2 ]] || fail '--port requires a value'; PORT="$2"; shift 2 ;;
    --allow-exec) [[ $# -ge 2 ]] || fail '--allow-exec requires a value'; ALLOW_EXEC+=("$2"); shift 2 ;;
    --repo) [[ $# -ge 2 ]] || fail '--repo requires a value'; REPO="$2"; shift 2 ;;
    --help) usage; exit 0 ;;
    *) fail "unknown option: $1" ;;
  esac
done

[[ "${EUID}" -eq 0 ]] || fail 'run as root'
[[ "$(uname -s)" == "Linux" ]] || fail 'the agent installer targets Linux'
case "$(uname -m)" in
  x86_64|amd64) ASSET='dsh-remote-vps-linux-x86_64.tar.gz' ;;
  *) fail "unsupported architecture: $(uname -m); use a release asset or build from source" ;;
esac
[[ "$ROOT" == /* && "$ROOT" != *$'\n'* && "$ROOT" != *' '* && "$ROOT" != *'|'* && "$ROOT" != *'&'* && "$ROOT" != *'\\'* ]] || fail '--root must be an absolute path without spaces, newlines, pipes, ampersands or backslashes'
[[ "$PORT" =~ ^[0-9]+$ && "$PORT" -ge 1024 && "$PORT" -le 65535 ]] || fail '--port must be between 1024 and 65535'
for command in "${ALLOW_EXEC[@]}"; do
  [[ "$command" == /* && "$command" != *$'\n'* && "$command" != *' '* && "$command" != *'|'* && "$command" != *'&'* && "$command" != *'\\'* ]] || fail '--allow-exec entries must be absolute paths without spaces, newlines, pipes, ampersands or backslashes'
done

command -v curl >/dev/null || fail 'curl is required'
command -v tar >/dev/null || fail 'tar is required'
command -v sha256sum >/dev/null || fail 'sha256sum is required'
command -v systemctl >/dev/null || fail 'systemd is required'

if [[ "$VERSION" == latest ]]; then
  BASE_URL="https://github.com/${REPO}/releases/latest/download"
  INSTALL_ID="latest-$(date -u +%Y%m%d%H%M%S)"
else
  TAG="${VERSION#v}"
  BASE_URL="https://github.com/${REPO}/releases/download/v${TAG}"
  INSTALL_ID="$TAG"
fi

TMP_DIR="$(mktemp -d /tmp/dsh-agent-install.XXXXXX)"
cleanup() { rm -rf "$TMP_DIR"; }
trap cleanup EXIT

curl --fail --location --proto '=https' --tlsv1.2 \
  --output "$TMP_DIR/$ASSET" "$BASE_URL/$ASSET"
curl --fail --location --proto '=https' --tlsv1.2 \
  --output "$TMP_DIR/SHA256SUMS" "$BASE_URL/SHA256SUMS"
EXPECTED="$(awk -v asset="$ASSET" '$2 == asset || $2 == "*" asset { print $1; exit }' "$TMP_DIR/SHA256SUMS")"
[[ "$EXPECTED" =~ ^[0-9a-fA-F]{64}$ ]] || fail "checksum for $ASSET not found"
printf '%s  %s\n' "$EXPECTED" "$TMP_DIR/$ASSET" | sha256sum -c - >/dev/null

tar -xzf "$TMP_DIR/$ASSET" -C "$TMP_DIR"
[[ -x "$TMP_DIR/dsh-agentd" ]] || fail 'release does not contain dsh-agentd'

install -d -m 0755 "$PREFIX/$INSTALL_ID"
install -m 0755 "$TMP_DIR/dsh-agentd" "$PREFIX/$INSTALL_ID/dsh-agentd"
if [[ -x "$TMP_DIR/dsh-controller" ]]; then
  install -m 0755 "$TMP_DIR/dsh-controller" "$PREFIX/$INSTALL_ID/dsh-controller"
fi
PREVIOUS_TARGET="$(readlink -f "$PREFIX/current" 2>/dev/null || true)"
ln -sfn "$PREFIX/$INSTALL_ID" "$PREFIX/current"

if ! getent passwd dsh-agent >/dev/null; then
  useradd --system --user-group --home-dir /var/lib/dsh-agent --create-home \
    --shell /usr/sbin/nologin dsh-agent
fi
if ! getent group dsh-agent >/dev/null; then
  groupadd --system dsh-agent
fi
install -d -o dsh-agent -g dsh-agent -m 0750 /var/lib/dsh-agent
if [[ ! -s /var/lib/dsh-agent/auth.token ]]; then
  umask 077
  od -An -N32 -tx1 /dev/urandom | tr -d ' \n' > /var/lib/dsh-agent/auth.token
fi
chown dsh-agent:dsh-agent /var/lib/dsh-agent/auth.token
chmod 0600 /var/lib/dsh-agent/auth.token
if [[ ! -e "$ROOT" ]]; then
  install -d -o dsh-agent -g dsh-agent -m 0750 "$ROOT"
elif ! runuser -u dsh-agent -- test -w "$ROOT"; then
  fail "existing root is not writable by dsh-agent: $ROOT; choose a dedicated --root or adjust ownership explicitly"
fi

ALLOW_ARGS=''
for command in "${ALLOW_EXEC[@]}"; do
  ALLOW_ARGS+=" --allow-exec $command"
done
SERVICE_TMP="$TMP_DIR/$SERVICE"
sed \
  -e "s|@ROOT@|$ROOT|g" \
  -e "s|@PREFIX@|$PREFIX|g" \
  -e "s|@PORT@|$PORT|g" \
  -e "s|@ALLOW_EXEC@|$ALLOW_ARGS|g" \
  "$(dirname "$0")/../packaging/systemd/dsh-agentd.service.in" > "$SERVICE_TMP"
install -m 0644 "$SERVICE_TMP" "/etc/systemd/system/$SERVICE"
systemctl daemon-reload
systemctl enable "$SERVICE"
if ! systemctl restart "$SERVICE"; then
  if [[ -n "$PREVIOUS_TARGET" && -x "$PREVIOUS_TARGET/dsh-agentd" ]]; then
    ln -sfn "$PREVIOUS_TARGET" "$PREFIX/current"
    systemctl daemon-reload
    systemctl restart "$SERVICE" || true
  fi
  fail 'agent service did not start; previous version restored when available'
fi
systemctl is-active --quiet "$SERVICE" || {
  systemctl --no-pager --full status "$SERVICE" >&2 || true
  fail 'agent service did not become active'
}

printf 'dsh-agentd installed: version=%s root=%s listen=127.0.0.1:%s\n' "$INSTALL_ID" "$ROOT" "$PORT"
printf 'SSH tunnel: ssh -N -L %s:127.0.0.1:%s user@vps\n' "$PORT" "$PORT"
