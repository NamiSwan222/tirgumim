// Publishes a new launcher version:
//   npm run publish-launcher -- 0.3.0 "מה חדש בגרסה הזו"
// Sets the version in every file, runs the tests, commits, tags v0.3.0 (the tag message becomes the release notes
// players see in the update window) and pushes. GitHub Actions (.github/workflows/release.yml) does the rest.
import { execFileSync } from 'node:child_process'
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { ROOT, setVersion, versions } from './versions.mjs'

const [version, ...noteWords] = process.argv.slice(2)
const notes = noteWords.join(' ').trim()
const die = (m) => { console.error(m); process.exit(1) }
const run = (cmd, args, opts = {}) => execFileSync(cmd, args, { cwd: ROOT, stdio: 'inherit', shell: process.platform === 'win32' && cmd === 'npm', ...opts })
const out = (cmd, args) => execFileSync(cmd, args, { cwd: ROOT, encoding: 'utf8' }).trim()

if (!/^\d+\.\d+\.\d+$/.test(version || '')) die('Usage: npm run publish-launcher -- <x.y.z> "release notes"')
if (!notes) die('Write what is new, in Hebrew, after the version: npm run publish-launcher -- 0.3.0 "מה חדש"')

const conf = JSON.parse(readFileSync(join(ROOT, 'src-tauri/tauri.conf.json'), 'utf8'))
if (!conf.plugins?.updater?.pubkey || conf.plugins.updater.pubkey.startsWith('REPLACE_')) die('The updater public key is not set in src-tauri/tauri.conf.json yet (see README, "Releasing the launcher").')
if (out('git', ['status', '--porcelain'])) die('Commit or stash your changes first; the release commit must hold only the version change.')
if (out('git', ['rev-parse', '--abbrev-ref', 'HEAD']) !== 'main') die('Release from main.')
if (out('git', ['tag', '-l', `v${version}`])) die(`Tag v${version} already exists.`)
const current = versions()['package.json']
const [a, b] = [current, version].map((v) => v.split('.').map(Number))
if (!(b[0] > a[0] || (b[0] === a[0] && (b[1] > a[1] || (b[1] === a[1] && b[2] > a[2]))))) die(`${version} must be newer than ${current}.`)

setVersion(version)
run('npm', ['test', '--silent'])
run('git', ['add', 'package.json', 'package-lock.json', 'src-tauri/Cargo.toml', 'src-tauri/Cargo.lock', 'src-tauri/tauri.conf.json'])
run('git', ['commit', '-m', `Launcher ${version}`])
run('git', ['tag', '-a', `v${version}`, '-m', notes])
run('git', ['push', 'origin', 'main', `v${version}`])
console.log(`\nPushed v${version}. GitHub Actions is building it: https://github.com/NamiSwan222/tirgumim/actions`)
