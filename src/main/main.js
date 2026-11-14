'use strict';
const { app, BrowserWindow, ipcMain, dialog, shell, protocol, net } = require('electron');
const fs = require('fs');
const path = require('path');
const { Installer, PackError } = require('../core/installer');
const { loadCatalog, downloadPack, pathToFileURL } = require('../core/catalog');
const { isGameDir, findEpic, findSteam, listSteamGames, listEpicGames, findSteamPosterFile } = require('../core/detect');

app.setName('Tirgumim');
// %APPDATA%\Tirgumim is already used by another app on this PC, so keep ours separate.
app.setPath('userData', path.join(app.getPath('appData'), 'Tirgumim Launcher'));
protocol.registerSchemesAsPrivileged([{ scheme: 'poster', privileges: { standard: true, secure: true, supportFetchAPI: true } }]);

const ROOT = path.join(__dirname, '..', '..');
const DEV_CATALOG = process.argv.includes('--dev-catalog');
const SHOT_ARG = process.argv.find((a) => a.startsWith('--screenshot='));
const BUNDLED_CATALOG = path.join(ROOT, 'catalog', 'catalog.json');
// Where the launcher reads our translations list. Catalog lives in the NamiSwan222/tirgumim repo;
// it can also be changed from the settings dialog.
const DEFAULT_CATALOG_URL = 'https://raw.githubusercontent.com/NamiSwan222/tirgumim/main/catalog/catalog.json';

let win;
let installer;
let catalog = null;
let catalogSource = null; // 'github' | 'bundled' | 'dev'
let catalogError = null;
let scanned = null; // installed games on this PC
let busy = false;

// ---------- catalog ----------
function catalogUrl() {
  if (DEV_CATALOG) return pathToFileURL(path.join(ROOT, 'dev', 'catalog.json')).href;
  return installer.loadState().settings.catalogUrl || DEFAULT_CATALOG_URL;
}

async function refreshCatalog() {
  catalogError = null;
  if (DEV_CATALOG) {
    try { catalog = await loadCatalog(catalogUrl()); catalogSource = 'dev'; return; } catch (e) { catalogError = e.message; }
  } else {
    try { catalog = await loadCatalog(catalogUrl()); catalogSource = 'github'; return; } catch (e) { catalogError = e.message; }
  }
  // Offline / not configured: use the copy shipped with the launcher.
  try { catalog = await loadCatalog(pathToFileURL(BUNDLED_CATALOG).href); catalogSource = 'bundled'; } catch (e) { catalog = null; catalogError = e.message; }
}

function findGame(gameId) {
  const g = catalog && catalog.games.find((x) => x.id === gameId);
  if (!g) throw new PackError('UNKNOWN_GAME', `Unknown game ${gameId}`);
  return g;
}

// ---------- installed games on this PC ----------
async function rescan() {
  const [steam, epic] = await Promise.all([listSteamGames().catch(() => []), Promise.resolve(listEpicGames())]);
  scanned = [...steam, ...epic];
}

function scannedFor(game) {
  const d = game.detect || {};
  return (scanned || []).find((s) => (s.store === 'steam' && d.steamAppId && s.id === String(d.steamAppId)) ||
                                      (s.store === 'epic' && d.epicAppName && s.id === d.epicAppName)) || null;
}

async function resolveGameDir(game) {
  const st = installer.loadState();
  const rec = st.installs[game.id];
  if (rec && fs.existsSync(rec.gameDir)) return { dir: rec.gameDir, source: 'installed' };
  const custom = st.customPaths[game.id] || (DEV_CATALOG ? game.devPath : null);
  if (custom && isGameDir(game, custom)) return { dir: custom, source: 'custom' };
  const hit = scannedFor(game);
  if (hit && isGameDir(game, hit.dir)) return { dir: hit.dir, source: hit.store };
  if (!scanned) { // scan not done yet: direct lookup
    const d = game.detect || {};
    const s = await findSteam(d.steamAppId);
    if (s && isGameDir(game, s)) return { dir: s, source: 'steam' };
    const e = findEpic(d.epicAppName);
    if (e && isGameDir(game, e)) return { dir: e, source: 'epic' };
  }
  return null;
}

// ---------- posters ----------
const posterMisses = new Set();
const STEAM_CDN = (id) => [
  `https://shared.cloudflare.steamstatic.com/store_item_assets/steam/apps/${id}/library_600x900.jpg`,
  `https://cdn.cloudflare.steamstatic.com/steam/apps/${id}/library_600x900.jpg`,
];

async function cachedDownload(urls, cacheName) {
  const dir = path.join(app.getPath('userData'), 'posters');
  const file = path.join(dir, cacheName);
  if (fs.existsSync(file)) return file;
  if (posterMisses.has(cacheName)) return null;
  for (const u of urls) {
    if (!/^https:\/\//.test(u)) continue;
    try {
      const res = await net.fetch(u);
      if (!res.ok || !/^image\//.test(res.headers.get('content-type') || '')) continue;
      fs.mkdirSync(dir, { recursive: true });
      fs.writeFileSync(file, Buffer.from(await res.arrayBuffer()));
      return file;
    } catch { /* try next */ }
  }
  posterMisses.add(cacheName);
  return null;
}

async function steamPoster(appId) {
  if (!/^\d+$/.test(appId)) return null;
  return (await findSteamPosterFile(appId)) || cachedDownload(STEAM_CDN(appId), `steam-${appId}.jpg`);
}

async function resolvePoster(kind, id) {
  if (kind === 'steam') return steamPoster(id);
  if (kind === 'game') {
    const g = catalog && catalog.games.find((x) => x.id === id);
    if (!g) return null;
    if (g.poster && g.poster.startsWith('file:')) return require('url').fileURLToPath(g.poster);
    if (g.poster) return cachedDownload([g.poster], `game-${g.id}${path.extname(new URL(g.poster).pathname) || '.jpg'}`);
    if (g.detect && g.detect.steamAppId) return steamPoster(String(g.detect.steamAppId));
  }
  if (kind === 'epic') {
    // Epic keeps no cover art on disk; most Epic games are also on Steam, so use that cover.
    const g = (scanned || []).find((s) => s.store === 'epic' && s.id === id);
    const appId = g && (await steamIdByName(g.name));
    if (appId) return steamPoster(appId);
  }
  return null; // the UI draws a placeholder
}

const norm = (s) => s.toLowerCase().replace(/[®™©]/g, '').replace(/[^a-z0-9]+/g, ' ').trim();
const steamIdCache = new Map();
async function steamIdByName(name) {
  if (steamIdCache.has(name)) return steamIdCache.get(name);
  let id = null;
  try {
    const res = await net.fetch(`https://store.steampowered.com/api/storesearch/?term=${encodeURIComponent(name)}&l=english&cc=US`);
    const items = (await res.json()).items || [];
    const hit = items.find((i) => norm(i.name) === norm(name)) || items.find((i) => norm(i.name).startsWith(norm(name)));
    if (hit) id = String(hit.id);
  } catch { /* offline */ }
  steamIdCache.set(name, id);
  return id;
}

function registerPosterProtocol() {
  protocol.handle('poster', async (req) => {
    try {
      const u = new URL(req.url);
      const file = await resolvePoster(u.hostname, decodeURIComponent(u.pathname.slice(1)));
      if (file && fs.existsSync(file)) return net.fetch(pathToFileURL(file).href);
    } catch { /* fall through */ }
    return new Response(null, { status: 404 });
  });
}

// ---------- views ----------
async function gameView(game) {
  const loc = await resolveGameDir(game);
  const rec = installer.getRecord(game.id);
  const v = rec ? await installer.verify(game.id) : null;
  let status = 'notFound';
  if (rec) status = !v.intact ? 'needsRepair' : installer.updateAvailable(game.id, game.pack && game.pack.version) ? 'updateAvailable' : 'installed';
  else if (loc) status = game.pack ? 'ready' : 'noPack';
  return {
    id: game.id, name: game.name, nameHe: game.nameHe, engine: game.engine,
    poster: `poster://game/${encodeURIComponent(game.id)}`,
    location: loc, status,
    hasPack: !!game.pack,
    available: game.pack ? { version: game.pack.version, size: game.pack.size, changelog: game.pack.changelog } : null,
    installed: rec ? { version: rec.version, installedAt: rec.installedAt, gameVersion: rec.gameVersion, forced: rec.forcedUnknownVersion, changed: v.changed } : null,
  };
}

function otherGames() {
  const matched = new Set((catalog ? catalog.games : []).map(scannedFor).filter(Boolean));
  return (scanned || [])
    .filter((s) => !matched.has(s))
    .map((s) => ({ store: s.store, id: s.id, name: s.name, dir: s.dir, poster: `poster://${s.store}/${encodeURIComponent(s.id)}` }))
    .sort((a, b) => a.name.localeCompare(b.name));
}

function send(ch, data) { if (win && !win.isDestroyed()) win.webContents.send(ch, data); }

function errOut(e) {
  return { ok: false, error: { code: e.code || 'ERROR', message: e.message, allowUnknown: e.allowUnknown } };
}

async function withBusy(fn) {
  if (busy) return { ok: false, error: { code: 'BUSY', message: 'Another operation is running' } };
  busy = true;
  try { return { ok: true, result: await fn() }; } catch (e) { return errOut(e); } finally { busy = false; send('progress', null); }
}

async function doInstall(gameId, { force = false, update = false } = {}) {
  const game = findGame(gameId);
  if (!game.pack) throw new PackError('NO_PACK', 'No translation pack yet for this game');
  const loc = await resolveGameDir(game);
  if (!loc) throw new PackError('NO_GAME_DIR', 'Game folder not found');
  send('progress', { gameId, step: 'download', fraction: 0 });
  const dl = path.join(installer.dataDir, 'downloads');
  const zipPath = await downloadPack(game.pack.url, dl, (f) => send('progress', { gameId, step: 'download', fraction: f }));
  try {
    const opts = { gameId, gameDir: loc.dir, zipPath, expectedSha256: game.pack.sha256, force, onProgress: (step) => send('progress', { gameId, step }) };
    return update ? await installer.update(opts) : await installer.install(opts);
  } finally {
    if (zipPath.startsWith(dl)) fs.rmSync(zipPath, { force: true });
  }
}

async function listAll(reload) {
  if (reload || !catalog) await refreshCatalog();
  if (reload || !scanned) await rescan();
  return {
    ok: true,
    result: {
      translations: catalog ? await Promise.all(catalog.games.map(gameView)) : [],
      others: otherGames(),
      catalog: { source: catalogSource, error: catalogError, updated: catalog && catalog.updated },
    },
  };
}

function registerIpc() {
  ipcMain.handle('app:info', () => ({ version: app.getVersion(), dev: DEV_CATALOG }));
  ipcMain.handle('games:list', (_e, { reload } = {}) => listAll(reload).catch(errOut));
  ipcMain.handle('game:install', (_e, gameId, opts) => withBusy(() => doInstall(gameId, opts)));
  ipcMain.handle('game:update', (_e, gameId, opts) => withBusy(() => doInstall(gameId, { ...opts, update: true })));
  ipcMain.handle('game:uninstall', (_e, gameId) => withBusy(() => installer.uninstall(gameId)));
  ipcMain.handle('game:pickFolder', async (_e, gameId) => {
    try {
      const game = findGame(gameId);
      if (installer.getRecord(gameId)) throw new PackError('ALREADY_INSTALLED', 'Uninstall the translation before changing the folder');
      const r = await dialog.showOpenDialog(win, { title: `בחר את תיקיית ההתקנה של ${game.nameHe || game.name}`, properties: ['openDirectory'] });
      if (r.canceled || !r.filePaths[0]) return { ok: true, result: null };
      const dir = r.filePaths[0];
      if (!isGameDir(game, dir)) throw new PackError('NOT_GAME_DIR', `The folder does not contain ${game.detect.markerFile}`);
      installer.setCustomPath(gameId, dir);
      return { ok: true, result: dir };
    } catch (e) { return errOut(e); }
  });
  ipcMain.handle('game:openFolder', async (_e, gameId) => {
    const loc = await resolveGameDir(findGame(gameId));
    if (loc) shell.openPath(loc.dir);
  });
  ipcMain.handle('settings:get', () => ({ catalogUrl: installer.loadState().settings.catalogUrl || '', defaultCatalogUrl: DEFAULT_CATALOG_URL }));
  ipcMain.handle('settings:set', (_e, { catalogUrl }) => {
    const url = (catalogUrl || '').trim();
    if (url && !/^https:\/\//.test(url)) return { ok: false, error: { code: 'BAD_URL', message: 'Must start with https://' } };
    const s = installer.loadState();
    if (url) s.settings.catalogUrl = url; else delete s.settings.catalogUrl;
    installer.saveState(s);
    return { ok: true };
  });
}

function createWindow() {
  win = new BrowserWindow({
    width: 1200, height: 820, minWidth: 820, minHeight: 560,
    title: 'תרגומים', backgroundColor: '#0f1115', autoHideMenuBar: true, show: !SHOT_ARG,
    webPreferences: { preload: path.join(__dirname, 'preload.js'), contextIsolation: true, nodeIntegration: false, sandbox: true },
  });
  win.loadFile(path.join(ROOT, 'src', 'renderer', 'index.html'));
  win.webContents.setWindowOpenHandler(() => ({ action: 'deny' }));
  win.webContents.on('will-navigate', (e) => e.preventDefault());
}

/** --screenshot=<dir>: capture the UI; with --dev-catalog also install + uninstall the sample pack. */
async function screenshotRun(dir) {
  fs.mkdirSync(dir, { recursive: true });
  const wait = (ms) => new Promise((r) => setTimeout(r, ms));
  const shot = async (name, scrollY = 0) => {
    await win.webContents.executeJavaScript(`(async () => { window.__refresh && await window.__refresh(); document.querySelector('main').scrollTo(0, ${scrollY}); })()`);
    await wait(1500);
    fs.writeFileSync(path.join(dir, name), (await win.webContents.capturePage()).toPNG());
  };
  if (win.webContents.isLoading()) await new Promise((r) => win.webContents.once('did-finish-load', r));
  await wait(1500);
  await shot('1-initial.png');
  await shot('1b-scrolled.png', 700);
  const log = [];
  const game = DEV_CATALOG && catalog.games.find((g) => g.pack);
  if (game) {
    log.push(['install', await withBusy(() => doInstall(game.id))]);
    await shot('2-installed.png');
    log.push(['uninstall', await withBusy(() => installer.uninstall(game.id))]);
    await shot('3-uninstalled.png');
  }
  fs.writeFileSync(path.join(dir, 'log.json'), JSON.stringify(log, null, 2));
  app.quit();
}

app.whenReady().then(async () => {
  installer = new Installer({ dataDir: path.join(app.getPath('userData'), DEV_CATALOG ? 'dev-data' : 'data'), launcherVersion: app.getVersion() });
  registerPosterProtocol();
  registerIpc();
  createWindow();
  if (SHOT_ARG) screenshotRun(SHOT_ARG.split('=')[1]).catch((e) => { console.error(e); app.exit(1); });
});

app.on('window-all-closed', () => app.quit());
