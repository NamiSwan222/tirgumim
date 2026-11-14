# Plan: what we take from the Multiplayer Everything launcher

Source: github.com/multiplayereverything/multiplayer-mods-launcher (MIT, since 2026-11-14).
We rewrote the launcher on their stack (Tauri 2: Rust + React + TypeScript), following their structure
and window layout, with our own code. If we ever copy code as-is, their LICENSE text goes into
`THIRD_PARTY_LICENSES.md`.

**Status (2026-11-14):** the rewrite (v0.2) and all of stage 1 are done: GitHub release packs with a
10-minute cache, "close the game first", `requires`, and the byte-identical install/uninstall test
(`npm test`). Stage 2's installer exists (`npm run release`, NSIS, Hebrew); self-update and the
signed catalog are still to do.

## Stage 1: ready for the first real translation (Phantom Blade Zero), done

| # | What | Their version | Ours |
|---|---|---|---|
| 1 | **Pack straight from a GitHub release** | `"package": { "github": "owner/repo", "asset": "MyMod-*.zip" }`; the checksum is the `digest` GitHub publishes for each file | Catalog `pack` can be `{ "github": "NamiSwan222/tirgumim", "tag": "pbz-*", "asset": "pbz-he-*.zip" }`. The launcher finds the newest matching release and takes its version, URL, size and SHA-256 from GitHub. Publishing a new version = upload a release; no catalog edit. The old form (url + sha256 written by hand) still works. If GitHub gives no digest, the pack is not installed. |
| 2 | **Go easy on GitHub** | GitHub allows 60 requests an hour without a login; the answer is reused for 10 minutes, F5 asks again | Same: `sources.json` in the data folder; reused for 10 minutes; the רענון (refresh) button asks again; if GitHub can't be reached, the last answer is used. |
| 3 | **The game must be closed** | `processNames`: install is refused while the game runs | `detect.processNames` in the catalog (e.g. `PBZ.exe`, `PBZ-Win64-Shipping.exe`). Install, update and uninstall say "close the game first". |
| 4 | **Required files** | `install.requires`: a file the game folder must have, with a message | `requires: [{ "path", "message" }]` in the manifest, e.g. if a translation needs a mod loader. Install is off while it's missing, with the message shown. |
| 5 | **Test on a copy of the game** | e2e: install, then uninstall, then the folder must be byte-identical | `npm run e2e`: copies the sample game, installs the pack, uninstalls it, and compares every file's hash with the start. Also run once by hand on a copy of the real PBZ folder. |

## Stage 2: giving the launcher to other people

| # | What | Their version | Ours |
|---|---|---|---|
| 6 | **Installer** (done) | NSIS installer from Tauri | Same: `npm run release` builds a Tauri NSIS installer with Hebrew screens and a Start menu shortcut. Next: upload it to a GitHub Release. |
| 7 | **Self-update** | Tauri updater; `latest.json` signed with an offline key (minisign) | Same Tauri updater, `latest.json` from our newest GitHub release, signed with our own key. The private key stays on your PC, never in the repo. |
| 8 | **Signed catalog** | (they trust their GitHub) | The same key signs `catalog.json` (`catalog.json.sig`). A catalog with a bad signature is ignored and the last good one is used. Protects players if the GitHub account is taken over. |

## Stage 3: nice to have (later)

- A first-run screen (they have a step-by-step setup).
- Error reports to a small server (they use Cloudflare, `reports-api/`). Free tier is enough.
- Light theme, Ctrl+K search, compact cards.

## Not taking

Rooms, lobby, relay, Discord status, Flash/Ruffle, controller support: multiplayer features we don't need.

## Open decision: a license for our repo

The repo `NamiSwan222/tirgumim` has none, so nobody may reuse it. Options:
- **MIT** (like your friend): anyone may use it. Fine even with paid accounts later, because what's paid is the translations, not the launcher code.
- **No license** (all rights reserved): only we may use the code.
- **GPL-3.0**: anyone may use it, but changed versions must stay open source.

The translations themselves (the Hebrew text in packs) are a separate question. They are best kept
"all rights reserved" either way.
