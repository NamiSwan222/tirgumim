# Tirgumim Launcher — Design (v0.2)

Windows desktop app (Tauri 2: Rust core in `src-tauri/`, React + TypeScript window in `src/`) that installs
Hebrew translation packs into games. v0.1 was Electron; v0.2 is a rewrite with the same catalog, pack and
`state.json` formats, so installs made by v0.1 carry over.
UI is Hebrew, right-to-left. No login in v0.1 (packs carry a `requiresAccount`
flag so accounts can be added later without changing the pack format).

## 1. Pieces

| Piece | Where | What |
|---|---|---|
| **Catalog** | `catalog.json` on GitHub (raw URL, configurable) | List of supported games, how to find them, and the latest pack for each |
| **Pack** | `.zip` on a GitHub Release | `tirgumim-pack.json` manifest + `files/` tree |
| **Launcher** | this repo | Finds games, downloads/verifies packs, installs with backup, restores |
| **make-pack tool** | `npm run make-pack` (`src-tauri/src/core/tools.rs`) | Builds a pack zip from a folder, fills in hashes, prints the catalog entry |

## 1b. Where the list of translations comes from (0.4.1+)

1. **Our GitHub repos.** One unauthenticated search finds every repo of `NamiSwan222` with the topic
   `tirgumim-pack` (archived repos are left out). Each repo's `pack/catalog-entry.json` (raw.githubusercontent.com, no
   API limit) is its game entry, in the catalog format below; `pack` defaults to the repo's own releases. Kept for
   10 minutes in `data\discovered.json`; Ctrl+F5 / רענון asks again (not sooner than 10 s); if GitHub can't be
   reached, the last answer is used. An entry the launcher can't read is skipped.
2. **`catalog.json`** from this repo (or the copy inside the app when offline), for anything not in a tagged repo.
   A repo entry replaces a catalog entry with the same `id`.

So adding a game needs neither a catalog commit nor a launcher update (see ADDING-A-GAME.md).
`npm run check-catalog -- --online --discover` checks the tagged repos with the strict rules; GitHub runs it on
every push.

## 2. Catalog format (`catalog.json`)

```json
{
  "schema": 1,
  "games": [
    {
      "id": "my-game",
      "name": "My Game",
      "nameHe": "המשחק שלי",
      "engine": "unreal",
      "aboutHe": "תרגום מלא של התפריטים, הכתוביות והדיאלוגים.",
      "detect": {
        "steamAppId": "1234560",
        "epicAppName": null,
        "markerFile": "Binaries/Win64/MyGame.exe",
        "processNames": ["MyGame.exe", "MyGame-Win64-Shipping.exe"]
      },
      "pack": { "github": "NamiSwan222/tirgumim", "tag": "mygame-*", "asset": "mygame-he-*.zip" },
      "requires": []
    }
  ]
}
```

- `detect.markerFile` is a path relative to the game root that must exist; it is
  how a Steam/Epic/custom folder is confirmed to be the right game.
- `detect.processNames`: the game's programs. While one runs, install, update and uninstall are refused
  ("close the game first") and the game page says so.
- `requires`: files the game folder must have (or must not have, with `"absent": true`) before the
  pack can go in, each with a Hebrew `message` and an optional https `url`. Shown on the game page and
  checked before downloading. A manifest can list `requires` too (checked at install).
- `poster` / `hero`: optional cover (2:3) and wide art URLs; otherwise Steam's.

**Where the pack is.** Two forms:
- **From a GitHub release** (`github`, `tag`, `asset`, optional `prerelease`): the launcher lists the
  repository's releases, keeps those whose tag and file name match the patterns (`*` and `?`), takes the
  highest version (the rightmost number run in the tag: `mygame-0.2.1` → `0.2.1`) and uses the SHA-256 GitHub
  publishes for the file (`digest`). No checksum from GitHub means no install. The release notes become the
  changelog unless `changelog` is set. Release lists are cached in `data\sources.json` for 10 minutes
  (refresh asks again, but not sooner than 30 seconds); if GitHub can't be reached, the last list is used.
  Without a login GitHub allows 60 requests an hour, and one repository serves every game.
- **Written out**: `version`, `url`, `sha256`, `size`, `changelog`. `url` may be absolute (`https://`,
  `file://`) or relative to the catalog URL (used by the local dev catalog).

The zip's SHA-256 is checked after download, before anything is extracted.

**Releases that need a newer launcher.** A release whose notes contain the line `tirgumim-min-launcher: 0.4.0` is
skipped by older launchers (from 0.4.0 on), which keep offering the newest release they can install. make-pack
prints the line. Launcher 0.3.0 doesn't read it: it offers the newest release and, after download, says the
launcher must be updated (0.3.0 updates itself from the status bar).

## 3. Pack format (`.zip`)

```
tirgumim-pack.json
files/
  <paths exactly as they go under the game root>
patches/              (optional)
  <game path>.bsdiff
```

### Manifest (`tirgumim-pack.json`)

```json
{
  "schema": 1,
  "packId": "my-game-he",
  "gameId": "my-game",
  "version": "0.1.0",
  "title": "תרגום עברי ל-My Game",
  "authors": ["Tirgumim"],
  "engine": "unreal",
  "minLauncherVersion": "0.1.0",
  "requiresAccount": false,
  "gameVersions": [
    {
      "name": "1.0.0 (Steam)",
      "checks": [
        { "path": "Binaries/Win64/MyGame.exe", "sha256": "…" }
      ]
    }
  ],
  "allowUnknownGameVersion": false,
  "files": [
    { "path": "MyGame/Content/Paks/~mods/zz_hebrew_P.pak", "sha256": "…", "size": 1048576 }
  ]
}
```

Rules:
- **Engine-agnostic.** A pack is just "these files go at these paths under the game
  root". Unreal `.pak` mods, Unity asset replacements, loose text files, font files,
  or a DLL loader all fit. `engine` is informational only.
- **Paths** are relative, forward slashes, no `..`, no drive letters. Anything else
  is rejected (zip-slip protection).
- **Per-file hashes** are checked after extraction and used later to tell whether a
  file on disk is still ours.
- **Game version checks.** `gameVersions` lists the builds the pack was made for.
  A version matches when every check passes (`sha256` and/or `size` of a file, or
  just `exists`). If none matches, install is blocked, unless
  `allowUnknownGameVersion` is true, in which case the user gets a warning and can
  continue. An empty/missing `gameVersions` means "any version".
- `minLauncherVersion` blocks packs that need a newer launcher.
- Each file is either **added** (didn't exist) or **replaced** (existed — original is
  backed up first). The launcher decides this at install time; the pack doesn't say.

### Binary patches (`patches`, launcher 0.3.0+)

For a big game file that only needs some bytes changed (a GameMaker `data.win`), a pack can carry a
bsdiff patch instead of the whole file, so it ships no copy of the game's own data:

```json
"patches": [
  { "path": "data.win", "fromSha256": "…", "toSha256": "…", "size": 123456789, "patch": "patches/data.win.bsdiff" }
]
```

- `path`: the game file (same rules as `files`; a path is in `files` or `patches`, not both). `patch`: the
  patch inside the zip, under `patches/`. Format: **BSDIFF40** (classic bsdiff 4 with bzip2; the `bsdiff` tool and
  the Rust `qbsdiff` crate both write it).
- `fromSha256` is the untouched game file. If the game's file is anything else (another game build, or already
  patched), the install is refused with `PATCH_SOURCE`, even when forced: a patch only fits its exact original.
- The patch is applied in memory in a temp folder and the result checked against `toSha256`
  (`PATCH_FAILED` otherwise) before anything in the game changes. Then the patched file goes in like a replaced
  one: original backed up, uninstall restores it, a game update that replaced it is left alone.
- `files` may be empty when `patches` isn't. A pack with `patches` must have `minLauncherVersion` 0.3.0 or newer:
  older launchers don't know the field and would skip it. make-pack sets this.
- Patching holds the original and the result in memory (a few hundred MB for a big `data.win` is fine).

### Launch-only packs (`activation`, launcher 0.4.0+)

Hebrew only when the game is started from the launcher ("שחק בעברית"); started from Steam or anywhere else, the
game is the original. Without `activation` a pack is permanent, exactly as before.

```json
"activation": {
  "mode": "args",                       // or "swap"
  "exe": "UNDERTALE.exe",               // relative to the game root; must exist
  "args": ["-game", "data_he.win"],     // via steam: relative names only (see below)
  "workingDir": "{gameDir}",            // optional, direct launches only; default the exe's folder
  "via": "steam",                       // or "direct"; default "steam"
  "env": { "TIRGUMIM_LANG": "he" },     // optional, needs via "direct" (Steam drops env vars)
  "needsSteam": true                    // optional, via "direct": refuse with a Hebrew message until Steam runs
}
```

- **`args`**: the pack only **adds** files (installing refuses anything that would replace a game file,
  `WOULD_REPLACE`). A bsdiff patch writes its result to a new file with `"output": "data_he.win"`, leaving `path`
  untouched. Play starts the game with `args`, so only that start loads the Hebrew files. For engines that can be
  pointed at other files (GameMaker `-game`), or that load a pack-supplied proxy DLL which does nothing unless
  an `env` variable is set (Yu-Gi-Oh: `xinput9_1_0.dll` + `TIRGUMIM_LANG=he`, redirecting to Hebrew copies). An
  `append` patch with `output` makes such a copy on the player's PC: the original followed by `data`, streamed,
  hashed as written; the original is only read. **Preferred**: no game file is ever changed, so a Steam launch is
  the original even after a crash.
- **`swap`**: install keeps every Hebrew file in `<game>/.tirgumim/he/` and changes no game file. Play:
  1. checks the game is closed (`GAME_ALREADY_RUNNING` otherwise: never swap under a running game), and that the
     game's files are still the ones the pack was prepared on (`GAME_CHANGED`);
  2. writes `.tirgumim/active.json`, then renames the originals to `.tirgumim/orig/` and ours into place (instant,
     same drive), or appends the bytes of an `append` patch;
  3. starts the game and watches `detect.processNames` (it must appear within 3 minutes; it is over when none has
     run for 15 s, so a game that restarts itself through Steam is still covered);
  4. puts the originals back and removes the journal. Restoring looks only at the files on disk, so it is safe
     after a crash at any step and can run again; a file the game replaced meanwhile (an update) is left alone.
  **Limit**: while swapped in, the real files are Hebrew. If the PC or launcher crashes during play and the game
  is then started from Steam before the launcher opens again, it is Hebrew once; and Steam's "verify integrity"
  during a session re-downloads the files. Use swap only for games that can't be redirected.
  The window can't be closed while a game is swapped in: closing hides it and the launcher quits after restoring.
  A second launcher start shows the first one. If the launcher dies anyway, its next start restores (or, if the
  game still runs, watches it again).
- **`via: "steam"`** runs `steam.exe -applaunch <catalog detect.steamAppId> <args>`: Steam keeps the overlay,
  playtime, achievements and DRM, and a game that would relaunch itself through Steam when started directly
  doesn't need to. Steam starts the game in its own folder but **drops the quotes** around arguments, so with
  `via: "steam"` the args may not contain `{gameDir}` or spaces (refused): use names relative to the game folder
  (tested on Undertale: `-applaunch 391540 -game data_he.win`). No `steam_appid.txt` or DRM workarounds. Steam not installed: `STEAM_MISSING`; not running:
  Steam is started first. **`via: "direct"`** starts `exe` itself (games without Steam, or known to run directly).
- **`append` patches**, for very big files. In args packs with `output` (a new file, see above); in swap packs:
  `{ "path": "YGO_DATA.dat", "kind": "append", "fromSha256", "fromSize", "toSha256", "size", "data": "patches/YGO_DATA.dat.append" }`.
  The result is the original followed by `data`. Nothing big is copied: play appends, restore cuts the file back to
  `fromSize` and checks `fromSha256`. Install checks `toSha256` by streaming (no extra RAM).
- A pack with `activation`, `append` or `output` needs `minLauncherVersion` 0.4.0 (0.3.0 refuses it instead of
  installing it for good). `.tirgumim/` belongs to the launcher; packs can't put files there.
- An update from a permanent pack to a launch-only one goes through the normal update (the permanent files are
  removed and the originals restored first).

## 4. Install / update / uninstall

State lives in `%APPDATA%\Tirgumim Launcher\` (`state.json` + `backups\<gameId>\…`).

**Install**
0. Refused while one of the game's `processNames` runs, or while catalog `requires` aren't met.
1. Download zip → verify zip SHA-256 → read & validate manifest.
2. Check `gameId`, launcher version, game version, that each patched file is its `fromSha256` original, manifest `requires`.
3. Extract to a temp folder, verify every file's hash; make each patched file there and verify its `toSha256`.
4. For each file: if the target exists, copy it to `backups\<gameId>\<path>`.
5. Copy pack files into the game folder.
6. Write the install record (pack version, game dir, each file's hash, whether it
   was backed up).
   If any step from 4 on fails, everything written so far is rolled back.

**Uninstall (restore)**
For each file in the record:
- file on disk still matches our hash → restore the backup, or delete if it was added;
- file was changed since (e.g. a Steam update replaced it) → leave the game's new
  file alone and discard our stale backup.
Then delete the record and backup folder.

**Update**: the new pack is opened and checked first (game, launcher version, game version,
requires); only then is the old version uninstalled and the new one installed. If the new install
fails it rolls itself back, so the game is left original (and the UI says so).

**Status** shown per game: not found / translation in progress / not available right now (GitHub
couldn't be asked) / ready / installed / update available / needs repair (some of our files were
overwritten, usually after a game update), plus "game is open" while it runs.

## 5. Finding games

- **Steam**: `HKCU\Software\Valve\Steam\SteamPath` → `steamapps\libraryfolders.vdf`
  → each library's `appmanifest_<appid>.acf` → `installdir`.
- **Epic**: `C:\ProgramData\Epic\EpicGamesLauncher\Data\Manifests\*.item` (JSON,
  `AppName` / `InstallLocation`).
- **Custom**: user picks a folder; it must contain `markerFile`. Saved in state.
A custom path wins over auto-detection.

## 6. Games on the PC and posters (v0.2)

- The launcher lists every installed Steam game (all `appmanifest_*.acf` in all libraries) and
  every Epic game (`.item` manifests). Games in our catalog get the full card; the rest are
  shown as "on your PC, no translation yet".
- **Art**: a cover (`library_600x900` / `library_capsule`) and a wide hero (`library_hero`) per game.
  Catalog `poster` / `hero` URL if set; otherwise Steam's own cached art on this PC
  (`Steam\appcache\librarycache\<appid>\…`); otherwise downloaded once from Steam's CDN and cached in
  `%APPDATA%\Tirgumim Launcher\posters`. Epic games are matched to their Steam page by name. The
  window loads art through the `poster` protocol (`http://poster.localhost/<cover|hero>/<steam|epic|game>/<id>`),
  so nothing outside that cache is exposed to the page. Missing art falls back to a blurred cover, then
  to the game's name.
- **Catalog source**: the GitHub URL (default or set in Settings); if it can't be reached, the copy
  bundled in the launcher (`catalog/catalog.json`) is used and the UI says so.
- Large files in version checks are hashed once per run and cached by size + modified time.
- The window asks for the list again every 15 seconds (cached answers, so cheap), which notices a game
  being opened or closed.

## 7. The window (v0.2)

Its own title bar (search, Ctrl+K; refresh, F5; window
buttons), a sidebar of categories with counts (home, our translations, installed, updates, coming soon,
games on this PC, settings), a home page with a rotating banner and shelves that page sideways, a game
page with wide art fading into a blurred copy, four fact boxes, one main button with the rest under
"More", and a "before installing" list of what still blocks the install. Messages float above the
status bar, which shows where the catalog came from.

## 8. Later
- Accounts / paid packs: `requiresAccount` packs will need a signed download URL
  from our server instead of a public GitHub link. Pack format stays the same.
- Signed catalog (ed25519) so a hijacked GitHub account can't push malicious packs.
- Launcher self-update (Tauri updater, signed with an offline key).
