#!/usr/bin/env node
'use strict';
/**
 * Build a Tirgumim translation pack.
 *
 *   node tools/make-pack.js <pack.config.json> [--game-dir <path to original game>] [--out <file.zip>] [--url <download url>]
 *
 * pack.config.json holds every manifest field except "files", plus:
 *   "filesDir": folder (relative to the config) whose contents go under the game root.
 * In "gameVersions[].checks", a check with only "path" gets its sha256 + size filled
 * in from --game-dir (an untouched install of the supported game version).
 *
 * Prints the catalog "pack" entry to paste into catalog.json.
 */
const fs = require('fs');
const path = require('path');
const AdmZip = require('adm-zip');
const { sha256File, sha256Buffer } = require('../src/core/util');
const { validateManifest, MANIFEST_NAME } = require('../src/core/manifest');

function walk(dir, base = dir, out = []) {
  for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
    const p = path.join(dir, e.name);
    if (e.isDirectory()) walk(p, base, out); else out.push(path.relative(base, p).replace(/\\/g, '/'));
  }
  return out;
}

async function makePack({ configPath, gameDir, out, url }) {
  const cfgDir = path.dirname(path.resolve(configPath));
  const cfg = JSON.parse(fs.readFileSync(configPath, 'utf8').replace(/^﻿/, ''));
  const filesDir = path.resolve(cfgDir, cfg.filesDir || 'files');
  delete cfg.filesDir;

  const rels = walk(filesDir).sort();
  const files = [];
  for (const rel of rels) {
    const abs = path.join(filesDir, ...rel.split('/'));
    files.push({ path: rel, sha256: await sha256File(abs), size: fs.statSync(abs).size });
  }

  for (const gv of cfg.gameVersions || []) {
    for (const c of gv.checks || []) {
      if (c.sha256 || c.size != null || c.exists) continue;
      if (!gameDir) throw new Error(`Check "${c.path}" needs --game-dir to compute its hash`);
      const abs = path.join(gameDir, ...c.path.split('/'));
      c.sha256 = await sha256File(abs);
      c.size = fs.statSync(abs).size;
    }
  }

  const manifest = { schema: 1, ...cfg, files };
  validateManifest(manifest); // throws if anything is wrong

  const zip = new AdmZip();
  zip.addFile(MANIFEST_NAME, Buffer.from(JSON.stringify(manifest, null, 2), 'utf8'));
  for (const rel of rels) zip.addFile(`files/${rel}`, fs.readFileSync(path.join(filesDir, ...rel.split('/'))));
  const outPath = path.resolve(out || `${manifest.packId}-${manifest.version}.zip`);
  fs.mkdirSync(path.dirname(outPath), { recursive: true });
  zip.writeZip(outPath);

  const buf = fs.readFileSync(outPath);
  const entry = { version: manifest.version, url: url || path.basename(outPath), sha256: sha256Buffer(buf), size: buf.length };
  return { outPath, manifest, entry };
}

module.exports = { makePack };

if (require.main === module) {
  const args = process.argv.slice(2);
  const opt = (n) => { const i = args.indexOf(n); return i >= 0 ? args[i + 1] : undefined; };
  const configPath = args.find((a, i) => !a.startsWith('--') && !(i > 0 && args[i - 1].startsWith('--')));
  if (!configPath) { console.error('usage: make-pack <pack.config.json> [--game-dir DIR] [--out FILE.zip] [--url URL]'); process.exit(2); }
  makePack({ configPath, gameDir: opt('--game-dir'), out: opt('--out'), url: opt('--url') })
    .then(({ outPath, manifest, entry }) => {
      console.log(`Built ${outPath} (${manifest.files.length} files)`);
      console.log('Catalog "pack" entry:');
      console.log(JSON.stringify(entry, null, 2));
    })
    .catch((e) => { console.error(`Error: ${e.message}`); process.exit(1); });
}
