// The launcher must never need an update for a new game: everything game-specific lives in the catalog and the
// packs. This fails if a catalog game's id, name or Steam id shows up in the app's code (tests excluded).
//   node scripts/no-game-code.mjs
import { readFileSync, readdirSync, statSync } from 'node:fs'
import { join, relative } from 'node:path'
import { ROOT } from './versions.mjs'

const catalog = JSON.parse(readFileSync(join(ROOT, 'catalog/catalog.json'), 'utf8').replace(/^﻿/, ''))
const needles = new Set()
for (const g of catalog.games ?? []) {
  for (const v of [g.id, g.name, g.detect?.steamAppId, g.detect?.epicAppName, g.detect?.markerFile, ...(g.detect?.processNames ?? [])]) {
    if (typeof v === 'string' && v.length >= 4) needles.add(v.toLowerCase())
  }
}

function* files(dir) {
  for (const name of readdirSync(dir)) {
    const p = join(dir, name)
    if (statSync(p).isDirectory()) yield* files(p)
    else if (/\.(rs|ts|tsx)$/.test(name) && name !== 'tests.rs') yield p
  }
}

const hits = []
for (const dir of ['src', 'src-tauri/src']) {
  for (const f of files(join(ROOT, dir))) {
    let text = readFileSync(f, 'utf8')
    const cut = text.indexOf('#[cfg(test)]')
    if (cut >= 0) text = text.slice(0, cut)
    const low = text.toLowerCase()
    for (const n of needles) if (low.includes(n)) hits.push(`${relative(ROOT, f)}: "${n}"`)
  }
}
if (hits.length) {
  console.error(`::error::Game-specific code found. Put it in the catalog or the pack instead:\n${hits.join('\n')}`)
  process.exit(1)
}
console.log(`No game-specific code (${needles.size} catalog values checked).`)
