// dsh-remote-vps — host plugin for DSH
//
// Provides (inside the preset's isolate realm):
//   - `sshPool`  : SSH multiplexed connection pool over Tailscale (ControlMaster)
//   - `fs`       : a remote FileSystem implementation (read/write/edit/read_image
//                  via the native @deepseek-ai/dsh-tool-fs mounted beside this row)
// Registers tools: `glob`, `grep`, `bash` (remote, same names as the native ones)
// Registers settings namespaces:
//   - `dsh-remote-vps`        : connections list + active connection + testRequest
//   - `dsh-remote-vps-status` : per-connection health (ok, latency, error)
//
// Zero install on the VPS: only sshd + python3 + ripgrep are used remotely.

import { Service } from '@deepseek-ai/cordis'
import { defineTool } from '@deepseek-ai/dsh-tools'
import { FileSystem, FsTargetKey, FsVersion, FsError } from '@deepseek-ai/dsh-fs'
import z from '@deepseek-ai/schemastery'
import { spawn } from 'node:child_process'
import { createHash } from 'node:crypto'
import { readFileSync, writeFileSync, renameSync, mkdirSync, chmodSync } from 'node:fs'
import { homedir, tmpdir } from 'node:os'
import { dirname, join } from 'node:path'

const SSH = '/usr/bin/ssh'

const NO_CONNECTION = 'Erreur : aucune connexion VPS configurée. Ajoute une connexion dans Settings → « VPS distants ».'

const name = 'dsh-remote-vps'
const inject = ['timer', 'tools', 'webServer']

const WRITE_SCRIPT = [
  '',
  'import sys, base64, os, tempfile, json',
  'p = base64.b64decode(sys.argv[1]).decode()',
  'mode = base64.b64decode(sys.argv[2]).decode()',
  'exp = base64.b64decode(sys.argv[3]).decode()',
  'data = sys.stdin.buffer.read()',
  'rp = os.path.realpath(p)',
  'exists = os.path.lexists(p)',
  'if mode == "create" and exists:',
  '    sys.stderr.write("write: target already exists (createIfAbsent)")',
  '    sys.exit(6)',
  'before = None',
  'mode_bits = 0',
  'if exists:',
  '    try:',
  '        st = os.stat(p)',
  '    except PermissionError:',
  '        sys.stderr.write("write: permission denied")',
  '        sys.exit(9)',
  '    cur = "%d:%d" % (st.st_mtime_ns, st.st_size)',
  '    if mode == "update" and exp and cur != exp:',
  '        sys.stderr.write("write: stale version")',
  '        sys.exit(7)',
  '    before = open(rp, "rb").read()',
  '    mode_bits = st.st_mode & 0o7777',
  'd = os.path.dirname(rp) or "."',
  'if d and not os.path.isdir(d):',
  '    try:',
  '        os.makedirs(d, exist_ok=True)',
  '    except PermissionError:',
  '        sys.stderr.write("write: permission denied")',
  '        sys.exit(9)',
  'try:',
  '    fd, tmp = tempfile.mkstemp(dir=d, prefix=".dsh-write-")',
  'except PermissionError:',
  '    sys.stderr.write("write: permission denied")',
  '    sys.exit(9)',
  'try:',
  '    with os.fdopen(fd, "wb") as f:',
  '        f.write(data)',
  '    if mode_bits:',
  '        os.chmod(tmp, mode_bits)',
  '    os.replace(tmp, rp)',
  'except PermissionError:',
  '    sys.stderr.write("write: permission denied")',
  '    sys.exit(9)',
  'except IsADirectoryError:',
  '    sys.stderr.write("write: is a directory")',
  '    sys.exit(8)',
  'finally:',
  '    if os.path.exists(tmp):',
  '        os.remove(tmp)',
  'st2 = os.stat(rp)',
  'out = {"op": "update" if exists else "create", "version": "%d:%d" % (st2.st_mtime_ns, st2.st_size)}',
  'if before is not None:',
  '    out["before"] = base64.b64encode(before).decode()',
  'print(json.dumps(out))',
].join('\n')

const EDIT_SCRIPT = [
  '',
  'import sys, base64, os, tempfile, json',
  'p = base64.b64decode(sys.argv[1]).decode()',
  'old = base64.b64decode(sys.argv[2]).decode()',
  'new = base64.b64decode(sys.argv[3]).decode()',
  'rall = base64.b64decode(sys.argv[4]).decode()',
  'exp = base64.b64decode(sys.argv[5]).decode()',
  'rp = os.path.realpath(p)',
  'try:',
  '    raw = open(rp, "rb").read()',
  'except FileNotFoundError:',
  '    sys.stderr.write("edit: not found")',
  '    sys.exit(2)',
  'except IsADirectoryError:',
  '    sys.stderr.write("edit: is a directory")',
  '    sys.exit(8)',
  'except PermissionError:',
  '    sys.stderr.write("edit: permission denied")',
  '    sys.exit(9)',
  'try:',
  '    text = raw.decode("utf-8")',
  'except UnicodeDecodeError:',
  '    sys.stderr.write("edit: not UTF-8 text")',
  '    sys.exit(3)',
  'st = os.stat(rp)',
  'cur = "%d:%d" % (st.st_mtime_ns, st.st_size)',
  'if exp and cur != exp:',
  '    sys.stderr.write("edit: stale version")',
  '    sys.exit(7)',
  'crlf = "\\r\\n" in text',
  'norm = text.replace("\\r\\n", "\\n")',
  'old_n = old.replace("\\r\\n", "\\n")',
  'cnt = norm.count(old_n)',
  'if cnt == 0:',
  '    sys.stderr.write("edit: old_string not found")',
  '    sys.exit(4)',
  'if cnt > 1 and rall != "1":',
  '    sys.stderr.write("edit: old_string appears %d times" % cnt)',
  '    sys.exit(5)',
  'after_n = norm.replace(old_n, new.replace("\\r\\n", "\\n"), -1 if rall == "1" else 1)',
  'after = after_n.replace("\\n", "\\r\\n") if crlf else after_n',
  'd = os.path.dirname(rp) or "."',
  'try:',
  '    fd, tmp = tempfile.mkstemp(dir=d, prefix=".dsh-edit-")',
  'except PermissionError:',
  '    sys.stderr.write("edit: permission denied")',
  '    sys.exit(9)',
  'try:',
  '    with os.fdopen(fd, "w", encoding="utf-8", newline="") as f:',
  '        f.write(after)',
  '    os.chmod(tmp, st.st_mode & 0o7777)',
  '    os.replace(tmp, rp)',
  'except PermissionError:',
  '    sys.stderr.write("edit: permission denied")',
  '    sys.exit(9)',
  'finally:',
  '    if os.path.exists(tmp):',
  '        os.remove(tmp)',
  'st2 = os.stat(rp)',
  'print(json.dumps({"version": "%d:%d" % (st2.st_mtime_ns, st2.st_size), "before": base64.b64encode(norm.encode()).decode(), "after": base64.b64encode(after_n.encode()).decode()}))',
].join('\n')

function b64(s) {
  return Buffer.from(String(s), 'utf8').toString('base64')
}

function b64d(s) {
  return Buffer.from(s, 'base64').toString('utf8')
}

function pyCommand(script, argsB64) {
  // Chaque argument est single-quote (un arg vide survit ainsi au join ssh/sh).
  return ['python3', '-c', '"$(printf %s ' + b64(script) + ' | base64 -d)"'].concat(argsB64.map(shQuote))
}

function findStatVersion(parts) {
  // parts = [type, size, T@] ; T@ = '1786702547.2402661480' (secondes décimales)
  // Les scripts python formatent la version en nanosecondes ENTIÈRES : '%d:%d' % (st_mtime_ns, st_size)
  const t = parts[2] || ''
  const dot = t.indexOf('.')
  const sec = dot === -1 ? t : t.slice(0, dot)
  const frac = (dot === -1 ? '' : t.slice(dot + 1)).padEnd(9, '0').slice(0, 9)
  try {
    return (BigInt(sec) * 1000000000n + BigInt(frac)).toString() + ':' + parts[1]
  } catch {
    return t + ':' + parts[1]
  }
}

function shQuote(s) {
  return "'" + String(s).replace(/'/g, "'\\''") + "'"
}

const SCHEMA_CONNECTION = z.object({
  id: z.string(),
  name: z.string().default(''),
  host: z.string(),
  user: z.string().default('root'),
  port: z.number().default(22),
  keyFile: z.string().default(''),
  baseDir: z.string().default(''),
})

const SCHEMA_CONFIG = z.object({
  connections: z.array(SCHEMA_CONNECTION).default([]),
  active: z.string().default(''),
  testRequest: z.number().default(0),
  testTarget: z.string().default(''),
})

const SCHEMA_STATUS = z.object({
  entries: z.array(z.object({
    id: z.string(),
    ok: z.boolean(),
    ms: z.number().default(0),
    error: z.string().default(''),
    checkedAt: z.number().default(0),
  })).default([]),
})

const MAX_STATE_BODY_BYTES = 256 * 1024

function isLoopbackHostname(hostname) {
  return hostname === 'localhost' || hostname === '127.0.0.1' || hostname === '::1'
}

function isLocalOrigin(value) {
  if (!value || value === 'null') return false
  try {
    return isLoopbackHostname(new URL(value).hostname)
  } catch {
    return false
  }
}

function validateStoreInput(connections, active, testTarget) {
  if (connections.length > 32) throw new Error('trop de connexions (maximum 32)')
  const ids = new Set()
  const endpoints = new Set()
  for (const connection of connections) {
    if (!connection.id || connection.id.length > 128 || /[\u0000-\u001f]/.test(connection.id)) throw new Error('id de connexion invalide')
    if (!connection.host || connection.host.length > 255 || /[\s/@\\\u0000]/.test(connection.host)) throw new Error('hôte invalide')
    if (!connection.user || connection.user.length > 128 || /[\s/@\\\u0000]/.test(connection.user)) throw new Error('utilisateur invalide')
    if (!Number.isInteger(connection.port) || connection.port < 1 || connection.port > 65535) throw new Error('port invalide')
    if (connection.keyFile.length > 4096 || connection.keyFile.includes('\u0000')) throw new Error('clé SSH invalide')
    if (connection.baseDir.length > 4096 || connection.baseDir.includes('\u0000') || (connection.baseDir && !connection.baseDir.startsWith('/'))) throw new Error('baseDir doit être absolu')
    if (connection.baseDir.split('/').includes('..')) throw new Error('baseDir ne doit pas contenir ..')
    if (ids.has(connection.id)) throw new Error('id de connexion dupliqué')
    const endpoint = connection.user + '@' + connection.host + ':' + connection.port
    if (endpoints.has(endpoint)) throw new Error('hôte et port dupliqués')
    ids.add(connection.id)
    endpoints.add(endpoint)
  }
  if (active && !ids.has(active)) throw new Error('connexion active inconnue')
  if (testTarget && !ids.has(testTarget)) throw new Error('cible de test inconnue')
}

/** SSH multiplexed connection pool, one ControlMaster per connection. */
class SshPool {
  constructor(defaults, transport) {
    this.defaults = defaults || {}
    this.transport = transport || null
    this.store = null
    this.masters = new Map()
    this.masterPromises = new Map()
  }

  attachStore(store) {
    this.store = store
  }

  current() {
    const defaults = this.defaults
    let conns = []
    let activeId = ''
    if (this.store) {
      if (Array.isArray(this.store.connections)) conns = this.store.connections
      if (this.store.active) activeId = this.store.active
    }
    if (!conns.length) {
      if (!defaults.host) return null
      return { id: 'default', name: 'default', host: defaults.host, user: defaults.user || 'root', port: defaults.port ?? 22, keyFile: defaults.keyFile || '', baseDir: defaults.baseDir || '' }
    }
    const c = conns.find((x) => x.id === activeId) || conns[0]
    return {
      id: c.id,
      name: c.name || c.host,
      host: c.host || defaults.host,
      user: c.user || defaults.user || 'root',
      port: c.port ?? defaults.port ?? 22,
      keyFile: c.keyFile || defaults.keyFile || '',
      baseDir: c.baseDir || defaults.baseDir || '',
    }
  }

  allConnections() {
    const list = []
    if (this.store) {
      if (Array.isArray(this.store.connections)) list.push(...this.store.connections)
    }
    if (!list.length) {
      const dflt = this.current()
      if (dflt !== null) list.push(dflt)
    }
    return list.map((c) => ({
      id: c.id || ('conn-' + c.host + '-' + (c.port ?? '')),
      name: c.name || c.host,
      host: c.host,
      user: c.user || this.defaults.user || 'root',
      port: c.port ?? this.defaults.port ?? 22,
      keyFile: c.keyFile || this.defaults.keyFile || '',
      baseDir: c.baseDir || this.defaults.baseDir || '',
    }))
  }

  sockPath(conn) {
    const key = conn.user + '@' + conn.host + ':' + (conn.port ?? 22)
    return join(tmpdir(), 'dsh-remote-vps-' + createHash('sha1').update(key).digest('hex').slice(0, 10) + '.ctl')
  }

  connArgs(conn) {
    const args = []
    if (conn.port) args.push('-p', String(conn.port))
    if (conn.keyFile) args.push('-i', conn.keyFile)
    return args
  }

  spawnOnce(argv, stdinData, graceMs, timeoutMs, binary) {
    return new Promise((resolve) => {
      const started = Date.now()
      let out = Buffer.alloc(0)
      let err = Buffer.alloc(0)
      let settled = false
      let timedOut = false
      let hardKiller = null
      const OUT_CAP = 8 * 1024 * 1024
      let proc
      try {
        proc = spawn(argv[0], argv.slice(1), { stdio: ['pipe', 'pipe', 'pipe'] })
      } catch (e) {
        resolve({ exit: -1, out: '', err: 'spawn failed: ' + e.message, ms: 0, timedOut: false })
        return
      }
      let killer = null
      if (timeoutMs && timeoutMs > 0) {
        killer = setTimeout(() => {
          timedOut = true
          try { proc.kill('SIGTERM') } catch (e) {}
          hardKiller = setTimeout(() => {
            try { proc.kill('SIGKILL') } catch (e) {}
          }, Math.max(1000, graceMs || 5000))
        }, timeoutMs)
      }
      proc.stdout.on('data', (d) => {
        if (out.length >= OUT_CAP) { try { proc.kill('SIGKILL') } catch (e) {} ; return }
        out = out.length + d.length <= OUT_CAP ? Buffer.concat([out, d]) : Buffer.concat([out, d]).subarray(0, OUT_CAP)
      })
      proc.stderr.on('data', (d) => { if (err.length < 65536) err = Buffer.concat([err, d]).subarray(0, 65536) })
      const finish = (exit, sig) => {
        if (settled) return
        settled = true
        if (killer) clearTimeout(killer)
        if (hardKiller) clearTimeout(hardKiller)
        resolve({ exit, signal: sig, out: binary ? out : out.toString('utf8'), err: err.toString('utf8'), ms: Date.now() - started, timedOut })
      }
      proc.on('error', (e) => finish(-1, null))
      proc.on('close', (code, sig) => finish(code ?? (sig ? -1 : 0), sig))
      proc.stdin.on('error', () => {})
      if (stdinData === undefined) proc.stdin.end()
      else proc.stdin.end(stdinData)
    })
  }

  ensureMaster(conn) {
    const sock = this.sockPath(conn)
    const existing = this.masterPromises.get(sock)
    if (existing) return existing
    const args = ['-MNf', '-S', sock, '-o', 'ControlPersist=900', '-o', 'BatchMode=yes', '-o', 'ConnectTimeout=15', '-o', 'StrictHostKeyChecking=accept-new'].concat(this.connArgs(conn), [conn.user + '@' + conn.host])
    const promise = this.spawnOnce([SSH].concat(args), undefined, 20000, 20000)
      .finally(() => this.masterPromises.delete(sock))
    this.masterPromises.set(sock, promise)
    return promise
  }

  async exec(conn, remoteArgv, stdinData, opts) {
    opts = opts || {}
    if (this.transport) {
      const cmdline = remoteArgv.join(' ')
      return await this.transport(cmdline, stdinData, opts)
    }
    const sock = this.sockPath(conn)
    const base = ['-S', sock, '-T', '-o', 'BatchMode=yes', '-o', 'ConnectTimeout=15'].concat(this.connArgs(conn), [conn.user + '@' + conn.host])
    let r = await this.spawnOnce([SSH].concat(base, remoteArgv), stdinData, opts.graceMs || 60000, opts.timeoutMs, opts.binary)
    if (r.exit === 255 || r.err.includes('Control socket connect')) {
      const master = await this.ensureMaster(conn)
      if (master.exit !== 0) return master
      r = await this.spawnOnce([SSH].concat(base, remoteArgv), stdinData, opts.graceMs || 60000, opts.timeoutMs, opts.binary)
    }
    return r
  }

  async py(conn, script, args, stdinData, opts) {
    return this.exec(conn, pyCommand(script, args.map(b64)), stdinData, opts)
  }

  async test(conn) {
    const r = await this.exec(conn, ['echo', 'ok'], undefined, { graceMs: 10000, timeoutMs: 12000 })
    return { ok: r.exit === 0 && r.out.trim() === 'ok', ms: r.ms, error: r.exit === 0 ? '' : (r.err.trim() || ('ssh exit ' + r.exit)) }
  }

  dispose() {
    for (const conn of this.allConnections()) {
      const sock = this.sockPath(conn)
      this.spawnOnce([SSH, '-S', sock, '-O', 'exit', conn.user + '@' + conn.host], undefined, 5000, 5000)
    }
  }
}

/** Remote FileSystem over SSH. Native tool-fs (read/write/edit/read_image) consumes this. */
class RemoteFileSystem extends FileSystem {
  static Config = z.object({
    defaultHost: z.string().default(''),
    defaultUser: z.string().default('root'),
    defaultPort: z.number().default(22),
    defaultKeyFile: z.string().default(''),
    defaultBaseDir: z.string().default(''),
  })

  static inject = ['sshPool']

  constructor(ctx, config) {
    super(ctx, 'fs')
    this.pool = ctx.sshPool
  }

  get sandboxMode() { return undefined }

  mapPath(p) {
    const conn = this.pool.current()
    if (conn === null) throw new FsError(NO_CONNECTION, 'FS_IO_ERROR')
    const base = (conn.baseDir || '').replace(/\/+$/, '')
    if (p.startsWith('/')) {
      if (p.startsWith('/home/')) return p
      if (p.startsWith('/Users/')) {
        const rest = p.split('/').slice(3).join('/')
        return base + (rest ? '/' + rest : '')
      }
      if (p.startsWith('/root/') || p.startsWith('/var/') || p.startsWith('/etc/') || p.startsWith('/tmp/') || p.startsWith('/opt/')) return p
      return base + p
    }
    return base + '/' + p
  }

  async resolve(path, opts) {
    const rp = this.mapPath(path)
    return { targetKey: FsTargetKey(rp), displayPath: rp }
  }

  conn() {
    const conn = this.pool.current()
    if (conn === null) throw new FsError(NO_CONNECTION, 'FS_IO_ERROR')
    return conn
  }

  processPath(target) { return target.targetKey }

  fileUrl(target) { return 'file://' + target.targetKey }

  contains(parent, child) {
    return child.targetKey === parent.targetKey || child.targetKey.startsWith(parent.targetKey.replace(/\/+$/, '') + '/')
  }

  async stat(target, signal) {
    const r = await this.pool.exec(this.conn(), ['find', '-L', shQuote(target.targetKey), '-maxdepth', '0', '-printf', shQuote('%y|%s|%T@')], undefined, { graceMs: 30000 })
    if (r.exit !== 0) return undefined
    const line = r.out.trim().split('\n')[0]
    if (!line) return undefined
    const parts = line.split('|')
    return {
      version: FsVersion(findStatVersion(parts)),
      type: parts[0] === 'd' ? 'directory' : parts[0] === 'f' ? 'file' : 'other',
      size: parseInt(parts[1], 10) || 0,
    }
  }

  async lstat(path, opts, signal) {
    const r = await this.pool.exec(this.conn(), ['find', shQuote(this.mapPath(path)), '-maxdepth', '0', '-printf', shQuote('%y|%s|%T@')], undefined, { graceMs: 30000 })
    if (r.exit !== 0) return undefined
    const line = r.out.trim().split('\n')[0]
    if (!line) return undefined
    const parts = line.split('|')
    return {
      version: FsVersion(findStatVersion(parts)),
      type: parts[0] === 'd' ? 'directory' : parts[0] === 'f' ? 'file' : parts[0] === 'l' ? 'symlink' : 'other',
      size: parseInt(parts[1], 10) || 0,
    }
  }

  async readText(target, signal) {
    const r = await this.pool.exec(this.conn(), ['cat', '--', shQuote(target.targetKey)], undefined, { graceMs: 60000, binary: true })
    if (r.exit !== 0) throw new FsError('not found: ' + target.displayPath, 'FS_NOT_FOUND')
    try {
      return new TextDecoder('utf-8', { fatal: true }).decode(r.out)
    } catch (e) {
      throw new FsError('not UTF-8 text: ' + target.displayPath, 'FS_NOT_TEXT')
    }
  }

  async streamText(target, signal) {
    const text = await this.readText(target, signal)
    const chunk = 64 * 1024
    return (async function* () {
      for (let i = 0; i < text.length; i += chunk) yield text.slice(i, i + chunk)
    })()
  }

  async readBytes(target, signal, maxBytes) {
    const info = await this.stat(target, signal)
    if (info === undefined) throw new FsError('not found: ' + target.displayPath, 'FS_NOT_FOUND')
    if (info.type !== 'file') throw new FsError('not a regular file: ' + target.displayPath, 'FS_NOT_REGULAR_FILE')
    if (info.size !== undefined && info.size > maxBytes) throw new FsError('file too large for byte read', 'FS_TOO_LARGE')
    const r = await this.pool.exec(this.conn(), ['base64', '-w0', shQuote(target.targetKey)], undefined, { graceMs: 60000 })
    if (r.exit !== 0) throw new FsError('read failed: ' + r.err.trim(), 'FS_IO_ERROR')
    return new Uint8Array(Buffer.from(r.out.replace(/\s+/g, ''), 'base64'))
  }

  async listDir(target, signal) {
    const script = 'find ' + shQuote(target.targetKey) + " -maxdepth 1 -mindepth 1 -printf '%y\\t%f\\t%s\\t%T@\\n' | sort -k2"
    const r = await this.pool.exec(this.conn(), ['bash', '-c', '"' + script + '"'], undefined, { graceMs: 30000 })
    if (r.exit !== 0) throw new FsError('list failed: ' + r.err.trim(), 'FS_IO_ERROR')
    const out = []
    for (const line of r.out.split('\n')) {
      if (!line.trim()) continue
      const parts = line.split('\t')
      out.push({
        name: parts[1],
        type: parts[0] === 'd' ? 'directory' : parts[0] === 'f' ? 'file' : 'other',
        target: { targetKey: FsTargetKey(target.targetKey.replace(/\/+$/, '') + '/' + parts[1]), displayPath: target.displayPath.replace(/\/+$/, '') + '/' + parts[1] },
        version: parts[3] ? FsVersion(findStatVersion(['', parts[2], parts[3]])) : undefined,
        size: parseInt(parts[2], 10) || 0,
      })
    }
    return out
  }

  async writeText(target, content, expected, signal, sandboxPolicy) {
    const mode = expected === undefined ? 'none' : (expected.kind === 'createIfAbsent' ? 'create' : 'update')
    const exp = expected !== undefined && expected.kind === 'replaceIfVersion' ? String(expected.version) : ''
    const r = await this.pool.py(this.conn(), WRITE_SCRIPT, [target.targetKey, mode, exp], content, { graceMs: 60000 })
    if (r.exit === 6) throw new FsError('target already exists: ' + target.displayPath, 'FS_NOT_OBSERVED')
    if (r.exit === 7) throw new FsError('stale version on write: ' + target.displayPath, 'FS_STALE_VERSION')
    if (r.exit === 8) throw new FsError('write failed: is a directory: ' + target.displayPath, 'FS_NOT_REGULAR_FILE')
    if (r.exit === 9) throw new FsError('write failed: permission denied: ' + target.displayPath, 'FS_PERMISSION_DENIED')
    if (r.exit !== 0) throw new FsError('write failed: ' + r.err.trim(), 'FS_IO_ERROR')
    const j = JSON.parse(r.out)
    return {
      operation: j.op,
      version: FsVersion(j.version),
      before: j.before !== undefined ? Buffer.from(j.before, 'base64').toString('utf8') : null,
      after: content,
    }
  }

  async editText(target, edit, expected, signal, sandboxPolicy) {
    const exp = expected !== undefined ? String(expected.version) : ''
    const r = await this.pool.py(this.conn(), EDIT_SCRIPT, [target.targetKey, edit.oldString, edit.newString, edit.replaceAll ? '1' : '0', exp], undefined, { graceMs: 60000 })
    if (r.exit === 2) throw new FsError('not found: ' + target.displayPath, 'FS_NOT_FOUND')
    if (r.exit === 3) throw new FsError('not UTF-8 text: ' + target.displayPath, 'FS_NOT_TEXT')
    if (r.exit === 8) throw new FsError('edit failed: is a directory: ' + target.displayPath, 'FS_NOT_REGULAR_FILE')
    if (r.exit === 9) throw new FsError('edit failed: permission denied: ' + target.displayPath, 'FS_PERMISSION_DENIED')
    if (r.exit === 4) throw new FsError('old_string not found in ' + target.displayPath, 'FS_EDIT_NOT_FOUND')
    if (r.exit === 5) throw new FsError('old_string appears more than once in ' + target.displayPath, 'FS_AMBIGUOUS_EDIT')
    if (r.exit === 7) throw new FsError('stale version on edit: ' + target.displayPath, 'FS_STALE_VERSION')
    if (r.exit !== 0) throw new FsError('edit failed: ' + r.err.trim(), 'FS_IO_ERROR')
    const j = JSON.parse(r.out)
    return {
      version: FsVersion(j.version),
      before: Buffer.from(j.before, 'base64').toString('utf8'),
      after: Buffer.from(j.after, 'base64').toString('utf8'),
    }
  }
}

export const Config = z.object({
  defaultHost: z.string().default(''),
  defaultUser: z.string().default('root'),
  defaultPort: z.number().default(22),
  defaultKeyFile: z.string().default(''),
  defaultBaseDir: z.string().default(''),
})

/**
 * Root (host-plane) plugin: owns the SSH pool, the settings namespaces and the
 * model-facing glob/grep/bash tools. It does NOT provide `fs` — the remote
 * filesystem belongs to the preset realm via the `./fs` subpath export.
 */
export function apply(ctx, config) {
  const pool = new SshPool({
    host: config.defaultHost,
    user: config.defaultUser,
    port: config.defaultPort,
    keyFile: config.defaultKeyFile,
    baseDir: config.defaultBaseDir,
  })

  ctx.provide('sshPool', pool)

  // ── store persistant (fichier JSON — le canal settings de DSH n'expose pas
  //    les namespaces tiers aux clients navigateur) ─────────────────────────
  const storePath = join(process.env.DSH_HOME || join(homedir(), '.dsh'), 'dsh-remote-vps.json')
  const emptyStore = { connections: [], active: '', testTarget: '', statuses: [] }
  let store
  try {
    const raw = JSON.parse(readFileSync(storePath, 'utf8'))
    store = {
      connections: Array.isArray(raw.connections) ? raw.connections : [],
      active: typeof raw.active === 'string' ? raw.active : '',
      testTarget: typeof raw.testTarget === 'string' ? raw.testTarget : '',
      statuses: Array.isArray(raw.statuses) ? raw.statuses : [],
    }
  } catch {
    store = Object.assign({}, emptyStore)
  }
  try {
    validateStoreInput(store.connections, store.active, store.testTarget)
  } catch {
    store = Object.assign({}, emptyStore)
  }
  function saveStore() {
    try {
      mkdirSync(dirname(storePath), { recursive: true, mode: 0o700 })
      const tmp = storePath + '.tmp'
      writeFileSync(tmp, JSON.stringify(store, null, 2), { mode: 0o600 })
      chmodSync(tmp, 0o600)
      renameSync(tmp, storePath)
    } catch (e) {
      console.error('[dsh-remote-vps] save store failed:', e)
    }
  }
  pool.attachStore(store)

  // ── health checker ─────────────────────────────────────────────────────
  let healthBusy = false
  async function runHealthCheck(targetId) {
    if (healthBusy) return
    healthBusy = true
    try {
      const entries = []
      const prevEntries = Array.isArray(store.statuses) ? store.statuses : []
      const targets = targetId
        ? pool.allConnections().filter((c) => c && c.id === targetId)
        : pool.allConnections()
      for (const c of targets) {
        if (c === null || c === undefined) continue
        const r = await pool.test(c)
        entries.push({ id: c.id, ok: r.ok, ms: r.ms, error: r.error, checkedAt: Date.now() })
      }
      // Conserve les statuts des connexions non testées (test ciblé)
      if (targetId) {
        for (const e of prevEntries) {
          if (!entries.some((x) => x.id === e.id)) entries.push(e)
        }
      }
      store.statuses = entries
      saveStore()
    } catch (e) {
      console.error('[dsh-remote-vps] health check failed:', e)
    } finally {
      healthBusy = false
    }
  }

  // ── pont HTTP local pour la section Settings (loopback uniquement) ──────
  const route = ctx.webServer.register({
    kind: 'prefix',
    path: '/dsh-remote-vps',
    handler: async (req, res) => {
      const send = (code, obj) => {
        res.statusCode = code
        res.setHeader('Content-Type', 'application/json; charset=utf-8')
        res.end(JSON.stringify(obj))
      }
      const addr = req.socket ? String(req.socket.remoteAddress || '') : ''
      if (addr !== '127.0.0.1' && addr !== '::1' && addr !== '::ffff:127.0.0.1') return send(403, { ok: false, message: 'loopback uniquement' })
      const host = String(req.headers?.host || '')
      if (host) {
        try {
          if (!isLoopbackHostname(new URL('http://' + host).hostname)) return send(403, { ok: false, message: 'hôte local uniquement' })
        } catch {
          return send(400, { ok: false, message: 'en-tête Host invalide' })
        }
      }
      const origin = req.headers?.origin || req.headers?.Origin
      if (origin && !isLocalOrigin(String(origin))) return send(403, { ok: false, message: 'origine locale uniquement' })
      if (String(req.headers?.['sec-fetch-site'] || '').toLowerCase() === 'cross-site') return send(403, { ok: false, message: 'requête cross-site refusée' })
      const url = new URL(req.url, 'http://localhost')
      if (url.pathname === '/dsh-remote-vps/state') {
        if (req.method === 'GET') {
          return send(200, { ok: true, connections: store.connections, active: store.active, statuses: store.statuses })
        }
        if (req.method === 'POST') {
          let body = ''
          let bodyBytes = 0
          for await (const chunk of req) {
            bodyBytes += Buffer.byteLength(chunk)
            if (bodyBytes > MAX_STATE_BODY_BYTES) return send(413, { ok: false, message: 'corps trop volumineux' })
            body += chunk
          }
          try {
            const input = JSON.parse(body)
            const conns = (Array.isArray(input.connections) ? input.connections : []).map((c) => SCHEMA_CONNECTION(c))
            const validated = SCHEMA_CONFIG({
              connections: conns,
              active: typeof input.active === 'string' ? input.active : store.active,
              testRequest: 0,
              testTarget: typeof input.testTarget === 'string' ? input.testTarget : '',
            })
            validateStoreInput(validated.connections, validated.active, validated.testTarget)
            store.connections = validated.connections
            store.active = validated.active
            store.testTarget = validated.testTarget
            saveStore()
            runHealthCheck(validated.testTarget)
            return send(200, { ok: true, connections: store.connections, active: store.active, statuses: store.statuses })
          } catch (e) {
            return send(400, { ok: false, message: e && e.message ? e.message : String(e) })
          }
        }
      }
      return send(404, { ok: false, message: 'not found' })
    },
  })
  ctx.effect(() => route)

  // Pré-établit le ControlMaster dès le démarrage (parallèle au health check)
  const initial = pool.current()
  if (initial !== null) pool.ensureMaster(initial).catch(() => {})
  const interval = ctx.interval(runHealthCheck, 30000)
  runHealthCheck()

  ctx.effect(() => () => {
    interval()
    pool.dispose()
  })

  // ── remote tools: glob / grep / bash ───────────────────────────────────
  function register(toolName, description, params, execute) {
    const tool = defineTool({
      name: toolName,
      description,
      parameters: params,
      output: {
        schema: { type: 'string' },
        render(_a, v) { return [{ type: 'text', text: v }] },
      },
      async execute(args) {
        try {
          return await execute(args)
        } catch (err) {
          return 'Error: ' + (err && err.message ? err.message : String(err))
        }
      },
    })
    const dispose = ctx.tools.register(tool)
    ctx.effect(() => dispose)
  }

  register('glob', 'Trouver des fichiers sur un VPS distant par motif glob. Pattern sans "/" matche le basename a toute profondeur. Chemins relatifs a baseDir du VPS. Exclut node_modules/.git/.next/dist.', {
    pattern: { type: 'string', required: true, description: 'Motif glob ; sans / il matche le basename a toute profondeur' },
    path: { type: 'string', description: 'Repertoire de depart sur le VPS (defaut baseDir)' },
  }, async (a) => {
    const conn = pool.current()
    if (conn === null) return NO_CONNECTION
    const base = a.path ? (a.path.startsWith('/') ? a.path : conn.baseDir + '/' + a.path) : conn.baseDir
    const argv = ['rg', '--files', '--hidden', '-g', '!node_modules', '-g', '!.git', '-g', '!.next', '-g', '!dist', '-g', '!playwright-report', '-g', '!test-results', '-g', '!backups', '-g', '!build', '-g', shQuote(a.pattern), '--', shQuote(base), '|', 'sort', '|', 'head', '-100']
    const r = await pool.exec(conn, argv, undefined, { graceMs: 30000 })
    if (r.exit !== 0) return 'Error: ' + (r.err.trim() || 'glob failed') + ' [exit code: ' + r.exit + ']'
    return r.out.trim() || 'No matches.'
  })

  register('grep', 'Rechercher un motif (ripgrep) dans le code sur un VPS distant, hors node_modules/.git/.next/dist. Chemins relatifs a baseDir du VPS.', {
    pattern: { type: 'string', required: true, description: 'Expression reguliere ripgrep' },
    path: { type: 'string', description: 'Fichier ou repertoire sur le VPS (defaut baseDir)' },
    include: { type: 'string', description: 'Glob de fichiers a inclure (ex: *.ts)' },
  }, async (a) => {
    const conn = pool.current()
    if (conn === null) return NO_CONNECTION
    const argv = ['rg', '-n', '--no-heading', '--color', 'never', '--hidden',
      '-g', '!node_modules', '-g', '!.git', '-g', '!.next', '-g', '!dist',
      '-g', '!playwright-report', '-g', '!test-results', '-g', '!backups',
      '-g', '!*.map', '-f', '-']
    if (a.include) argv.push('-g', shQuote(a.include))
    if (a.path) argv.push(shQuote(a.path.startsWith('/') ? a.path : conn.baseDir + '/' + a.path))
    argv.push('|', 'head', '-300')
    const r = await pool.exec(conn, argv, a.pattern, { graceMs: 60000 })
    if (r.exit === 0) return r.out || 'No matches.'
    return 'Error: ' + (r.err.trim() || 'rg failed') + ' [exit code: ' + r.exit + ']'
  })

  register('bash', 'Executer une commande shell sur un VPS distant. PATH inclut le node le plus recent via nvm si present. workdir par defaut = baseDir du VPS. ATTENTION : cet outil est le shell du VPS, pas du poste local.', {
    command: { type: 'string', required: true, description: 'Commande a executer sur le VPS' },
    workdir: { type: 'string', description: 'Repertoire de travail distant (defaut baseDir)' },
    timeout_ms: { type: 'integer', description: 'Timeout en ms (defaut 300000)' },
  }, async (a) => {
    const conn = pool.current()
    if (conn === null) return NO_CONNECTION
    const wd = a.workdir ? (a.workdir.startsWith('/') ? a.workdir : conn.baseDir + '/' + a.workdir) : conn.baseDir
    // Node via nvm : on prend la version la plus récente installée, sans version codée en dur.
    const nodeSetup = 'NODE_BIN=$(ls -d "$HOME/.nvm/versions/node/v"* 2>/dev/null | sort -V | tail -1)/bin\n[ -n "$NODE_BIN" ] && export PATH="$NODE_BIN:$PATH"\ntrue\n'
    const script = nodeSetup + 'cd ' + shQuote(wd) + ' 2>/dev/null || cd ' + shQuote(conn.baseDir) + '\n' + a.command + '\n'
    const r = await pool.exec(conn, ['bash', '-s'], script, { graceMs: 300000, timeoutMs: a.timeout_ms || 300000 })
    let out = r.out
    if (r.err && r.err.trim()) out += (out ? '\n' : '') + '[stderr]\n' + r.err
    if (r.timedOut) out += (out ? '\n' : '') + '[timed out]'
    if (r.exit !== 0) out += (out ? '\n' : '') + '[exit code: ' + r.exit + ']'
    return out
  })
}

export const SCRIPTS = { WRITE: WRITE_SCRIPT, EDIT: EDIT_SCRIPT }
export { SCHEMA_CONFIG, SCHEMA_CONNECTION, SCHEMA_STATUS }

export { RemoteFileSystem, SshPool, name, inject }
