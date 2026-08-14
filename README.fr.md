# dsh-remote-vps

[English](README.md) | [中文](README.zh.md) | Français

Outils distants pour [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness), avec deux modes d’exécution :

- **plugin JavaScript SSH** : compatible avec le package DSH existant, sans daemon à installer sur le VPS ;
- **agent Rust** : daemon durci, protocole Protobuf/gRPC versionné, controller local et installation systemd pour un usage production.

Le mode Rust est recommandé pour une nouvelle installation. Le daemon écoute uniquement sur loopback et doit être atteint par un tunnel SSH.

## Release actuelle

La version courante est `0.3.0`. Les releases GitHub fournissent des archives vérifiées par `SHA256SUMS` :

- `dsh-remote-vps-linux-x86_64.tar.gz` — agent + controller Linux x86_64 ;
- `dsh-remote-vps-macos-x86_64.tar.gz` — controller/agent macOS Intel ;
- `dsh-remote-vps-macos-aarch64.tar.gz` — controller/agent macOS Apple Silicon.

## Installation Rust recommandée

### VPS Linux

Le VPS n’a pas besoin de Rust ni de Cargo : l’installateur télécharge l’archive de release, vérifie son SHA-256, crée un utilisateur système dédié et installe un service systemd durci.

```bash
git clone https://github.com/AlexisDevos/dsh-remote-vps.git /tmp/dsh-remote-vps
sudo /tmp/dsh-remote-vps/scripts/install-agent.sh \
  --version 0.3.0 \
  --root /srv/dsh-remote-vps/project
```

`Exec` reste désactivé par défaut. Pour une allowlist explicite :

```bash
sudo /tmp/dsh-remote-vps/scripts/install-agent.sh \
  --version 0.3.0 \
  --root /srv/dsh-remote-vps/project \
  --allow-exec /usr/bin/rg
```

Le service créé est `dsh-agentd.service`, avec :

- utilisateur `dsh-agent` sans shell de connexion ;
- bind `127.0.0.1:7443` ;
- `ProtectSystem=strict`, `ProtectHome`, `NoNewPrivileges`, namespaces et capacités restreints ;
- `ReadWritePaths` limité à la racine configurée ;
- un token Bearer aléatoire dans `/var/lib/dsh-agent/auth.token`, mode `0600` ;
- redémarrage automatique et arrêt propre sur `SIGTERM`.

Pour contrôler le service :

```bash
sudo systemctl status dsh-agentd
sudo journalctl -u dsh-agentd -n 100 --no-pager
```

### Controller local

Sur macOS Apple Silicon :

```bash
curl --fail --location https://raw.githubusercontent.com/AlexisDevos/dsh-remote-vps/main/scripts/install-controller.sh \
  --output /tmp/install-dsh-controller.sh
bash /tmp/install-dsh-controller.sh --version 0.3.0
```

L’installateur place le binaire dans `~/.local/bin/dsh-controller`. Il ne copie pas automatiquement le token du VPS.

### Tunnel SSH

```bash
ssh -N -o ExitOnForwardFailure=yes \
  -L 7443:127.0.0.1:7443 user@vps
```

Copier le token via un compte SSH administrateur sans l’afficher dans le terminal :

```bash
umask 077
mkdir -p ~/.config/dsh-remote-vps
ssh user@vps 'sudo cat /var/lib/dsh-agent/auth.token' \
  > ~/.config/dsh-remote-vps/agent.token
chmod 600 ~/.config/dsh-remote-vps/agent.token
```

Dans un autre terminal :

```bash
dsh-controller --auth-token-file ~/.config/dsh-remote-vps/agent.token health --endpoint http://127.0.0.1:7443
dsh-controller --auth-token-file ~/.config/dsh-remote-vps/agent.token capabilities --endpoint http://127.0.0.1:7443
dsh-controller --auth-token-file ~/.config/dsh-remote-vps/agent.token stat --endpoint http://127.0.0.1:7443 --root project src/main.rs
dsh-controller --auth-token-file ~/.config/dsh-remote-vps/agent.token read --endpoint http://127.0.0.1:7443 --root project README.md
```

Le tunnel SSH et le token Bearer protègent tous deux le endpoint RPC. Le daemon refuse tout bind non-loopback ; ne l’expose pas directement sur Internet ou Tailscale sans couche d’authentification supplémentaire.

## Fonctionnalités de l’agent Rust

- `Health`, `Capabilities` ;
- `Stat`, `Read`, `List` avec streams et limites ;
- `Write` streaming, écritures atomiques, `fsync`, version optimiste ;
- `Edit` littéral, gestion CRLF, version optimiste ;
- `Glob`, `Grep` bornés, sans suivi de symlinks ;
- `Exec` uniquement par chemin absolu allowlisté, sans shell implicite ;
- timeout, limite de sortie, groupe de processus isolé et arrêt des descendants ;
- annulation quand le client gRPC se déconnecte ;
- confinement de chemins par racine, refus de `..`, chemins absolus et sorties par symlink ;
- logs JSON structurés et arrêt propre systemd.

Les limites sont appliquées côté agent, même si le controller envoie des valeurs plus grandes.

## Plugin DSH JavaScript

Le plugin historique reste disponible pour les installations DSH existantes :

```bash
mkdir -p ~/.dsh/profiles/web/node_modules
cp -R dsh-remote-vps ~/.dsh/profiles/web/node_modules/dsh-remote-vps
```

Ajouter ensuite la ligne `dsh-remote-vps` dans `cordis.patch.yml`, monter `src/fs.js` dans le preset distant, puis redémarrer DSH. Le plugin utilise SSH multiplexé, un store local `~/.dsh/dsh-remote-vps.json` protégé en `0600` et un bridge Settings loopback limité et anti-CSRF.

Ce mode n’installe rien sur le VPS mais dépend de `sshd`, `python3` et `ripgrep`. Le mode Rust ne dépend pas de ces outils pour les opérations filesystem.

## Développement

Toolchain imposée : Rust `1.97.1` (`rust-toolchain.toml`).

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

Le workflow GitHub `verify.yml` exécute ces contrôles sur chaque push/PR. Le workflow `release.yml` construit les archives Linux/macOS et publie leurs checksums à chaque tag `v*`.

## Structure

| Chemin | Rôle |
|---|---|
| `proto/dsh/remote/v1/remote.proto` | Contrat Protobuf versionné |
| `rust/crates/protocol` | Code gRPC généré |
| `rust/crates/core` | Politique de chemins et versions |
| `rust/crates/agentd` | Daemon VPS |
| `rust/crates/controller` | Bibliothèque et CLI controller |
| `scripts/install-agent.sh` | Installation VPS depuis une release GitHub |
| `scripts/install-controller.sh` | Installation du controller local |
| `packaging/systemd/dsh-agentd.service.in` | Modèle systemd durci |
| `src/index.js` / `src/fs.js` | Compatibilité plugin DSH JavaScript |
| `lib/client.js` | Section Settings navigateur |

## Tests VPS réalisés

La release a été compilée et testée sur Ubuntu x86_64 via `ssh droitpourtous` :

- tests Rust workspace : **9 OK / 0 KO** ;
- build release Linux : **OK** ;
- suite JavaScript Linux : **25 OK / 0 KO** ;
- smoke gRPC réel : health, stat, read, list, write, edit, glob, grep et exec ;
- aucun daemon de test ni artefact temporaire conservé après validation.

## Sécurité et limites connues

- le transport Rust suppose un tunnel SSH ou une autre frontière réseau authentifiée ; l’installation systemd active aussi un token Bearer par installation ;
- `Exec` reste une capability sensible : ne l’activez qu’avec des chemins absolus strictement nécessaires ;
- l’agent ne fournit pas de sandbox kernel complète seccomp/cgroups par lui-même ; le service systemd réduit toutefois les privilèges et le périmètre ;
- aucune clé privée, API key ou credential n’est embarqué dans le dépôt.

## Licence

MIT
