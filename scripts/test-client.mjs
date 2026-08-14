// Vérifie que le bundle client charge, applique et enregistre la section
// « VPS distants » dans le slot settings.section — données via le pont HTTP.
import { readFileSync } from 'node:fs'
import React from '/Users/alexis/.npm/_npx/1e7f6d9597241db0/node_modules/react/index.js'

let def
global.window = { __ModuleLoader__: { load(d) { def = d } } }
global.document = {
  getElementById: () => null,
  createElement: () => ({ id: '', textContent: '', appendChild() {} }),
  head: { appendChild() {} },
}
global.fetch = async () => ({ json: async () => ({ ok: true, connections: [], active: '', statuses: [] }) })

const src = readFileSync(new URL('../lib/client.js', import.meta.url), 'utf8')
new Function('window', src)(global.window)

const exportsObj = def.factory((id) => {
  if (id === 'react') return React
  throw new Error('unknown require: ' + id)
})

if (exportsObj.name !== 'dsh-remote-vps-ui') throw new Error('name inattendu: ' + exportsObj.name)

let captured = null
const ctx = {
  get(name) {
    if (name === 'slots') return {
      inject(slotName, cb) { if (slotName === 'settings.section') captured = cb() },
      register(opts, comp) { return { opts, comp } },
    }
    return undefined
  },
}

exportsObj.apply(ctx)
if (!captured) throw new Error('registration jamais appelée')
if (captured.opts.id !== 'dsh-remote-vps') throw new Error('id de section inattendu: ' + captured.opts.id)
if (captured.opts.label !== 'VPS distants') throw new Error('label inattendu: ' + captured.opts.label)
if (typeof captured.comp !== 'function') throw new Error('composant manquant')

console.log('✓ bundle charge et enregistre la section « ' + captured.opts.label + ' » (id ' + captured.opts.id + ')')
