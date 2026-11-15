# Adding a game (no launcher update needed)

The launcher knows nothing about any game. A new game is a **catalog entry** plus a **pack release**; players see it
within 10 minutes (or right away with רענון). `check.yml` fails if game-specific code ever enters the app
(`scripts/no-game-code.mjs`).

## 1. Pick how the translation turns on

The user's rule: Hebrew only when started from the launcher; a Steam launch is the original game.

| The game can... | Use | Example |
|---|---|---|
| load other data files from the command line | `activation.mode: "args"`, patches with `output` (a new file next to the original) | Undertale: `-game data_he.win`, via steam |
| load a DLL from its folder (most Windows games: `xinput*.dll`, `dinput8.dll`, `version.dll`, `winmm.dll`) | `"args"` + a pack-supplied proxy DLL that does nothing unless `env` is set; Hebrew copies via `output` (bsdiff, or `append` for huge files) | Yu-Gi-Oh: `xinput9_1_0.dll` + `TIRGUMIM_LANG=he`, via direct + `needsSteam` |
| neither | `"swap"` (files swapped in for the session; see the crash limit in DESIGN.md) | — |

With `via: "steam"`, args can't contain `{gameDir}` or spaces (Steam drops quotes): use names relative to the game
folder. Env vars need `via: "direct"`.

The proxy-DLL pattern covers almost any engine, so new games should not need new launcher features. If one ever
does, the pack sets `minLauncherVersion` and older launchers say "update the launcher" (never "broken").

## 2. Build the pack

`npm run make-pack -- <pack.config.json> --game-dir "<clean install>" --out out/<game>-he-<ver>.zip`
(see README, "Make a pack"). It fills in hashes and `minLauncherVersion`, and prints a line for the release notes.

## 3. Publish

1. Release on the game's repo, tag `<game>-<ver>`, asset `<game>-he-<ver>.zip`. Paste the
   `tirgumim-min-launcher: X` line make-pack printed into the release notes, so older launchers keep the last
   release they can install.
2. Add the game to `catalog/catalog.json`:
   `id`, `name`, `nameHe`, `aboutHe`, `engine`, `detect` (`steamAppId` / `epicAppName`, `markerFile`,
   `processNames`), `pack: { github, tag, asset }`. Add `"minLauncherVersion"` if no released launcher can install
   it yet (players then see "update the launcher"). `npm run check-catalog` must pass.
3. Push. GitHub's check runs the same catalog check, including that the release exists with a checksum.

Run the game-translation risk check first (free-only vs paid).
