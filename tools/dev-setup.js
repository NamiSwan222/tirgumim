#!/usr/bin/env node
'use strict';
// Builds the dev sample: a fresh sample game folder, the sample pack, and dev/catalog.json.
const fs = require('fs');
const path = require('path');
const { makePack } = require('./make-pack');

const dev = path.join(__dirname, '..', 'dev');
(async () => {
  const game = path.join(dev, 'sample-game');
  fs.rmSync(game, { recursive: true, force: true });
  fs.cpSync(path.join(dev, 'sample-game-template'), game, { recursive: true });
  const { outPath, entry } = await makePack({
    configPath: path.join(dev, 'sample-pack', 'pack.config.json'),
    gameDir: path.join(dev, 'sample-game-template'),
    out: path.join(dev, 'packs', 'sample-game-he-0.1.0.zip'),
    url: 'packs/sample-game-he-0.1.0.zip',
  });
  const catalog = {
    schema: 1,
    games: [
      {
        id: 'sample-game', name: 'Sample Game', nameHe: 'משחק דוגמה', engine: 'custom',
        detect: { markerFile: 'SampleGame.exe' },
        devPath: game,
        pack: { ...entry, changelog: 'חבילת בדיקה' },
      },
      ...JSON.parse(fs.readFileSync(path.join(__dirname, '..', 'catalog', 'catalog.json'), 'utf8')).games,
    ],
  };
  fs.writeFileSync(path.join(dev, 'catalog.json'), JSON.stringify(catalog, null, 2));
  console.log(`Sample game: ${game}\nPack: ${outPath}\nCatalog: ${path.join(dev, 'catalog.json')}`);
})().catch((e) => { console.error(e); process.exit(1); });
