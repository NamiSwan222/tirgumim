# Plan

**Status (2026-11-14):** v0.2 (Tauri: Rust + React + TypeScript) and all of stage 1 are done: GitHub
release packs with a 10-minute cache, "close the game first", `requires`, and the byte-identical
install/uninstall test (`npm test`). Stage 2's installer exists (`npm run release`, NSIS, Hebrew);
self-update and the signed catalog are still to do.

## Stage 1: ready for the first real translation, done

| # | What | How |
|---|---|---|
| 1 | **Pack straight from a GitHub release** | Catalog `pack` can be `{ "github": "NamiSwan222/tirgumim", "tag": "mygame-*", "asset": "mygame-he-*.zip" }`. The launcher finds the newest matching release and takes its version, URL, size and SHA-256 (GitHub's `digest`). Publishing a new version = upload a release; no catalog edit. The written-out form (url + sha256) still works. No digest from GitHub means no install. |
| 2 | **Go easy on GitHub** | GitHub allows 60 requests an hour without a login. Release lists are kept in `sources.json` and reused for 10 minutes; the רענון (refresh) button asks again; if GitHub can't be reached, the last answer is used. |
| 3 | **The game must be closed** | `detect.processNames` in the catalog (e.g. `MyGame.exe`). Install, update and uninstall say "close the game first". |
| 4 | **Required files** | `requires: [{ "path", "message" }]` in the catalog or manifest, e.g. if a translation needs a mod loader. Install is off while it's missing, with the message shown. |
| 5 | **Test on a copy of the game** | `npm test` copies the sample game, installs the pack, uninstalls it, and compares every file's hash with the start. Also run once by hand on a copy of the first real game folder. |

## Stage 2: giving the launcher to other people

| # | What | How |
|---|---|---|
| 6 | **Installer** (done) | `npm run release` builds a Tauri NSIS installer with Hebrew screens and a Start menu shortcut. GitHub Actions publishes it on every `v*` tag. |
| 7 | **Self-update** (built, first release pending) | Tauri updater reads `latest.json` from the fixed `updater` release, signed with our own key (repo secret; the key file stays on your PC). `npm run publish-launcher` + `.github/workflows/release.yml`. Waits on the signing key being set up. |
| 8 | **Signed catalog** | The same key signs `catalog.json` (`catalog.json.sig`). A catalog with a bad signature is ignored and the last good one is used. Protects players if the GitHub account is taken over. |
| 9 | **Catalog check** (done) | `.github/workflows/check.yml` checks `catalog.json` on every push with the launcher's parser and rules, and that each GitHub-release pack has a release. |

## Stage 4: paid translations, per game (maybe, parked: not planned for now)

Only for games whose risk check allows paid (Undertale and Yu-Gi-Oh are free only). Free games stay as they are.
Goal: one launcher update (0.5.0) adds the store; after that a paid game is a catalog entry + a product + an
upload, like a free one.

| Piece | How |
|---|---|
| **Catalog** | `"access": { "type": "paid", "productId": "…", "priceLabel": "₪19" }` and `"minLauncherVersion": "0.5.0"` on the game, so 0.4.0 shows "update the launcher" (already built), never a broken install. |
| **Payment** | A merchant of record (Lemon Squeezy or Paddle: they handle VAT, invoices and refunds) sells one product per game and issues a **license key** per purchase. No card data ever touches us. Check that the provider pays out to an Israeli seller before choosing. |
| **Buying in the launcher** | "קנייה" opens the provider's checkout in the browser. After paying, a `tirgumim://activate?key=…` link (or pasting the key) brings the key back. Keys are kept in Windows Credential Manager. |
| **Paid packs** | Not on public GitHub. Stored privately (Cloudflare R2) behind a small Worker: `POST /download { gameId, key, deviceId }` checks the key with the provider (activation limit, e.g. 3 PCs) and returns a short-lived download URL + sha256. Catalog pack form: `{ "gated": "https://api.tirgumim…/download" }`. |
| **Playing** | Launch-only packs already need the launcher to play in Hebrew. "שחק בעברית" re-checks the key now and then, with 14 days offline grace, so a refund or shared key stops working without bothering honest players. |
| **Cost** | Cloudflare free tier covers the Worker and storage at our size; the provider takes a fee per sale. |

Decisions for the user before building: provider, prices, activation limit, refund policy, whether players need
an account (email) or a license key is enough (simplest: key only).

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
