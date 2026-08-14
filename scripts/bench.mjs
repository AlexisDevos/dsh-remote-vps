// Mesure la latence des opérations distantes (multiplexées) contre un VPS.
// Utilise la connexion active du store (~/.dsh/dsh-remote-vps.json), ou :
//   node scripts/bench.mjs <host> <user> <port> <baseDir>
import { SshPool } from '../src/index.js'
import { readFileSync } from 'node:fs'
import { homedir } from 'node:os'
import { join } from 'node:path'

let host, user, port, base
const storePath = join(process.env.DSH_HOME || join(homedir(), '.dsh'), 'dsh-remote-vps.json')
try {
  const raw = JSON.parse(readFileSync(storePath, 'utf8'))
  const conns = Array.isArray(raw.connections) ? raw.connections : []
  const active = conns.find((c) => c.id === raw.active) || conns[0]
  if (active && active.host) {
    host = active.host
    user = active.user || 'root'
    port = active.port ?? 22
    base = active.baseDir || ''
  }
} catch {}
if (process.argv[2]) host = process.argv[2]
if (process.argv[3]) user = process.argv[3]
if (process.argv[4]) port = parseInt(process.argv[4], 10)
if (process.argv[5]) base = process.argv[5]
if (!host || !base) {
  console.error('Aucune connexion disponible : configure un VPS dans Settings → « VPS distants » ou passe les arguments : node scripts/bench.mjs <host> <user> <port> <baseDir>')
  process.exit(1)
}

const pool = new SshPool({ host, user, port, baseDir: base })
const conn = pool.current()
const shq = (s) => "'" + String(s).replace(/'/g, "'\\''") + "'"

const target = base + '/package.json'
const cases = [
  ['stat ', ['find', '-L', shq(target), '-maxdepth', '0', '-printf', shq('%y|%s|%T@')]],
  ['read ', ['cat', '--', shq(target)]],
  ['glob ', ['rg', '--files', '--hidden', '-g', '!node_modules', '-g', 'package.json', '--', shq(base), '|', 'head', '-5']],
  ['write', ['python3', '-c', '"$(printf %s ' + Buffer.from("import sys,base64,os,tempfile\np=base64.b64decode(sys.argv[1]).decode()\ndata=sys.stdin.buffer.read()\nd=os.path.dirname(p) or '.'\nfd,tmp=tempfile.mkstemp(dir=d,prefix='.bench-')\ntry:\n with open(fd,'wb') as f: f.write(data)\n os.replace(tmp,p)\nfinally:\n if os.path.exists(tmp): os.remove(tmp)\nprint('ok')\n").toString('base64') + ' | base64 -d)"', shq(Buffer.from(base + '/tmp/.bench-target.txt').toString('base64'))], 'bench\n'],
]

await pool.test(conn)

for (const [label, argv, stdin] of cases) {
  const times = []
  for (let i = 0; i < 3; i++) {
    const r = await pool.exec(conn, argv, stdin, { graceMs: 20000 })
    times.push(r.ms)
  }
  times.sort((a, b) => a - b)
  console.log(label + ' médiane: ' + times[1] + ' ms (min ' + times[0] + ', max ' + times[2] + ')')
}

await pool.exec(conn, ['rm', '-f', base + '/tmp/.bench-target.txt'])
pool.dispose()
console.log('Fait.')
