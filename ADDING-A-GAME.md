# Adding a game (no launcher update needed)

The launcher knows nothing about any game. It finds translations by itself in our GitHub repos: **a new game is a
repo with the topic `tirgumim-pack`, a `pack/catalog-entry.json` file and a release**. No catalog commit, no
launcher update. Players see it within 10 minutes, or right away with **Ctrl+F5**. `check.yml` fails if game-specific code ever enters the app
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
2. In the game's repo (owner `NamiSwan222`, public), commit `pack/catalog-entry.json` on the default branch:
   `id`, `name`, `nameHe`, `aboutHe`, `engine`, `detect` (`steamAppId` / `epicAppName`, `markerFile`,
   `processNames`), and optionally `pack: { tag, asset }` (default: every release of this repo, first `.zip`).
   Add `"minLauncherVersion"` if no released launcher can install it yet.
3. Add the topic **`tirgumim-pack`** to the repo (GitHub: the gear next to "About"). That is what makes the launcher
   find it. Removing the topic, or archiving the repo, takes the game out.
4. Check it: `npm run check-catalog -- --online --discover` in the launcher repo (GitHub runs the same on every
   push there). It reads every tagged repo's entry with the strict rules and checks its release has a checksum.

`catalog/catalog.json` still works and ships inside the app as the offline fallback; an entry in a repo replaces a
catalog entry with the same `id`. Only repos of `NamiSwan222` count, whatever topics others use.

Run the game-translation risk check first (free-only vs paid).

**A paid translation** (only after the risk check says it may be sold): the entry gets `"paid": true`,
`"minLauncherVersion"` of the first launcher with accounts, and no `pack`. Keep its repo **private** (a public repo
gives away the translation files) and put the entry in `catalog/catalog.json` instead (discovery only sees public
repos); never publish the zip as a public release. The zip goes to the account server (`PUT /v1/admin/packs/<gameId>`) and its price to
`PUT /v1/admin/products/<gameId>`. See ENTITLEMENTS.md.
