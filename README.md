# dsh-remote-vps

Des outils fichiers **distants** pour [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness) : `read`, `write`, `edit`, `read_image`, `glob`, `grep` et `bash` pilotant **vos propres VPS** via SSH (recommandé : Tailscale), sans rien installer sur les serveurs.

```
[ Votre machine : DSH + ce package ] ── SSH multiplexé (ControlMaster) ──► [ VPS : rien à installer ]
```

Aucune valeur personnelle n'est embarquée : chaque utilisateur configure ses serveurs dans **Settings → « VPS distants »**.

## Fonctionnalités

- **Outils fichiers complets** : `read`/`write`/`edit`/`read_image` gardent la sémantique native de DSH (numéros de ligne, écritures atomiques, gardes de version, erreurs typées) mais s'exécutent sur le VPS.
- **Recherche et shell distants** : `glob`/`grep` (ripgrep) et `bash` (shell du VPS, avec node le plus récent via nvm si présent).
- **Section Settings « VPS distants »** : ajouter/éditer/supprimer des connexions, activer une connexion, tester (badge OK + latence), copier la commande SSH, heure de dernière vérification.
- **Connexions multiples** : une connexion active parmi plusieurs, chacune avec son `baseDir` (répertoire de travail distant).
- **Zéro installation sur le VPS** : seuls `sshd`, `python3` et `ripgrep` (déjà présents sur la plupart des serveurs) sont utilisés à distance.
- **Robuste** : écritures atomiques (temp + rename), versionnage (`mtime ns:size`), symlinks préservés, fins de ligne CRLF préservées, mapping d'erreurs typé (`FS_PERMISSION_DENIED`, `FS_STALE_VERSION`, …).

## Installation

### 1. Installer le package dans le profil web

```bash
mkdir -p ~/.dsh/profiles/web/node_modules
cp -R dsh-remote-vps ~/.dsh/profiles/web/node_modules/dsh-remote-vps
```

### 2. Déclarer la rangée racine dans le patch du profil

Éditer `~/.dsh/profiles/web/cordis.patch.yml` :

```yaml
- insert:
    - id: dsh-remote-vps
      name: 'dsh-remote-vps'
```

Cette rangée monte le plugin racine (pool SSH, store persistant, pont HTTP local, outils `glob`/`grep`/`bash`, health-checker). Elle ne fournit **pas** le service `fs` : le filesystem local de DSH reste intact pour tous les autres modes.

### 3. Créer un preset qui monte le filesystem distant

Copier le preset `standard` (via l'interface ou `~/.dsh/.agent-presets/`), retirer les rangées `tool-fs`, `tool-fs-search` et `tool-bash`, puis ajouter ce groupe :

```yaml
- id: remote
  name: cordis:group
  group: true
  isolate:
    fs: true
  config:
    - id: dsh-remote-vps-fs
      name: /chemin/absolu/vers/dsh-remote-vps/src/fs.js

    - id: tool-fs
      name: '@deepseek-ai/dsh-tool-fs'

    - id: fs-observation-policy
      name: '@deepseek-ai/dsh-fs-observation-policy'
```

> Le `fs` distant vit dans un realm isolé propre à chaque session du preset : les modes `standard`/`cordis`/`minimal` gardent leur filesystem local sandboxé.

### 4. Redémarrer et configurer

```bash
npx @deepseek-ai/dsh web
```

Puis : **Settings → « VPS distants » → « + Ajouter un VPS »** avec :

| Champ | Exemple | Obligatoire |
|---|---|---|
| Nom | `Mon VPS` | non |
| Hôte | `mon-vps` (MagicDNS Tailscale) ou `100.x.y.z` | **oui** |
| Utilisateur | `root` | non (défaut `root`) |
| Port | `22` | non |
| Clé SSH | `~/.ssh/id_ed25519` | non |
| baseDir | `/srv/app` | non |

Les connexions sont persistées dans `~/.dsh/dsh-remote-vps.json`. Le bouton **Tester** affiche la latence mesurée.

## Utilisation

Une fois le preset actif, les outils travaillent sur le VPS :

- `read /home/user/app/README.md` — lecture distante avec numéros de ligne ;
- `write` / `edit` — écritures atomiques distantes (gardes de version) ;
- `glob "*.ts"` / `grep "workerAdapter"` — ripgrep distant (hors `node_modules`/`.git`/`.next`/`dist`) ;
- `bash` — shell du VPS (workdir = `baseDir` par défaut).

Sans connexion configurée, les outils répondent par une erreur explicite.

## Architecture

| Fichier | Rôle |
|---|---|
| `src/index.js` (export `.`) | Plugin **racine** : pool SSH multiplexé (ControlMaster), store JSON `~/.dsh/dsh-remote-vps.json`, pont HTTP local `GET/POST /dsh-remote-vps/state` (loopback strict), health-checker, outils `glob`/`grep`/`bash`. **Ne fournit pas `fs`.** |
| `src/fs.js` (export `./fs`) | Classe `RemoteFileSystem` (service `fs`) — à monter dans un preset, groupe `isolate: { fs: true }`, à côté de `@deepseek-ai/dsh-tool-fs` et `@deepseek-ai/dsh-fs-observation-policy`. |
| `lib/client.js` (export `./client`) | Bundle navigateur : section Settings « VPS distants » (déclaration `dsh.client` dans `package.json`). |
| `scripts/` | Tests et benchmark. |

### Pourquoi un pont HTTP plutôt que `ctx.settings` ?

Le canal de réglages de DSH (`settings.describe`/`settings.mutate`) n'expose aux clients navigateur qu'une **liste blanche fixe** de namespaces ; un package tiers ne peut pas s'y inscrire dans cette version du harness. Le pont local `/dsh-remote-vps/state` (loopback uniquement, validation par schéma côté hôte) contourne ce verrou **sans modifier aucun fichier livré**.

## Sécurité

- Aucun port ouvert : SSH sortant multiplexé, authentification par vos clés existantes.
- Le pont HTTP n'accepte que les requêtes loopback (`127.0.0.1`/`::1`).
- `StrictHostKeyChecking=accept-new` : premier contact de confiance (modèle TOFU du client SSH classique).
- Aucune clé API stockée : le package n'utilise que SSH.
- En Tailscale, l'IP publique du VPS peut rester fermée au SSH : tout passe par le tailnet.

## Tests

```bash
npm test            # glue backend : write/edit (gardes, symlinks, CRLF, permissions) + commandes coreutils — 22 scénarios
npm run test:client # bundle client : chargement + enregistrement de la section Settings
npm run bench       # latence médiane des opérations contre la connexion active du store
```

Les tests s'exécutent en local via le transport injectable du pool (aucun VPS requis). Les commandes GNU-only (`find -printf`, `base64 -w0`) sont sautées sur macOS BSD et restent validées en live sur un VPS Ubuntu.

## Dépannage

| Symptôme | Cause probable |
|---|---|
| « Pont local injoignable » en rouge dans la section | Le serveur DSH n'a pas été redémarré après l'installation, ou le patch `cordis.patch.yml` est invalide |
| « Écriture refusée : … » | Le message précise le champ en erreur (hôte obligatoire, doublon host:port, port non numérique) |
| Section absente dans Settings | Recharger la page en forçant (Cmd+Shift+R) après redémarrage de DSH |
| Outils : « aucune connexion VPS configurée » | Ajouter une connexion dans Settings → « VPS distants » |
| Badge « Échec » au test | Vérifier la clé d'hôte (`ssh-keyscan`), la clé SSH et la portée Tailscale |

## Licence

MIT
