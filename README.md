# dsh-remote-vps

English | [中文](README.zh.md) | [Français](README.fr.md)

Remote tools for [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness), with two execution modes:

- **JavaScript SSH plugin**: compatible with the existing DSH package and installs no daemon on the VPS;
- **Rust agent**: hardened daemon, versioned Protobuf/gRPC protocol, local controller, and systemd installation for production use.

The Rust mode is recommended for new installations. The daemon binds to loopback only and is reached through an SSH tunnel.

## Current release

The current release is `0.3.0`. GitHub releases provide SHA-256 verified archives:

- `dsh-remote-vps-linux-x86_64.tar.gz` — Linux x86_64 agent and controller;
- `dsh-remote-vps-macos-x86_64.tar.gz` — Intel macOS agent/controller;
- `dsh-remote-vps-macos-aarch64.tar.gz` — Apple Silicon macOS agent/controller.

## Recommended Rust installation

### Linux VPS

The VPS does not need Rust or Cargo. The installer downloads a release archive, verifies `SHA256SUMS`, creates a dedicated system user, and installs a hardened systemd unit.

```bash
git clone https://github.com/AlexisDevos/dsh-remote-vps.git /tmp/dsh-remote-vps
sudo /tmp/dsh-remote-vps/scripts/install-agent.sh \
  --version 0.3.0 \
  --root /srv/dsh-remote-vps/project
```

`Exec` is disabled by default. To enable an explicit allowlist:

```bash
sudo /tmp/dsh-remote-vps/scripts/install-agent.sh \
  --version 0.3.0 \
  --root /srv/dsh-remote-vps/project \
  --allow-exec /usr/bin/rg
```

The installed `dsh-agentd.service` uses:

- a `dsh-agent` system user without a login shell;
- `127.0.0.1:7443` only;
- `ProtectSystem=strict`, `ProtectHome`, `NoNewPrivileges`, restricted namespaces and capabilities;
- `ReadWritePaths` limited to the configured root;
- a randomly generated Bearer token at `/var/lib/dsh-agent/auth.token`, mode `0600`;
- automatic restart and clean `SIGTERM` shutdown.

```bash
sudo systemctl status dsh-agentd
sudo journalctl -u dsh-agentd -n 100 --no-pager
```

### Local controller

On Apple Silicon macOS:

```bash
curl --fail --location https://raw.githubusercontent.com/AlexisDevos/dsh-remote-vps/main/scripts/install-controller.sh \
  --output /tmp/install-dsh-controller.sh
bash /tmp/install-dsh-controller.sh --version 0.3.0
```

The installer places `dsh-controller` in `~/.local/bin`. It does not copy the VPS token automatically.

### SSH tunnel

```bash
ssh -N -o ExitOnForwardFailure=yes \
  -L 7443:127.0.0.1:7443 user@vps
```

Copy the token through an administrative SSH account without printing it in the terminal:

```bash
umask 077
mkdir -p ~/.config/dsh-remote-vps
ssh user@vps 'sudo cat /var/lib/dsh-agent/auth.token' \
  > ~/.config/dsh-remote-vps/agent.token
chmod 600 ~/.config/dsh-remote-vps/agent.token
```

In another terminal:

```bash
dsh-controller --auth-token-file ~/.config/dsh-remote-vps/agent.token health --endpoint http://127.0.0.1:7443
dsh-controller --auth-token-file ~/.config/dsh-remote-vps/agent.token capabilities --endpoint http://127.0.0.1:7443
dsh-controller --auth-token-file ~/.config/dsh-remote-vps/agent.token stat --endpoint http://127.0.0.1:7443 --root project src/main.rs
dsh-controller --auth-token-file ~/.config/dsh-remote-vps/agent.token read --endpoint http://127.0.0.1:7443 --root project README.md
```

The SSH tunnel and the Bearer token both protect the RPC endpoint. The daemon refuses non-loopback binds; do not expose it directly to the Internet or Tailscale without an additional authentication layer.

## Rust agent capabilities

- `Health`, `Capabilities`;
- bounded streaming `Stat`, `Read`, and `List`;
- streaming `Write`, atomic replacement, `fsync`, and optimistic versions;
- literal `Edit`, CRLF preservation, and optimistic versions;
- bounded `Glob` and `Grep` without following symlinks;
- `Exec` only for absolute allowlisted executables, without an implicit shell;
- timeouts, output limits, process-group termination, and descendant cleanup;
- cancellation when the gRPC client disconnects;
- root confinement, rejection of `..`, absolute paths, and symlink escapes;
- structured JSON logs and clean systemd shutdown.

Limits are enforced by the agent even when the controller sends larger values.

## JavaScript DSH plugin

The existing JavaScript plugin remains available for DSH installations:

```bash
mkdir -p ~/.dsh/profiles/web/node_modules
cp -R dsh-remote-vps ~/.dsh/profiles/web/node_modules/dsh-remote-vps
```

Add the `dsh-remote-vps` row to `cordis.patch.yml`, mount `src/fs.js` in the remote preset, and restart DSH. The plugin uses multiplexed SSH, a `0600` local store at `~/.dsh/dsh-remote-vps.json`, and a bounded loopback Settings bridge with origin checks.

This mode installs nothing on the VPS but depends on `sshd`, `python3`, and `ripgrep`. The Rust mode does not depend on those tools for filesystem operations.

## Development

Pinned toolchain: Rust `1.97.1` (`rust-toolchain.toml`).

```bash
source "$HOME/.cargo/env"
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked
cargo build --workspace --release --locked
npm install --ignore-scripts
npm run test
npm run test:client
bash -n scripts/install-agent.sh scripts/install-controller.sh scripts/package-release.sh
```

GitHub workflow `verify.yml` runs these checks on pushes and pull requests. Workflow `release.yml` builds Linux/macOS archives and publishes checksums for every `v*` tag.

## Repository layout

| Path | Role |
|---|---|
| `proto/dsh/remote/v1/remote.proto` | Versioned Protobuf contract |
| `rust/crates/protocol` | Generated gRPC contract |
| `rust/crates/core` | Path policy and file versions |
| `rust/crates/agentd` | VPS daemon |
| `rust/crates/controller` | Controller library and CLI |
| `scripts/install-agent.sh` | GitHub release VPS installer |
| `scripts/install-controller.sh` | Local controller installer |
| `packaging/systemd/dsh-agentd.service.in` | Hardened systemd template |
| `src/index.js` / `src/fs.js` | JavaScript DSH compatibility path |
| `lib/client.js` | Browser Settings section |

## VPS validation

The release has been built and tested on Ubuntu x86_64 through `ssh droitpourtous`:

- Rust workspace tests: **9 passed / 0 failed**;
- Linux release build: **passed**;
- Linux JavaScript suite: **25 passed / 0 failed**;
- real gRPC smoke test: health, stat, read, list, write, edit, glob, grep, and allowlisted exec;
- no test daemon or temporary artifact remains after validation.

## Security and known limits

- Rust transport assumes an SSH tunnel or another authenticated network boundary; the systemd installer also enables a per-installation Bearer token;
- `Exec` is a sensitive capability: enable only the strictly necessary absolute paths;
- the agent is not a complete seccomp/cgroup sandbox by itself; systemd still reduces privileges and filesystem scope;
- no private key, API key, or credential is embedded in the repository.

## License

MIT
