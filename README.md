# Tirgumim Launcher

Windows launcher that installs Hebrew translation packs into games, with a backup of every game file a pack
replaces and a full restore on uninstall. Hebrew, right-to-left interface.

Built with **Tauri 2**: the work is done in Rust (`src-tauri/`), the window is React + TypeScript (`src/`).
Its structure follows the [Multiplayer Everything launcher](https://github.com/multiplayereverything/multiplayer-mods-launcher)
(MIT); the code here is our own.

See [DESIGN.md](DESIGN.md) for the catalog and pack formats, and [PLAN.md](PLAN.md) for what's next.

## Needs

- Node.js 20 or newer
- Rust (stable, via rustup) and the Visual Studio C++ build tools
- WebView2 (built into Windows 11)

## Run

```
npm install
npm run dev:setup        # builds the sample game, the sample pack and dev/catalog.json
npm run app:dev-catalog  # the launcher with the local test catalog (sample game first)
npm run app              # the launcher with the real catalog from GitHub
npm test                 # Rust tests: installs, uninstalls, packs, GitHub releases, detection
npm run release          # the Windows installer (src-tauri/target/release/bundle/nsis/)
```

## Make a pack

1. Put the files exactly as they go under the game root in a folder, e.g. `mypack/files/...`.
2. Write `mypack/pack.config.json` (every manifest field except `files`, plus `filesDir`).
   See `dev/sample-pack/pack.config.json`.
3. `npm run make-pack -- mypack/pack.config.json --game-dir "<clean game install>" --out out/pack.zip`
4. Publish it. Either:
   - **GitHub release (easiest):** upload the zip to a release on `NamiSwan222/tirgumim` with a tag like
     `pbz-0.1.0`. If the game's catalog entry has `"pack": { "github": "NamiSwan222/tirgumim", "tag": "pbz-*", "asset": "pbz-he-*.zip" }`,
     the launcher finds the new version by itself, with the checksum GitHub publishes. No catalog edit.
   - **Written out:** paste the `pack` entry make-pack prints (url, sha256, size, version) into `catalog/catalog.json`.

## Layout

- `src-tauri/src/core/`: installer, manifest checks, Steam/Epic detection, catalog and GitHub releases, pack tools
  (no window; `cargo test`)
- `src-tauri/src/lib.rs`: the commands the window calls, the `poster` protocol for game art, progress events
- `src/`: the Hebrew window (title bar, sidebar, home, game page, status bar)
- `catalog/catalog.json`: the list of our translations (the launcher reads it from GitHub; this copy also ships with the app)
- `dev/`: sample game and sample pack for testing

User data: `%APPDATA%\Tirgumim Launcher\` (`data\state.json`, `data\backups\`, `data\sources.json`, `posters\`).
The test catalog uses `dev-data\`.
