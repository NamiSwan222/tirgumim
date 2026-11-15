# Tirgumim Launcher

Windows launcher that installs Hebrew translation packs into games, with a backup of every game file a pack
replaces and a full restore on uninstall. Hebrew, right-to-left interface.

Built with **Tauri 2**: the work is done in Rust (`src-tauri/`), the window is React + TypeScript (`src/`).

See [DESIGN.md](DESIGN.md) for the catalog and pack formats, [ADDING-A-GAME.md](ADDING-A-GAME.md) for adding a game
(no launcher update needed), and [PLAN.md](PLAN.md) for what's next.

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
npm run release          # the Windows installer (src-tauri/target/release/bundle/nsis/), unsigned for updates
npm run check-catalog    # the same catalog check GitHub Actions runs
```

## Releasing the launcher

GitHub Actions builds and publishes every version, and installed launchers update themselves.

```
npm run publish-launcher -- 0.3.0 "מה חדש בגרסה הזו"
```

This sets the version in every file, runs the tests, commits, tags `v0.3.0` and pushes. Then
`.github/workflows/release.yml` builds the installer on Windows, signs it for the updater, publishes release
`v0.3.0` and copies its `latest.json` to the fixed `updater` release. Every launcher checks that file at start, every hour and when
the player presses "בדוק עדכונים" (status bar). By default it downloads the new version in the background and shows a
banner with "הפעל מחדש ועדכן"; with automatic updates off (Settings) the banner offers "עדכן עכשיו". The signature is
checked before installing. The tag message is what players see under "מה חדש". Not in the test catalog mode.

**Signing key (one time).** Updates are signed with our own key, so nobody else can push an update to players.
1. `npx tauri signer generate -w "%USERPROFILE%.tauri	irgumim.key"` (a password is optional). Keep the
   `.key` file and the password safe and out of the repo: if they are lost, installed launchers can't be updated
   anymore.
2. Put the contents of `tirgumim.key.pub` in `src-tauri/tauri.conf.json` → `plugins.updater.pubkey`.
3. On GitHub: repo → Settings → Secrets and variables → Actions → add `TAURI_SIGNING_PRIVATE_KEY` (the contents of
   `tirgumim.key`) and, if you set one, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`.

Launchers from before the updater (0.2.0) can't update themselves; install the first updater version by hand.

**Catalog check.** `.github/workflows/check.yml` runs on every push and pull request: the catalog with the
launcher's own parser and rules (`src-tauri/src/core/check.rs`), whether each GitHub-release pack really has a
release with a checksum, the tests, and the window build. Pack releases in this repo need their own tag pattern
(not `v*` or `updater`); the check enforces that.

## Make a pack

1. Put the files exactly as they go under the game root in a folder, e.g. `mypack/files/...`.
2. Write `mypack/pack.config.json` (every manifest field except `files`, plus `filesDir`).
   See `dev/sample-pack/pack.config.json`.
3. `npm run make-pack -- mypack/pack.config.json --game-dir "<clean game install>" --out out/pack.zip`

   **Patching a big game file instead of shipping it** (e.g. a GameMaker `data.win`): add to the config
   `"patches": [{ "path": "data.win", "patched": "work/data.win" }]`, where `work/data.win` is your changed copy
   (relative to the config). make-pack reads the original from `--game-dir`, writes a bsdiff into the pack, checks
   that it rebuilds your file exactly, and fills in the hashes and `minLauncherVersion` (0.3.0). If you already
   have a bsdiff (BSDIFF40), use `"patchFile": "work/data.win.bsdiff"` instead. The `files/` folder is optional
   then. See DESIGN.md, "Binary patches".

   **Launch-only (Hebrew only when started from the launcher):** add `"activation"` to the config
   (DESIGN.md, "Launch-only packs"). For an args pack, give the patch `"output": "data_he.win"` so the original
   stays untouched. For a very big file in a swap pack, use
   `{ "path": "YGO_DATA.dat", "kind": "append", "appendFile": "work/added.bin" }` (streamed, never loaded into
   memory). make-pack sets `minLauncherVersion` 0.4.0. Making the bsdiff of a file of a few hundred MB takes a while and
   several GB of memory.
4. Publish it. Either:
   - **GitHub release (easiest):** upload the zip to a release on `NamiSwan222/tirgumim` with a tag like
     `mygame-0.1.0`. If the game's catalog entry has `"pack": { "github": "NamiSwan222/tirgumim", "tag": "mygame-*", "asset": "mygame-he-*.zip" }`,
     the launcher finds the new version by itself, with the checksum GitHub publishes. No catalog edit.
   - **Written out:** paste the `pack` entry make-pack prints (url, sha256, size, version) into `catalog/catalog.json`.

## Layout

- `src-tauri/src/core/`: installer, manifest checks, Steam/Epic detection, catalog and GitHub releases, pack tools,
  the catalog check (no window; `cargo test`)
- `src-tauri/src/lib.rs`: the commands the window calls, the `poster` protocol for game art, progress events
- `src/`: the Hebrew window (title bar, sidebar, home, game page, status bar)
- `catalog/catalog.json`: the list of our translations (the launcher reads it from GitHub; this copy also ships with the app)
- `src/updater.ts`: the launcher updating itself
- `.github/workflows/`: `release.yml` (build and publish on a `v*` tag), `check.yml` (catalog check and tests)
- `scripts/`: `publish-launcher.mjs`, `versions.mjs` (the version is in five files)
- `dev/`: sample game and sample pack for testing

User data: `%APPDATA%\Tirgumim Launcher\` (`data\state.json`, `data\backups\`, `data\sources.json`, `posters\`).
The test catalog uses `dev-data\`.
