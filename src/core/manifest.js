'use strict';
const fs = require('fs');
const { safeRelPath, resolveInside, sha256FileCached: sha256File, compareVersions } = require('./util');

const MANIFEST_NAME = 'tirgumim-pack.json';
const SCHEMA = 1;

/** Validate a parsed manifest object. Throws with a readable message. Returns a normalised copy. */
function validateManifest(m) {
  const errs = [];
  if (!m || typeof m !== 'object') throw new Error('Manifest is not an object');
  if (m.schema !== SCHEMA) errs.push(`unsupported schema ${m.schema} (expected ${SCHEMA})`);
  for (const k of ['packId', 'gameId', 'version']) if (typeof m[k] !== 'string' || !m[k]) errs.push(`missing "${k}"`);
  if (!Array.isArray(m.files) || m.files.length === 0) errs.push('"files" must be a non-empty array');
  const files = [];
  const seen = new Set();
  for (const f of m.files || []) {
    try {
      const p = safeRelPath(f.path);
      const key = p.toLowerCase(); // Windows is case-insensitive
      if (seen.has(key)) throw new Error(`duplicate path ${p}`);
      seen.add(key);
      if (!/^[0-9a-f]{64}$/i.test(f.sha256 || '')) throw new Error(`bad sha256 for ${p}`);
      files.push({ path: p, sha256: f.sha256.toLowerCase(), size: f.size });
    } catch (e) { errs.push(e.message); }
  }
  const gameVersions = [];
  for (const gv of m.gameVersions || []) {
    if (!Array.isArray(gv.checks) || !gv.checks.length) { errs.push(`gameVersion "${gv.name}" has no checks`); continue; }
    try {
      gameVersions.push({ name: gv.name || '?', checks: gv.checks.map((c) => ({ ...c, path: safeRelPath(c.path) })) });
    } catch (e) { errs.push(e.message); }
  }
  if (errs.length) throw new Error(`Invalid pack manifest: ${errs.join('; ')}`);
  return {
    ...m,
    files,
    gameVersions,
    allowUnknownGameVersion: !!m.allowUnknownGameVersion,
    requiresAccount: !!m.requiresAccount,
  };
}

/**
 * Check which declared game version the folder matches.
 * Returns { ok, matched: name|null, anyVersion: bool, allowUnknown: bool }
 */
async function checkGameVersion(manifest, gameDir) {
  if (!manifest.gameVersions.length) return { ok: true, matched: null, anyVersion: true, allowUnknown: true };
  for (const gv of manifest.gameVersions) {
    let all = true;
    for (const c of gv.checks) {
      const abs = resolveInside(gameDir, c.path);
      let st;
      try { st = fs.statSync(abs); } catch { all = false; break; }
      if (c.size != null && st.size !== c.size) { all = false; break; }
      if (c.sha256 && (await sha256File(abs)) !== c.sha256.toLowerCase()) { all = false; break; }
    }
    if (all) return { ok: true, matched: gv.name, anyVersion: false, allowUnknown: manifest.allowUnknownGameVersion };
  }
  return { ok: false, matched: null, anyVersion: false, allowUnknown: manifest.allowUnknownGameVersion };
}

function checkLauncherVersion(manifest, launcherVersion) {
  if (!manifest.minLauncherVersion) return true;
  return compareVersions(launcherVersion, manifest.minLauncherVersion) >= 0;
}

module.exports = { MANIFEST_NAME, SCHEMA, validateManifest, checkGameVersion, checkLauncherVersion };
