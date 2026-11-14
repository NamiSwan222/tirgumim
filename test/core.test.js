'use strict';
const test = require('node:test');
const assert = require('node:assert');
const fs = require('fs');
const os = require('os');
const path = require('path');
const AdmZip = require('adm-zip');
const { Installer } = require('../src/core/installer');
const { makePack } = require('../tools/make-pack');
const { parseVdf } = require('../src/core/detect');
const { sha256File } = require('../src/core/util');

const DEV = path.join(__dirname, '..', 'dev');

function setup() {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'tirgumim-test-'));
  const game = path.join(root, 'game');
  fs.cpSync(path.join(DEV, 'sample-game-template'), game, { recursive: true });
  const inst = new Installer({ dataDir: path.join(root, 'data'), launcherVersion: '0.1.0' });
  return { root, game, inst };
}

async function buildPack(root, mutate) {
  const cfgDir = path.join(root, 'packsrc');
  fs.cpSync(path.join(DEV, 'sample-pack'), cfgDir, { recursive: true });
  if (mutate) mutate(cfgDir);
  return makePack({ configPath: path.join(cfgDir, 'pack.config.json'), gameDir: path.join(DEV, 'sample-game-template'), out: path.join(root, 'pack.zip') });
}

const read = (p) => fs.readFileSync(p, 'utf8');

test('install replaces + adds files, uninstall restores originals exactly', async () => {
  const { root, game, inst } = setup();
  const { outPath, entry } = await buildPack(root);
  const origStrings = read(path.join(game, 'data/strings.txt'));
  const origHash = await sha256File(path.join(game, 'data/strings.txt'));

  const rec = await inst.install({ gameId: 'sample-game', gameDir: game, zipPath: outPath, expectedSha256: entry.sha256 });
  assert.equal(rec.version, '0.1.0');
  assert.equal(rec.gameVersion, '1.0');
  assert.match(read(path.join(game, 'data/strings.txt')), /התחל משחק/);
  assert.ok(fs.existsSync(path.join(game, 'data/fonts/hebrew.ttf')));
  assert.deepEqual((await inst.verify('sample-game')).intact, true);

  const res = await inst.uninstall('sample-game');
  assert.deepEqual(res.restored, ['data/strings.txt']);
  assert.deepEqual(res.deleted, ['data/fonts/hebrew.ttf']);
  assert.equal(read(path.join(game, 'data/strings.txt')), origStrings);
  assert.equal(await sha256File(path.join(game, 'data/strings.txt')), origHash);
  assert.ok(!fs.existsSync(path.join(game, 'data/fonts')), 'created folder removed');
  assert.equal(inst.getRecord('sample-game'), null);
  assert.ok(!fs.existsSync(inst.backupDir('sample-game')));
});

test('rejects zip with wrong hash and wrong game id', async () => {
  const { root, game, inst } = setup();
  const { outPath } = await buildPack(root);
  await assert.rejects(inst.install({ gameId: 'sample-game', gameDir: game, zipPath: outPath, expectedSha256: '0'.repeat(64) }), { code: 'ZIP_HASH' });
  await assert.rejects(inst.install({ gameId: 'other-game', gameDir: game, zipPath: outPath }), { code: 'WRONG_GAME' });
  assert.match(read(path.join(game, 'data/strings.txt')), /Start Game/);
});

test('unknown game version: blocked unless forced and allowed', async () => {
  const { root, game, inst } = setup();
  fs.writeFileSync(path.join(game, 'SampleGame.exe'), 'patched exe v1.1');
  const { outPath } = await buildPack(root);
  await assert.rejects(inst.install({ gameId: 'sample-game', gameDir: game, zipPath: outPath }), { code: 'GAME_VERSION', allowUnknown: true });
  const rec = await inst.install({ gameId: 'sample-game', gameDir: game, zipPath: outPath, force: true });
  assert.equal(rec.forcedUnknownVersion, true);

  const { root: r2, game: g2, inst: i2 } = setup();
  fs.writeFileSync(path.join(g2, 'SampleGame.exe'), 'patched exe v1.1');
  const strict = await buildPack(r2, (d) => {
    const f = path.join(d, 'pack.config.json');
    fs.writeFileSync(f, JSON.stringify({ ...JSON.parse(read(f)), allowUnknownGameVersion: false }));
  });
  await assert.rejects(i2.install({ gameId: 'sample-game', gameDir: g2, zipPath: strict.outPath, force: true }), { code: 'GAME_VERSION', allowUnknown: false });
});

test('uninstall after game update keeps the game’s new file', async () => {
  const { root, game, inst } = setup();
  const { outPath } = await buildPack(root);
  await inst.install({ gameId: 'sample-game', gameDir: game, zipPath: outPath });
  fs.writeFileSync(path.join(game, 'data/strings.txt'), 'menu.start=Start Game (v1.1)\n');
  assert.deepEqual((await inst.verify('sample-game')).changed, ['data/strings.txt']);
  const res = await inst.uninstall('sample-game');
  assert.deepEqual(res.skippedChanged, ['data/strings.txt']);
  assert.match(read(path.join(game, 'data/strings.txt')), /v1\.1/);
});

test('update installs newer version and keeps true originals', async () => {
  const { root, game, inst } = setup();
  const v1 = await buildPack(root);
  await inst.install({ gameId: 'sample-game', gameDir: game, zipPath: v1.outPath });
  const root2 = fs.mkdtempSync(path.join(os.tmpdir(), 'tirgumim-test-'));
  const v2 = await buildPack(root2, (d) => {
    const f = path.join(d, 'pack.config.json');
    fs.writeFileSync(f, JSON.stringify({ ...JSON.parse(read(f)), version: '0.2.0' }));
    fs.writeFileSync(path.join(d, 'files/data/strings.txt'), 'menu.start=התחלה\n');
  });
  assert.ok(inst.updateAvailable('sample-game', '0.2.0'));
  await inst.update({ gameId: 'sample-game', gameDir: game, zipPath: v2.outPath });
  assert.equal(inst.getRecord('sample-game').version, '0.2.0');
  assert.match(read(path.join(game, 'data/strings.txt')), /התחלה/);
  await inst.uninstall('sample-game');
  assert.match(read(path.join(game, 'data/strings.txt')), /Start Game/);
});

test('rejects path traversal in manifest', async () => {
  const { root, game, inst } = setup();
  const zip = new AdmZip();
  zip.addFile('tirgumim-pack.json', Buffer.from(JSON.stringify({
    schema: 1, packId: 'x', gameId: 'sample-game', version: '1.0.0',
    files: [{ path: '../evil.txt', sha256: 'a'.repeat(64) }],
  })));
  const p = path.join(root, 'evil.zip');
  zip.writeZip(p);
  await assert.rejects(inst.install({ gameId: 'sample-game', gameDir: game, zipPath: p }), { code: 'BAD_MANIFEST' });
  assert.ok(!fs.existsSync(path.join(root, 'evil.txt')));
});

test('failed install rolls back', async () => {
  const { root, game, inst } = setup();
  const { outPath } = await buildPack(root);
  // Files are written in order fonts/hebrew.ttf, strings.txt. Make strings.txt read-only
  // so the second write fails after the first has landed.
  const target = path.join(game, 'data/strings.txt');
  fs.chmodSync(target, 0o444);
  try {
    await assert.rejects(inst.install({ gameId: 'sample-game', gameDir: game, zipPath: outPath }));
  } finally { fs.chmodSync(target, 0o666); }
  assert.match(read(target), /Start Game/);
  assert.ok(!fs.existsSync(path.join(game, 'data/fonts')), 'added file and folder rolled back');
  assert.equal(inst.getRecord('sample-game'), null);
  assert.ok(!fs.existsSync(inst.backupDir('sample-game')));
});

test('VDF parser reads libraryfolders format', () => {
  const v = parseVdf('"libraryfolders"\n{\n "0"\n {\n  "path"  "C:\\\\Program Files (x86)\\\\Steam"\n  "apps" { "570" "123" }\n }\n "1" { "path" "D:\\\\SteamLibrary" }\n}');
  assert.equal(v.libraryfolders['0'].path, 'C:\\Program Files (x86)\\Steam');
  assert.equal(v.libraryfolders['1'].path, 'D:\\SteamLibrary');
});
