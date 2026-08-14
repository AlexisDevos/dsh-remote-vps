// Suite de tests du backend dsh-remote-vps — exécute la glue réelle
// (scripts python write/edit + commandes coreutils stat/read/list/glob)
// localement, via le transport injectable du pool. Usage : npm test
import { SshPool, SCRIPTS } from '../src/index.js'
import { spawnSync } from 'node:child_process'
import { mkdtempSync, writeFileSync, mkdirSync, symlinkSync, chmodSync, readFileSync, statSync, lstatSync, rmSync, existsSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

let passed = 0
let failed = 0
const failures = []

function check(name, cond, detail) {
  if (cond) passed += 1
  else { failed += 1; failures.push(name + (detail ? ' → ' + detail : '')) }
  console.log((cond ? '  ✓' : '  ✗') + ' ' + name)
}

// Transport local : exécute la ligne de commande construite par le pool via bash.
function localTransport(cmdline, stdinData) {
  const r = spawnSync('/bin/bash', ['-c', cmdline], { input: stdinData, encoding: 'utf8', maxBuffer: 64 * 1024 * 1024 })
  return { exit: r.status, out: r.stdout || '', err: r.stderr || '', ms: 0 }
}

const pool = new SshPool({}, localTransport)
const conn = { user: 'test', host: 'local', port: 22 }

const root = mkdtempSync(join(tmpdir(), 'dsh-remote-vps-test-'))
const shq = (s) => "'" + String(s).replace(/'/g, "'\\''") + "'"

function p(rel) { return join(root, rel) }

async function py(script, args, stdin) {
  return await pool.py(conn, script, args, stdin, { graceMs: 15000 })
}

async function exec(argv, stdin) {
  return await pool.exec(conn, argv, stdin, { graceMs: 15000 })
}

async function statVer(rel) {
  const r = await exec(['find', '-L', shq(p(rel)), '-maxdepth', '0', '-printf', shq('%y|%s|%T@')])
  const parts = r.out.trim().split('|')
  return parts[2] + ':' + parts[1]
}

// ── fixtures ────────────────────────────────────────────────────────────────
mkdirSync(p('w'), { recursive: true })
mkdirSync(p('e'), { recursive: true })
mkdirSync(p('r'), { recursive: true })
mkdirSync(p('ld'), { recursive: true })
mkdirSync(p('p'), { recursive: true })

const results = []
results.push(await py(SCRIPTS.WRITE, [p('r/t.txt'), 'create', ''], 'ligne 1\nligne 2\nligne 3\n'))
results.push(await py(SCRIPTS.WRITE, [p('w/a.txt'), 'create', ''], 'alpha beta\ngamma\n'))
results.push(await py(SCRIPTS.WRITE, [p('e/a.txt'), 'create', ''], 'alpha beta\ngamma\n'))
results.push(await py(SCRIPTS.WRITE, [p('e/b.txt'), 'create', ''], 'alpha alpha\nbeta\nalpha\n'))
results.push(await py(SCRIPTS.WRITE, [p('p/a.txt'), 'create', ''], 'mode test\n'))
chmodSync(p('p/a.txt'), 0o755)
results.push(await py(SCRIPTS.WRITE, [p('ld/bbb.txt'), 'create', ''], 'bbb\n'))
results.push(await py(SCRIPTS.WRITE, [p('ld/aaa.txt'), 'create', ''], 'aaa\n'))
writeFileSync(p('e/target.txt'), 'symlink target\n')
symlinkSync(p('e/target.txt'), p('e/link.txt'))

const hasRg = spawnSync('rg', ['--version']).status === 0
const hasFindPrintf = spawnSync('bash', ['-c', "find . -maxdepth 0 -printf '%y' 2>/dev/null || true"]).stdout.includes('f')
const bsdBase64 = String(spawnSync('bash', ['-c', "printf hi > /tmp/.b64probe 2>/dev/null; base64 -i /tmp/.b64probe >/dev/null 2>&1; echo $?"]).stdout || '').trim() === '0'

console.log('write — création avec création de répertoire parent')
check('exit 0 + version', results[0].exit === 0 && /"op": "create"/.test(results[0].out))

console.log('write — gardes de version')
{
  const r = await py(SCRIPTS.WRITE, [p('r/t.txt'), 'create', ''], 'x')
  check('createIfAbsent sur existant → exit 6', r.exit === 6)
  const v = JSON.parse(results[0].out).version
  const ok = await py(SCRIPTS.WRITE, [p('r/t.txt'), 'update', v], 'ligne 1 MODIF\n')
  check('update avec version correcte → exit 0', ok.exit === 0)
  const stale = await py(SCRIPTS.WRITE, [p('r/t.txt'), 'update', '1:1'], 'x')
  check('update avec version périmée → exit 7', stale.exit === 7)
}

console.log('write — round-trip contenu')
{
  await py(SCRIPTS.WRITE, [p('r/unicode.txt'), 'create', ''], 'café ☕ — accents & émoji\n')
  const cat = await exec(['cat', '--', shq(p('r/unicode.txt'))])
  check('Unicode intact après write+cat', cat.exit === 0 && cat.out.includes('café ☕'))
}

console.log('write — fichier vide')
{
  const r = await py(SCRIPTS.WRITE, [p('r/empty.txt'), 'create', ''], '')
  check('write fichier vide → exit 0', r.exit === 0)
  const cat = await exec(['cat', '--', shq(p('r/empty.txt'))])
  check('cat fichier vide → sortie vide', cat.exit === 0 && cat.out === '')
}

console.log('edit — sémantique littérale')
{
  const ok = await py(SCRIPTS.EDIT, [p('e/a.txt'), 'alpha', 'ALPHA', '0', ''])
  check('edit simple → exit 0', ok.exit === 0)
  const cat = await exec(['cat', '--', shq(p('e/a.txt'))])
  check('une seule occurrence remplacée', cat.out === 'ALPHA beta\ngamma\n')

  const ra = await py(SCRIPTS.EDIT, [p('e/b.txt'), 'alpha', 'A', '1', ''])
  check('edit replaceAll → exit 0', ra.exit === 0)
  const cat2 = await exec(['cat', '--', shq(p('e/b.txt'))])
  check('toutes les occurrences remplacées', cat2.out === 'A A\nbeta\nA\n')

  const nf = await py(SCRIPTS.EDIT, [p('e/a.txt'), 'introuvable', 'x', '0', ''])
  check('old_string introuvable → exit 4', nf.exit === 4)

  const amb = await py(SCRIPTS.EDIT, [p('e/b.txt'), 'A', 'x', '0', ''])
  check('ambigu sans replaceAll → exit 5', amb.exit === 5)
}

console.log('edit — robustesse symlink & CRLF')
{
  const ok = await py(SCRIPTS.EDIT, [p('e/link.txt'), 'symlink', 'SYMLINK', '0', ''])
  check('edit via symlink → exit 0', ok.exit === 0)
  const lst = lstatSync(p('e/link.txt'))
  check('le lien survit', lst.isSymbolicLink())

  await py(SCRIPTS.WRITE, [p('e/crlf.txt'), 'create', ''], 'ligne1\r\nligne2\r\n')
  const ce = await py(SCRIPTS.EDIT, [p('e/crlf.txt'), 'ligne1', 'LIGNE1', '0', ''])
  check('edit CRLF → exit 0', ce.exit === 0)
  const raw = readFileSync(p('e/crlf.txt'), 'utf8')
  check('CRLF préservé après édition', raw.includes('\r\n'))
}

console.log('stat/read/list/glob — commandes coreutils')
{
  if (hasFindPrintf) {
    const st = await exec(['find', '-L', shq(p('r/t.txt')), '-maxdepth', '0', '-printf', shq('%y|%s|%T@')])
    check('stat → f|size|T@', st.exit === 0 && /^f\|\d+\|\d+(\.\d+)?$/.test(st.out.trim()))
    const missing = await exec(['find', shq(p('r/absent.txt')), '-maxdepth', '0', '-printf', shq('%y|%s|%T@')])
    check('stat absent → exit non-0', missing.exit !== 0)
  } else {
    console.log('  - find -printf indisponible localement (GNU uniquement) : tests stat/list/glob-printf sautés (validés en live sur le VPS Ubuntu)')
  }

  const cat = await exec(['cat', '--', shq(p('r/t.txt'))])
  check('cat → contenu exact', cat.exit === 0 && cat.out === 'ligne 1 MODIF\n')

  const b64argv = bsdBase64 ? ['base64', '-i', shq(p('r/unicode.txt'))] : ['base64', '-w0', shq(p('r/unicode.txt'))]
  const b64 = await exec(b64argv)
  check('base64 → décodable', b64.exit === 0 && Buffer.from(b64.out.replace(/\s+/g, ''), 'base64').toString('utf8').includes('café'), 'exit=' + b64.exit + ' err=' + JSON.stringify(b64.err.slice(0,60)) + ' out=' + JSON.stringify(b64.out.slice(0,40)))

  if (hasFindPrintf) {
    const ls = await exec(['bash', '-c', '"' + 'find ' + shq(p('ld')) + " -maxdepth 1 -mindepth 1 -printf '%y\\t%f\\t%s\\t%T@\\n' | sort -k2" + '"'])
    const names = ls.out.split('\n').filter(Boolean).map((l) => l.split('\t')[1])
    check('list → trié par nom', ls.exit === 0 && names.join(',') === 'aaa.txt,bbb.txt')
  }

  if (hasRg) {
    const g = await exec(['rg', '--files', '--hidden', '-g', '!node_modules', '-g', shq('*.txt'), '--', shq(p('r')), '|', 'sort'])
    check('glob → fichiers .txt', g.exit === 0 && g.out.trim().split('\n').length === 3)
  } else {
    console.log('  - rg absent localement : test glob sauté')
  }
}

console.log('permissions')
{
  const before = statSync(p('p/a.txt')).mode & 0o777
  const ok = await py(SCRIPTS.EDIT, [p('p/a.txt'), 'mode', 'MODE', '0', ''])
  const after = statSync(p('p/a.txt')).mode & 0o777
  check('mode 755 préservé après edit', ok.exit === 0 && before === 0o755 && after === 0o755)
}

console.log('before/after du edit')
{
  await py(SCRIPTS.WRITE, [p('e/ba.txt'), 'create', ''], 'une\ndeux\n')
  const r = await py(SCRIPTS.EDIT, [p('e/ba.txt'), 'deux', 'DEUX', '0', ''])
  const j = JSON.parse(r.out)
  const before = Buffer.from(j.before, 'base64').toString('utf8')
  const after = Buffer.from(j.after, 'base64').toString('utf8')
  check('before/after cohérents', r.exit === 0 && before === 'une\ndeux\n' && after === 'une\nDEUX\n')
}

rmSync(root, { recursive: true, force: true })

console.log('──────────────────────────────────────────')
console.log('Résultat : ' + passed + ' OK / ' + failed + ' KO')
if (failed > 0) {
  console.log('Échecs :')
  for (const f of failures) console.log('  - ' + f)
  process.exit(1)
}
