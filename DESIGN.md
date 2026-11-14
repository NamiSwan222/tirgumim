# Tirgumim Launcher — Design (v0.1)

Windows desktop app (Electron) that installs Hebrew translation packs into games.
UI is Hebrew, right-to-left. No login in v0.1 (packs carry a `requiresAccount`
flag so accounts can be added later without changing the pack format).

## 1. Pieces

| Piece | Where | What |
|---|---|---|
| **Catalog** | `catalog.json` on GitHub (raw URL, configurable) | List of supported games, how to find them, and the latest pack for each |
| **Pack** | `.zip` on a GitHub Release | `tirgumim-pack.json` manifest + `files/` tree |
| **Launcher** | this repo | Finds games, downloads/verifies packs, installs with backup, restores |
| **make-pack tool** | `tools/make-pack.js` | Builds a pack zip from a folder, fills in hashes, prints the catalog entry |

## 2. Catalog format (`catalog.json`)

```json
{
  "schema": 1,
  "games": [
    {
      "id": "phantom-blade-zero",
      "name": "Phantom Blade Zero",
      "nameHe": "פנטום בלייד זירו",
      "engine": "unreal",
      "detect": {
        "steamAppId": "4115450",
        "epicAppName": null,
        "markerFile": "platform/steam/PBZ.exe"
      },
      "pack": {
        "version": "0.1.0",
        "url": "https://github.com/<org>/<repo>/releases/download/pbz-0.1.0/pbz-he-0.1.0.zip",
        "sha256": "…",
        "size": 123456,
        "changelog": "גרסה ראשונה"
      }
    }
  ]
}
```

- `detect.markerFile` is a path relative to the game root that must exist; it is
  how a Steam/Epic/custom folder is confirmed to be the right game.
- `pack.url` may be absolute (`https://`, `file://`) or relative to the catalog URL
  (used by the local dev catalog).
- The zip's SHA-256 is checked after download, before anything is extracted.

## 3. Pack format (`.zip`)

```
tirgumim-pack.json
files/
  <paths exactly as they go under the game root>
```

### Manifest (`tirgumim-pack.json`)

```json
{
  "schema": 1,
  "packId": "phantom-blade-zero-he",
  "gameId": "phantom-blade-zero",
  "version": "0.1.0",
  "title": "תרגום עברי ל-Phantom Blade Zero",
  "authors": ["Tirgumim"],
  "engine": "unreal",
  "minLauncherVersion": "0.1.0",
  "requiresAccount": false,
  "gameVersions": [
    {
      "name": "1.0.0 (Steam)",
      "checks": [
        { "path": "platform/steam/PBZ.exe", "sha256": "…" }
      ]
    }
  ],
  "allowUnknownGameVersion": false,
  "files": [
    { "path": "ProjectPBZ/Content/Paks/~mods/zz_hebrew_P.pak", "sha256": "…", "size": 1048576 }
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

## 4. Install / update / uninstall

State lives in `%APPDATA%\Tirgumim Launcher\` (`state.json` + `backups\<gameId>\…`).

**Install**
1. Download zip → verify zip SHA-256 → read & validate manifest.
2. Check `gameId`, launcher version, game version.
3. Extract to a temp folder, verify every file's hash.
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

**Update** = uninstall old version, then install new. If the new install fails it
rolls itself back, so the game is left original (and the UI says so).

**Status** shown per game: not found / found / installed / update available /
needs repair (some of our files were overwritten — usually after a game update).

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
- **Posters**: catalog `poster` URL if set; otherwise Steam's own cached cover on this PC
  (`Steam\appcache\librarycache\<appid>\…\library_capsule.jpg`); otherwise downloaded once from
  Steam's CDN and cached in `%APPDATA%\Tirgumim Launcher\posters`. Epic games are matched to
  their Steam page by name for the cover. Missing covers show the game's name.
- **Catalog source**: the GitHub URL (default or set in Settings); if it can't be reached, the copy
  bundled in the launcher (`catalog/catalog.json`) is used and the UI says so.
- Large files in version checks are hashed once per run and cached by size + modified time.

## 7. Later
- Accounts / paid packs: `requiresAccount` packs will need a signed download URL
  from our server instead of a public GitHub link. Pack format stays the same.
- Signed catalog (ed25519) so a hijacked GitHub account can't push malicious packs.
- Installer build (`electron-builder`, NSIS) and launcher auto-update.
