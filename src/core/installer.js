'use strict';
const fs = require('fs');
const path = require('path');
const crypto = require('crypto');
const AdmZip = require('adm-zip');
const { sha256File, sha256FileCached, resolveInside, exists, readJson, writeJsonAtomic, compareVersions } = require('./util');
const { MANIFEST_NAME, validateManifest, checkGameVersion, checkLauncherVersion } = require('./manifest');

class PackError extends Error {
  constructor(code, message, extra) { super(message); this.code = code; Object.assign(this, extra); }
}

function assertGameId(id) {
  if (typeof id !== 'string' || !/^[a-z0-9][a-z0-9._-]*$/i.test(id)) throw new PackError('BAD_GAME_ID', `Invalid game id: ${id}`);
}

/**
 * Installs / removes translation packs. All persistent data lives under dataDir:
 *   state.json            install records + custom game paths
 *   backups/<gameId>/...  original game files that a pack replaced
 *   tmp/                  extraction scratch
 */
class Installer {
  constructor({ dataDir, launcherVersion }) {
    this.dataDir = dataDir;
    this.launcherVersion = launcherVersion;
    this.stateFile = path.join(dataDir, 'state.json');
    fs.mkdirSync(dataDir, { recursive: true });
  }

  // ---------- state ----------
  loadState() {
    const s = readJson(this.stateFile, null) || {};
    return { installs: s.installs || {}, customPaths: s.customPaths || {}, settings: s.settings || {} };
  }
  saveState(s) { writeJsonAtomic(this.stateFile, s); }
  getRecord(gameId) { return this.loadState().installs[gameId] || null; }
  setCustomPath(gameId, dir) {
    assertGameId(gameId);
    const s = this.loadState();
    if (dir) s.customPaths[gameId] = dir; else delete s.customPaths[gameId];
    this.saveState(s);
  }
  backupDir(gameId) { assertGameId(gameId); return path.join(this.dataDir, 'backups', gameId); }

  // ---------- packs ----------
  /** Open and validate a pack zip. Verifies the zip hash if expectedSha256 is given. */
  async openPack(zipPath, expectedSha256) {
    if (expectedSha256) {
      const got = await sha256File(zipPath);
      if (got !== expectedSha256.toLowerCase()) throw new PackError('ZIP_HASH', `Pack file is corrupt (hash mismatch)`);
    }
    let zip;
    try { zip = new AdmZip(zipPath); } catch (e) { throw new PackError('ZIP_READ', `Cannot read pack zip: ${e.message}`); }
    const entry = zip.getEntry(MANIFEST_NAME);
    if (!entry) throw new PackError('NO_MANIFEST', `Pack has no ${MANIFEST_NAME}`);
    let raw;
    try { raw = JSON.parse(entry.getData().toString('utf8').replace(/^﻿/, '')); } catch (e) { throw new PackError('BAD_MANIFEST', `Manifest is not valid JSON: ${e.message}`); }
    let manifest;
    try { manifest = validateManifest(raw); } catch (e) { throw new PackError('BAD_MANIFEST', e.message); }
    return { manifest, zip };
  }

  /** Extract declared files to a temp dir and verify each hash. Returns the temp dir. */
  async extractVerified(zip, manifest) {
    const tmp = path.join(this.dataDir, 'tmp', crypto.randomBytes(6).toString('hex'));
    fs.mkdirSync(tmp, { recursive: true });
    try {
      const byName = new Map(zip.getEntries().map((e) => [e.entryName.replace(/\\/g, '/'), e]));
      for (const f of manifest.files) {
        const e = byName.get(`files/${f.path}`);
        if (!e || e.isDirectory) throw new PackError('MISSING_FILE', `Pack is missing file ${f.path}`);
        const out = resolveInside(tmp, f.path);
        fs.mkdirSync(path.dirname(out), { recursive: true });
        fs.writeFileSync(out, e.getData());
        if ((await sha256File(out)) !== f.sha256) throw new PackError('FILE_HASH', `File ${f.path} in pack is corrupt`);
      }
      return tmp;
    } catch (e) {
      fs.rmSync(tmp, { recursive: true, force: true });
      throw e;
    }
  }

  /**
   * Pre-install checks without touching the game. Returns
   * { manifest, gameVersion: {ok, matched, anyVersion, allowUnknown} }
   */
  async inspect({ gameId, gameDir, zipPath, expectedSha256 }) {
    const { manifest } = await this.openPack(zipPath, expectedSha256);
    if (manifest.gameId !== gameId) throw new PackError('WRONG_GAME', `Pack is for "${manifest.gameId}", not "${gameId}"`);
    if (!checkLauncherVersion(manifest, this.launcherVersion)) {
      throw new PackError('LAUNCHER_TOO_OLD', `Pack needs launcher ${manifest.minLauncherVersion} or newer`);
    }
    const gameVersion = await checkGameVersion(manifest, gameDir);
    return { manifest, gameVersion };
  }

  /**
   * Install a pack. `force` allows installing on an unrecognised game version when
   * the pack sets allowUnknownGameVersion. Rolls back on any failure.
   */
  async install({ gameId, gameDir, zipPath, expectedSha256, force = false, onProgress = () => {} }) {
    assertGameId(gameId);
    if (!gameDir || !exists(gameDir)) throw new PackError('NO_GAME_DIR', `Game folder not found: ${gameDir}`);
    if (this.getRecord(gameId)) throw new PackError('ALREADY_INSTALLED', 'A translation is already installed for this game; uninstall or update it');

    onProgress('verify');
    const { manifest, zip } = await this.openPack(zipPath, expectedSha256);
    if (manifest.gameId !== gameId) throw new PackError('WRONG_GAME', `Pack is for "${manifest.gameId}", not "${gameId}"`);
    if (!checkLauncherVersion(manifest, this.launcherVersion)) throw new PackError('LAUNCHER_TOO_OLD', `Pack needs launcher ${manifest.minLauncherVersion} or newer`);
    const gv = await checkGameVersion(manifest, gameDir);
    if (!gv.ok && !(gv.allowUnknown && force)) {
      throw new PackError('GAME_VERSION', 'This game version is not supported by the pack', { allowUnknown: gv.allowUnknown });
    }

    const tmp = await this.extractVerified(zip, manifest);
    const bdir = this.backupDir(gameId);
    if (exists(bdir)) {
      // Leftovers without an install record (e.g. crash). Never delete originals: set aside.
      fs.renameSync(bdir, `${bdir}.orphan-${Date.now()}`);
    }

    const done = []; // { abs, backupAbs|null }
    const createdDirs = [];
    const recordFiles = [];
    try {
      onProgress('backup');
      // 1) back up everything first
      const plan = manifest.files.map((f) => {
        const abs = resolveInside(gameDir, f.path);
        const had = exists(abs);
        const backupAbs = had ? resolveInside(bdir, f.path) : null;
        if (had) {
          if (fs.statSync(abs).isDirectory()) throw new PackError('IS_DIR', `${f.path} is a folder in the game`);
          fs.mkdirSync(path.dirname(backupAbs), { recursive: true });
          fs.copyFileSync(abs, backupAbs);
        }
        return { f, abs, backupAbs };
      });
      // 2) write pack files
      onProgress('copy');
      for (const { f, abs, backupAbs } of plan) {
        this._mkdirTracked(gameDir, path.dirname(abs), createdDirs);
        fs.copyFileSync(resolveInside(tmp, f.path), abs);
        done.push({ abs, backupAbs });
        recordFiles.push({ path: f.path, sha256: f.sha256, backedUp: !!backupAbs });
      }
      const s = this.loadState();
      s.installs[gameId] = {
        packId: manifest.packId,
        version: manifest.version,
        title: manifest.title || manifest.packId,
        gameDir,
        gameVersion: gv.matched,
        forcedUnknownVersion: !gv.ok,
        installedAt: new Date().toISOString(),
        files: recordFiles,
        createdDirs: createdDirs.map((d) => path.relative(gameDir, d).replace(/\\/g, '/')),
      };
      this.saveState(s);
      onProgress('done');
      return s.installs[gameId];
    } catch (e) {
      for (const { abs, backupAbs } of done.reverse()) {
        try { if (backupAbs) fs.copyFileSync(backupAbs, abs); else fs.rmSync(abs, { force: true }); } catch { /* best effort */ }
      }
      this._removeEmptyDirs(createdDirs);
      fs.rmSync(bdir, { recursive: true, force: true });
      throw e;
    } finally {
      fs.rmSync(tmp, { recursive: true, force: true });
    }
  }

  /**
   * Restore originals. Files the game has changed since install (e.g. after a game
   * update) are left as they are. Returns { restored, deleted, skippedChanged }.
   */
  async uninstall(gameId) {
    assertGameId(gameId);
    const rec = this.getRecord(gameId);
    if (!rec) throw new PackError('NOT_INSTALLED', 'No translation installed for this game');
    const bdir = this.backupDir(gameId);
    const result = { restored: [], deleted: [], skippedChanged: [], missingBackup: [] };
    for (const f of rec.files) {
      const abs = resolveInside(rec.gameDir, f.path);
      const current = exists(abs) ? await sha256File(abs) : null;
      if (current !== null && current !== f.sha256) { result.skippedChanged.push(f.path); continue; }
      if (f.backedUp) {
        const b = resolveInside(bdir, f.path);
        if (!exists(b)) { result.missingBackup.push(f.path); continue; }
        fs.mkdirSync(path.dirname(abs), { recursive: true });
        fs.copyFileSync(b, abs);
        result.restored.push(f.path);
      } else if (current !== null) {
        fs.rmSync(abs, { force: true });
        result.deleted.push(f.path);
      }
    }
    this._removeEmptyDirs((rec.createdDirs || []).map((d) => resolveInside(rec.gameDir, d)));
    if (result.missingBackup.length) {
      // Keep record + backups so nothing is lost; caller shows an error.
      throw new PackError('MISSING_BACKUP', `Backup missing for: ${result.missingBackup.join(', ')}`, { result });
    }
    fs.rmSync(bdir, { recursive: true, force: true });
    const s = this.loadState();
    delete s.installs[gameId];
    this.saveState(s);
    return result;
  }

  /** Update = uninstall old, install new. */
  async update(opts) {
    if (this.getRecord(opts.gameId)) await this.uninstall(opts.gameId);
    return this.install(opts);
  }

  /** Check installed files are still ours. Returns { intact, changed: [paths] } or null. */
  async verify(gameId) {
    const rec = this.getRecord(gameId);
    if (!rec) return null;
    const changed = [];
    for (const f of rec.files) {
      const abs = resolveInside(rec.gameDir, f.path);
      if (!exists(abs) || (await sha256FileCached(abs)) !== f.sha256) changed.push(f.path);
    }
    return { intact: changed.length === 0, changed };
  }

  /** Is the catalog's pack newer than what's installed? */
  updateAvailable(gameId, catalogVersion) {
    const rec = this.getRecord(gameId);
    return !!(rec && catalogVersion && compareVersions(catalogVersion, rec.version) > 0);
  }

  _mkdirTracked(root, dir, created) {
    const rootAbs = path.resolve(root);
    const missing = [];
    let d = path.resolve(dir);
    while (d.startsWith(rootAbs + path.sep) && !exists(d)) { missing.unshift(d); d = path.dirname(d); }
    for (const m of missing) { fs.mkdirSync(m); created.push(m); }
  }

  _removeEmptyDirs(dirs) {
    for (const d of [...dirs].sort((a, b) => b.length - a.length)) {
      try { if (fs.readdirSync(d).length === 0) fs.rmdirSync(d); } catch { /* ignore */ }
    }
  }
}

module.exports = { Installer, PackError };
