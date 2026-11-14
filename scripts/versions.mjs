// The launcher's version lives in four files; these keep them the same.
//   node scripts/versions.mjs --check v0.3.0   (CI: the tag must match every file)
import { readFileSync, writeFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { dirname, join } from 'node:path'

export const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..')
const p = (f) => join(ROOT, f)

const FILES = {
  'package.json': {
    get: (t) => JSON.parse(t).version,
    set: (t, v) => t.replace(/^(\s*"version":\s*")[^"]+(")/m, `$1${v}$2`),
  },
  'package-lock.json': {
    get: (t) => JSON.parse(t).version,
    // The root entry and the "" package entry.
    set: (t, v) => t.replace(/^(  "version":\s*")[^"]+(")/m, `$1${v}$2`).replace(/("packages":\s*\{\s*"":\s*\{[^}]*?"version":\s*")[^"]+(")/, `$1${v}$2`),
  },
  'src-tauri/Cargo.toml': {
    get: (t) => /^\[package\][\s\S]*?^version\s*=\s*"([^"]+)"/m.exec(t)?.[1],
    set: (t, v) => t.replace(/^(\[package\][\s\S]*?^version\s*=\s*")[^"]+(")/m, `$1${v}$2`),
  },
  'src-tauri/tauri.conf.json': {
    get: (t) => JSON.parse(t).version,
    set: (t, v) => t.replace(/^(  "version":\s*")[^"]+(")/m, `$1${v}$2`),
  },
  'src-tauri/Cargo.lock': {
    get: (t) => /name = "tirgumim"\nversion = "([^"]+)"/.exec(t)?.[1],
    set: (t, v) => t.replace(/(name = "tirgumim"\nversion = ")[^"]+(")/, `$1${v}$2`),
  },
}

export function versions() {
  return Object.fromEntries(Object.entries(FILES).map(([f, x]) => [f, x.get(readFileSync(p(f), 'utf8').replace(/\r\n/g, '\n'))]))
}

export function setVersion(v) {
  for (const [f, x] of Object.entries(FILES)) {
    const raw = readFileSync(p(f), 'utf8')
    const crlf = raw.includes('\r\n')
    const next = x.set(raw.replace(/\r\n/g, '\n'), v)
    if (x.get(next) !== v) throw new Error(`Could not set the version in ${f}`)
    writeFileSync(p(f), crlf ? next.replace(/\n/g, '\r\n') : next)
  }
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1] && process.argv[2] === '--check') {
  const want = (process.argv[3] || '').replace(/^v/, '')
  const found = versions()
  const bad = Object.entries(found).filter(([, v]) => v !== want)
  for (const [f, v] of Object.entries(found)) console.log(`${f}: ${v}`)
  if (!want || bad.length) {
    console.error(`::error::The tag says ${want || '(none)'}, but ${bad.map(([f, v]) => `${f} says ${v}`).join(', ')}. Use npm run publish-launcher.`)
    process.exit(1)
  }
  console.log(`All files are at ${want}.`)
}
