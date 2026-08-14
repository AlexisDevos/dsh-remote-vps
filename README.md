# dsh-remote-vps

English | [中文](README.zh.md) | [Français](README.fr.md)

Remote filesystem tools for [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness): `read`, `write`, `edit`, `read_image`, `glob`, `grep` and `bash` driving **your own VPS** over SSH (Tailscale recommended), with nothing to install on the servers.

```
[ Your machine: DSH + this package ] ── multiplexed SSH (ControlMaster) ──► [ VPS: nothing to install ]
```

No personal values are baked in: every user configures their own servers in **Settings → "VPS distants"**.

## Features

- **Full file tools**: `read`/`write`/`edit`/`read_image` keep DSH's native semantics (line numbers, atomic writes, version guards, typed errors) but run on the VPS.
- **Remote search and shell**: `glob`/`grep` (ripgrep) and `bash` (VPS shell, with the latest node via nvm when available).
- **Settings section "VPS distants"**: add/edit/delete connections, activate one, test (OK badge + latency), copy the SSH command, last-check time.
- **Multiple connections**: one active connection among several, each with its own `baseDir` (remote working directory).
- **Zero installation on the VPS**: only `sshd`, `python3` and `ripgrep` (already present on most servers) are used remotely.
- **Robust**: atomic writes (temp + rename), versioning (`mtime ns:size`), symlinks preserved, CRLF line endings preserved, typed error mapping (`FS_PERMISSION_DENIED`, `FS_STALE_VERSION`, …).

## Installation

### 1. Install the package into the web profile

```bash
mkdir -p ~/.dsh/profiles/web/node_modules
cp -R dsh-remote-vps ~/.dsh/profiles/web/node_modules/dsh-remote-vps
```

### 2. Declare the root row in the profile patch

Edit `~/.dsh/profiles/web/cordis.patch.yml`:

```yaml
- insert:
    - id: dsh-remote-vps
      name: 'dsh-remote-vps'
```

This row mounts the root plugin (SSH pool, persistent store, local HTTP bridge, `glob`/`grep`/`bash` tools, health checker). It does **not** provide the `fs` service: DSH's local filesystem stays intact for every other mode.

### 3. Create a preset that mounts the remote filesystem

Copy the `standard` preset (through the UI or `~/.dsh/.agent-presets/`), remove the `tool-fs`, `tool-fs-search` and `tool-bash` rows, then add this group:

```yaml
- id: remote
  name: cordis:group
  group: true
  isolate:
    fs: true
  config:
    - id: dsh-remote-vps-fs
      name: /absolute/path/to/dsh-remote-vps/src/fs.js

    - id: tool-fs
      name: '@deepseek-ai/dsh-tool-fs'

    - id: fs-observation-policy
      name: '@deepseek-ai/dsh-fs-observation-policy'
```

> The remote `fs` lives in an isolate realm private to each session of the preset: the `standard`/`cordis`/`minimal` modes keep their sandboxed local filesystem.

### 4. Restart and configure

```bash
npx @deepseek-ai/dsh web
```

Then: **Settings → "VPS distants" → "+ Add a VPS"** with:

| Field | Example | Required |
|---|---|---|
| Name | `My VPS` | no |
| Host | `my-vps` (Tailscale MagicDNS) or `100.x.y.z` | **yes** |
| User | `root` | no (default `root`) |
| Port | `22` | no |
| SSH key | `~/.ssh/id_ed25519` | no |
| baseDir | `/srv/app` | no |

Connections persist in `~/.dsh/dsh-remote-vps.json`. The **Test** button shows the measured latency.

## Usage

Once the preset is active, the tools work on the VPS:

- `read /home/user/app/README.md` — remote read with line numbers;
- `write` / `edit` — atomic remote writes (version guards);
- `glob "*.ts"` / `grep "workerAdapter"` — remote ripgrep (excluding `node_modules`/`.git`/`.next`/`dist`);
- `bash` — VPS shell (workdir = `baseDir` by default).

With no connection configured, the tools answer with an explicit error.

## Architecture

| File | Role |
|---|---|
| `src/index.js` (export `.`) | **Root** plugin: multiplexed SSH pool (ControlMaster), JSON store `~/.dsh/dsh-remote-vps.json`, local HTTP bridge `GET/POST /dsh-remote-vps/state` (strict loopback), health checker, `glob`/`grep`/`bash` tools. **Does not provide `fs`.** |
| `src/fs.js` (export `./fs`) | `RemoteFileSystem` class (the `fs` service) — mount it in a preset, `isolate: { fs: true }` group, beside `@deepseek-ai/dsh-tool-fs` and `@deepseek-ai/dsh-fs-observation-policy`. |
| `lib/client.js` (export `./client`) | Browser bundle: the "VPS distants" Settings section (`dsh.client` declaration in `package.json`). |
| `scripts/` | Tests and benchmark. |

### Why an HTTP bridge instead of `ctx.settings`?

DSH's settings channel (`settings.describe`/`settings.mutate`) only exposes a **fixed allowlist** of namespaces to browser clients; a third-party package cannot register itself there in this harness version. The local `/dsh-remote-vps/state` bridge (loopback only, host-side schema validation) bypasses that lock **without modifying any shipped file**.

## Security

- No open port: outgoing multiplexed SSH, authentication with your existing keys.
- The HTTP bridge only accepts loopback requests (`127.0.0.1`/`::1`).
- `StrictHostKeyChecking=accept-new`: first-contact trust (the classic SSH client TOFU model).
- No API key stored: the package only uses SSH.
- With Tailscale, the VPS's public IP can stay closed to SSH: everything goes through the tailnet.

## Tests

```bash
npm test            # backend glue: write/edit (guards, symlinks, CRLF, permissions) + coreutils commands — 22 scenarios
npm run test:client # client bundle: loading + Settings section registration
npm run bench       # median latency of the operations against the active store connection
```

Tests run locally through the pool's injectable transport (no VPS required). GNU-only commands (`find -printf`, `base64 -w0`) are skipped on macOS BSD and remain validated live on an Ubuntu VPS.

## Troubleshooting

| Symptom | Likely cause |
|---|---|
| "Pont local injoignable" shown in red in the section | The DSH server was not restarted after installation, or `cordis.patch.yml` is invalid |
| "Écriture refusée: …" | The message names the failing field (missing host, duplicate host:port, non-numeric port) |
| Section missing in Settings | Force-reload the page (Cmd+Shift+R) after restarting DSH |
| Tools: "aucune connexion VPS configurée" | Add a connection in Settings → "VPS distants" |
| "Échec" badge on test | Check the host key (`ssh-keyscan`), the SSH key, and the Tailscale scope |

## License

MIT
