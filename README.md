# Tirgumim Launcher

Windows launcher (Electron) that installs Hebrew translation packs into games, with
backup and restore of the original files. Hebrew RTL interface.

See [DESIGN.md](DESIGN.md) for the catalog and pack formats.

## Run

```
npm install
npm run dev:setup      # builds the sample game, sample pack and dev/catalog.json
npm run dev            # launcher using the local dev catalog
npm start              # launcher using the real catalog URL (src/main/main.js)
npm test               # core install/uninstall tests
```

## Make a pack

1. Put the files exactly as they go under the game root in a folder, e.g. `mypack/files/...`.
2. Write `mypack/pack.config.json` (all manifest fields except `files`, plus `filesDir`).
   See `dev/sample-pack/pack.config.json`.
3. `npm run make-pack -- mypack/pack.config.json --game-dir "<clean game install>" --out out/pack.zip --url <release download URL>`
4. Upload the zip to a GitHub Release and paste the printed `pack` entry into `catalog.json`.

## Layout

- `src/core/` — installer, manifest checks, Steam/Epic detection, catalog download (no Electron; tested with `node --test`)
- `src/main/` — Electron main process + preload (IPC)
- `src/renderer/` — Hebrew UI
- `tools/` — `make-pack.js`, `dev-setup.js`
- `dev/` — sample game + sample pack for testing

User data: `%APPDATA%\Tirgumim Launcher\data` (`state.json`, `backups\`). Dev mode uses `dev-data`.
