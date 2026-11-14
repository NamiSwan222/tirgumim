# Plan

**Status (2026-11-14):** v0.2 (Tauri: Rust + React + TypeScript) and all of stage 1 are done: GitHub
release packs with a 10-minute cache, "close the game first", `requires`, and the byte-identical
install/uninstall test (`npm test`). Stage 2's installer exists (`npm run release`, NSIS, Hebrew);
self-update and the signed catalog are still to do.

## Stage 1: ready for the first real translation (Phantom Blade Zero), done

| # | What | How |
|---|---|---|
| 1 | **Pack straight from a GitHub release** | Catalog `pack` can be `{ "github": "NamiSwan222/tirgumim", "tag": "pbz-*", "asset": "pbz-he-*.zip" }`. The launcher finds the newest matching release and takes its version, URL, size and SHA-256 (GitHub's `digest`). Publishing a new version = upload a release; no catalog edit. The written-out form (url + sha256) still works. No digest from GitHub means no install. |
| 2 | **Go easy on GitHub** | GitHub allows 60 requests an hour without a login. Release lists are kept in `sources.json` and reused for 10 minutes; the רענון (refresh) button asks again; if GitHub can't be reached, the last answer is used. |
| 3 | **The game must be closed** | `detect.processNames` in the catalog (e.g. `PBZ.exe`). Install, update and uninstall say "close the game first". |
| 4 | **Required files** | `requires: [{ "path", "message" }]` in the catalog or manifest, e.g. if a translation needs a mod loader. Install is off while it's missing, with the message shown. |
| 5 | **Test on a copy of the game** | `npm test` copies the sample game, installs the pack, uninstalls it, and compares every file's hash with the start. Also run once by hand on a copy of the real PBZ folder. |

## Stage 2: giving the launcher to other people

| # | What | How |
|---|---|---|
| 6 | **Installer** (done) | `npm run release` builds a Tauri NSIS installer with Hebrew screens and a Start menu shortcut. Next: upload it to a GitHub Release. |
| 7 | **Self-update** | Tauri updater, `latest.json` from our newest GitHub release, signed with our own key. The private key stays on your PC, never in the repo. |
| 8 | **Signed catalog** | The same key signs `catalog.json` (`catalog.json.sig`). A catalog with a bad signature is ignored and the last good one is used. Protects players if the GitHub account is taken over. |

## Stage 3: nice to have (later)

- A first-run screen, step by step.
- Error reports to a small server (a free Cloudflare Worker is enough).
- Light theme, compact cards.

## Open decision: a license for our repo

The repo `NamiSwan222/tirgumim` has none, so nobody may reuse it. Options:
- **MIT**: anyone may use it. Fine even with paid accounts later, because what's paid is the translations, not the launcher code.
- **No license** (all rights reserved): only we may use the code.
- **GPL-3.0**: anyone may use it, but changed versions must stay open source.

The translations themselves (the Hebrew text in packs) are a separate question. They are best kept
"all rights reserved" either way.
