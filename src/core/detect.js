'use strict';
const fs = require('fs');
const path = require('path');
const { execFile } = require('child_process');
const { exists, readJson, resolveInside } = require('./util');

function regQuery(key, value) {
  return new Promise((resolve) => {
    if (process.platform !== 'win32') return resolve(null);
    execFile('reg', ['query', key, '/v', value], { windowsHide: true }, (err, out) => {
      if (err) return resolve(null);
      const m = out.match(new RegExp(`${value}\\s+REG_\\w+\\s+(.+)`, 'i'));
      resolve(m ? m[1].trim() : null);
    });
  });
}

/** Minimal Valve KeyValues (VDF/ACF) parser — enough for libraryfolders.vdf and appmanifest. */
function parseVdf(text) {
  const tokens = [];
  const re = /"((?:[^"\\]|\\.)*)"|([{}])/g;
  let m;
  while ((m = re.exec(text))) tokens.push(m[2] || { s: m[1].replace(/\\\\/g, '\\') });
  let i = 0;
  function obj() {
    const o = {};
    while (i < tokens.length) {
      const t = tokens[i++];
      if (t === '}') return o;
      const key = t.s;
      const v = tokens[i++];
      if (v === '{') o[key] = obj(); else if (v && v.s !== undefined) o[key] = v.s;
    }
    return o;
  }
  return obj();
}

let steamRootsCache = null;
async function steamRoots() {
  if (steamRootsCache) return steamRootsCache;
  const roots = new Set();
  const reg = (await regQuery('HKCU\\Software\\Valve\\Steam', 'SteamPath')) ||
              (await regQuery('HKLM\\SOFTWARE\\WOW6432Node\\Valve\\Steam', 'InstallPath'));
  if (reg) roots.add(path.normalize(reg));
  roots.add('C:\\Program Files (x86)\\Steam');
  steamRootsCache = [...roots].filter((r) => exists(path.join(r, 'steamapps')));
  return steamRootsCache;
}

async function steamLibraries() {
  const roots = await steamRoots();
  const libs = new Set();
  for (const r of roots) {
    const vdf = path.join(r, 'steamapps', 'libraryfolders.vdf');
    if (!exists(vdf)) continue;
    libs.add(path.normalize(r));
    try {
      const data = parseVdf(fs.readFileSync(vdf, 'utf8'));
      const lf = data.libraryfolders || data.LibraryFolders || {};
      for (const v of Object.values(lf)) {
        const p = typeof v === 'string' ? v : v && v.path;
        if (p) libs.add(path.normalize(p));
      }
    } catch { /* ignore */ }
  }
  return [...libs];
}

async function findSteam(appId) {
  if (!appId) return null;
  for (const lib of await steamLibraries()) {
    const acf = path.join(lib, 'steamapps', `appmanifest_${appId}.acf`);
    if (!exists(acf)) continue;
    try {
      const st = parseVdf(fs.readFileSync(acf, 'utf8')).AppState || {};
      if (st.installdir) {
        const dir = path.join(lib, 'steamapps', 'common', st.installdir);
        if (exists(dir)) return dir;
      }
    } catch { /* ignore */ }
  }
  return null;
}

function findEpic(appName) {
  if (!appName) return null;
  const dir = path.join(process.env.ProgramData || 'C:\\ProgramData', 'Epic', 'EpicGamesLauncher', 'Data', 'Manifests');
  let files = [];
  try { files = fs.readdirSync(dir).filter((f) => f.endsWith('.item')); } catch { return null; }
  for (const f of files) {
    const j = readJson(path.join(dir, f), null);
    if (j && (j.AppName === appName || j.MainGameAppName === appName) && j.InstallLocation && exists(j.InstallLocation)) {
      return j.InstallLocation;
    }
  }
  return null;
}

// Steam tools/runtimes that aren't games
const NOT_A_GAME = /redistributable|proton|steam linux runtime|steamworks|steamvr|dedicated server|\bsdk\b|soundtrack/i;

/** Every installed Steam game: [{ store:'steam', id, name, dir }] */
async function listSteamGames() {
  const out = new Map();
  for (const lib of await steamLibraries()) {
    const sa = path.join(lib, 'steamapps');
    let files = [];
    try { files = fs.readdirSync(sa).filter((f) => /^appmanifest_\d+\.acf$/.test(f)); } catch { continue; }
    for (const f of files) {
      try {
        const st = parseVdf(fs.readFileSync(path.join(sa, f), 'utf8')).AppState || {};
        if (!st.appid || !st.installdir || !st.name || NOT_A_GAME.test(st.name) || st.appid === '228980') continue;
        const dir = path.join(sa, 'common', st.installdir);
        if (!exists(dir)) continue;
        out.set(st.appid, { store: 'steam', id: st.appid, name: st.name, dir });
      } catch { /* skip broken manifest */ }
    }
  }
  return [...out.values()];
}

/** Every installed Epic game: [{ store:'epic', id: AppName, name, dir }] */
function listEpicGames() {
  const dir = path.join(process.env.ProgramData || 'C:\\ProgramData', 'Epic', 'EpicGamesLauncher', 'Data', 'Manifests');
  let files = [];
  try { files = fs.readdirSync(dir).filter((f) => f.endsWith('.item')); } catch { return []; }
  const out = [];
  for (const f of files) {
    const j = readJson(path.join(dir, f), null);
    if (!j || !j.AppName || !j.DisplayName || !j.InstallLocation || !exists(j.InstallLocation)) continue;
    if (j.bIsIncompleteInstall) continue;
    const cats = j.AppCategories || [];
    if (cats.length && !cats.includes('games')) continue;
    if (j.MainGameAppName && j.MainGameAppName !== j.AppName) continue; // DLC
    out.push({ store: 'epic', id: j.AppName, name: j.DisplayName, dir: j.InstallLocation });
  }
  return out;
}

/** Look for Steam's own cached portrait cover for an app. Returns a file path or null. */
async function findSteamPosterFile(appId) {
  if (!/^\d+$/.test(String(appId))) return null;
  const wanted = ['library_600x900.jpg', 'library_capsule.jpg', 'library_600x900_2x.jpg'];
  for (const root of await steamRoots()) {
    const base = path.join(root, 'appcache', 'librarycache');
    const candidates = [];
    for (const w of wanted) candidates.push(path.join(base, `${appId}_${w}`)); // older Steam layout
    const appDir = path.join(base, String(appId));
    for (const w of wanted) candidates.push(path.join(appDir, w));
    try {
      for (const sub of fs.readdirSync(appDir, { withFileTypes: true })) {
        if (sub.isDirectory()) for (const w of wanted) candidates.push(path.join(appDir, sub.name, w));
      }
    } catch { /* no folder */ }
    const hit = candidates.find(exists);
    if (hit) return hit;
  }
  return null;
}

/** Does `dir` look like this game (marker file present)? */
function isGameDir(game, dir) {
  if (!dir || !exists(dir)) return false;
  const marker = game.detect && game.detect.markerFile;
  if (!marker) return true;
  try { return exists(resolveInside(dir, marker)); } catch { return false; }
}

/** Returns { dir, source: 'custom'|'steam'|'epic' } or null. */
async function locateGame(game, customPath) {
  if (customPath && isGameDir(game, customPath)) return { dir: customPath, source: 'custom' };
  const d = game.detect || {};
  const steam = await findSteam(d.steamAppId);
  if (steam && isGameDir(game, steam)) return { dir: steam, source: 'steam' };
  const epic = findEpic(d.epicAppName);
  if (epic && isGameDir(game, epic)) return { dir: epic, source: 'epic' };
  return null;
}

module.exports = { locateGame, isGameDir, parseVdf, findSteam, findEpic, listSteamGames, listEpicGames, findSteamPosterFile };
