# Accounts, purchases and entitlements

How paid translations work: who may install them, how a purchase or an owner approval becomes access, how refunds
take it away, and the exact contract between the launcher and the account server. The launcher side is in
`src-tauri/src/core/account.rs` and `src-tauri/src/core/entitlements.rs`; a reference server that implements this
contract is in `server/`.

Free translations don't use any of this. With no account server built into the launcher, free games work exactly as
before, with no account, and paid games show as "not for sale yet".

## 1. What is protected, and what isn't

| Threat | Protection |
|---|---|
| Downloading a paid pack without paying | Paid packs are **never** public (no GitHub release, no public link). The only way to the file is a **download grant**: a signed, 10-minute, single-account, single-PC link the server issues after checking the account owns the game. The download link is checked by the server (or the storage, as a presigned URL), not by the launcher. |
| Editing the launcher's files to say "owned" | The entitlement list is signed by the server (Ed25519). The launcher checks the signature against public keys built into it, every time it reads the cached list. An edited list is ignored. |
| Pointing the launcher at your own server or key | Release builds take the server address and public keys only at build time. Run-time overrides exist only in debug builds. |
| Editing the catalog to mark a paid game free | The catalog doesn't carry a link for paid packs, so there is nothing to download. An install from a paid pack is remembered (`paid` in `state.json`) and stays gated. |
| Copying a signed list to another PC | Lists and grants name the PC's device id; another launcher refuses them. |
| Staying offline forever, or turning the clock back | A cached list expires 14 days after it was issued (the launcher caps it at 14 days + 1 hour whatever the server says). The launcher keeps the latest time it has seen; a clock more than 5 minutes behind that is "clock changed" and paid games are refused until the clock is right. Each fresh list resets that mark to the server's time, so a clock that was once wrongly far ahead doesn't lock anyone out. |
| Stolen session token | Tokens are random 256-bit values kept in the Windows Credential Manager (per Windows user, encrypted by Windows), never in a plain file. The server stores only their SHA-256. The owner can end all of an account's sessions. |
| Someone else signing in as you | The sign-in link goes to the email address. The page it opens shows a 4-digit code the launcher also shows, and needs a button press (an email scanner opening the link does not sign anyone in). Only the launcher that started the sign-in can collect the session (it alone knows the poll secret). |
| Forged payment notifications | The webhook checks the provider's signature on the raw body (Stripe: HMAC-SHA256 with `STRIPE_WEBHOOK_SECRET`, 5-minute tolerance), and applies each event id once. |
| Secrets in the client | The launcher holds only public keys and the player's own session. Signing key, Stripe keys, webhook secret and admin token live on the server, from environment variables. |

**What can't be protected:** once a pack's files are on a player's PC, a determined player can copy them, or start a
launch-only game with the right arguments by hand. Revocation (refund, chargeback, owner revoke) stops new installs,
updates and "Play in Hebrew" from the launcher; it does not delete files from a PC. That is the same for every
desktop game store; the realistic goal is "buying is easier than pirating", plus no public download.

## 2. Configuration

**Launcher** (`src-tauri/src/core/account.rs`):

| What | Where | Unset |
|---|---|---|
| Server address | `ACCOUNT_URL`, or `TIRGUMIM_ACCOUNT_URL` at **build** time. https only. | Accounts off: `accountState().status === 'notConfigured'`, store shows everything "not for sale yet", free games unaffected. |
| Server public keys | `ENTITLEMENT_KEYS` (`"k1:<base64url>,k2:<base64url>"`), or `TIRGUMIM_ENTITLEMENT_KEYS` at **build** time. Public, so commit them once generated. | Same as above. |
| Debug builds only | The same two variables at **run** time, and `http://127.0.0.1` / `http://localhost` addresses, for the reference server on this PC. | |

**Server**: environment variables, see `server/.env.example`. Nothing secret has a default; `.env` is git-ignored.

## 3. Catalog fields (launcher 0.5.0+)

Both optional; leaving them out means free, exactly as before.

```json
{ "id": "my-game", "name": "My Game", "detect": { … },
  "paid": true,
  "priceId": "price_1Q…",
  "minLauncherVersion": "0.5.0" }
```

- `paid`: install, update and play need an active entitlement. Free games never go through the gate.
- `priceId`: the payment provider's price id (Stripe Price, or another provider's). A hint for the store; the
  amount, currency and the price actually charged come from the server (`/v1/store`), never from the catalog.
- A paid entry **has no `pack`** (its pack comes from the server) and **must set `minLauncherVersion` ≥ 0.5.0**:
  older launchers don't know `paid`. `--check-catalog` fails otherwise. A tagged repo's
  `pack/catalog-entry.json` with `"paid": true` doesn't get the repo's releases as its pack, and that repo must not
  publish the pack zip as a release. Better: keep a paid translation's repo private (a public repo gives the
  translated files away) and put its entry in `catalog/catalog.json`. (0.5.0 is `PAID_MIN_LAUNCHER` in `check.rs`: set it to the first released
  launcher that contains this code.)

## 4. Server contract

Base: `https://<server>`. JSON in and out (`Content-Type: application/json`), UTF-8. Times are **unix seconds**
except `grantedAt`/`publishedAt` (ISO 8601 UTC). Signed-in calls send `Authorization: Bearer <sessionToken>`. Admin
calls send `Authorization: Bearer <ADMIN_TOKEN>` and optionally `X-Admin-Name: <who>` (recorded in the audit log).

Errors are always `{"error": {"code": "UPPER_SNAKE", "message": "English, for logs"}}` with a fitting HTTP status.
The launcher passes `code` through to the window. A `401` on a signed-in call means the session is over: the launcher
signs out.

### Sign-in (email link, no passwords)

**`POST /v1/auth/start`** (rate limit: 5 per email and 30 per IP per 15 minutes)
```json
→ { "email": "player@example.com", "deviceId": "dev_<32 hex>", "deviceName": "PC-NAME",
    "pollHash": "<sha256 hex of a 32-byte random poll secret the launcher keeps>" }
← { "requestId": "req_…", "code": "4821", "expiresAt": 1800000900 }
```
The server emails `https://<server>/v1/auth/verify?t=<link token>` (link token: 32 random bytes, stored hashed,
15 minutes, one use). With no mail provider, the reference server prints the link to its log.

**`GET /v1/auth/verify?t=…`**: an HTML page (Hebrew, `Referrer-Policy: no-referrer`, strict CSP) with the email, the
code, and a confirm button. Does not sign in. Expired/used: 410 page.

**`POST /v1/auth/verify`** (form `t=…`): approves the request, creates the account on first sign-in.

**`POST /v1/auth/poll`** (the launcher, every 3 s)
```json
→ { "requestId": "req_…", "pollSecret": "<the secret>" }
← { "status": "pending" }
← { "status": "approved", "sessionToken": "<43 chars>", "accountId": "acc_…", "email": "…", "expiresAt": 1807776000 }
```
The session is created at this moment and handed out once. Wrong secret or unknown id: `404 AUTH_REQUEST_UNKNOWN`.
Expired or already collected: `410 AUTH_REQUEST_EXPIRED`.

**`POST /v1/auth/signout`** (signed in) → `{ "ok": true }`: ends the session on the server.
**`GET /v1/me`** (signed in) → `{ "accountId", "email" }`.

### Entitlements

**`GET /v1/entitlements`** (signed in) → a signed envelope (§5) whose payload is:
```json
{ "typ": "tirgumim.entitlements.v1",
  "accountId": "acc_…", "deviceId": "dev_…",           // the session's account and PC
  "issuedAt": 1800000000, "expiresAt": 1801209600,      // 14 days
  "entitlements": [
    { "gameId": "my-game", "source": "purchase", "status": "active",
      "grantedAt": "2027-01-15T08:00:00Z", "orderRef": "ord_…" } ] }
```
Every entitlement the account ever had is listed (revoked and refunded too, so the window can say why). A game is
owned when at least one of its entries is `active`.

### Store and packs (public)

**`GET /v1/store`** →
```json
{ "paymentsConfigured": false,
  "products": [ { "gameId": "my-game", "priceId": "price_…", "amount": 2990, "currency": "ILS", "active": true } ] }
```
`amount` in minor units (agorot). Only products with a price id and an amount are `active` and listed.

**`GET /v1/packs/<gameId>`** → `{ "gameId", "version", "sha256", "size", "changelog", "publishedAt" }` (no link),
or `404 NO_PACK`. The launcher uses it to show "update available".

### Checkout

**`POST /v1/checkout`** (signed in) `{ "gameId" }` → `{ "url": "https://checkout.stripe.com/…", "orderId": "ord_…" }`.
Errors: `503 PAYMENTS_NOT_CONFIGURED`, `404 NOT_FOR_SALE` (no active product), `409 ALREADY_OWNED`, `429 RATE_LIMITED`.
The server records an order (`created`), creates a hosted checkout session with the product's price id and the order
id / account id / game id in its metadata, and returns the provider's page. The launcher opens it in the browser.
Return pages: `GET /v1/checkout/done`, `GET /v1/checkout/cancelled`.

**`POST /v1/webhooks/stripe`** (from Stripe, raw body, header `Stripe-Signature: t=…,v1=…`)
- Signature: HMAC-SHA256(`STRIPE_WEBHOOK_SECRET`, `"<t>." + raw body`), compared in constant time; `t` within
  5 minutes. Bad: `400 BAD_SIGNATURE`.
- Idempotent: the event id is inserted into `webhook_events` in the same transaction as its effect; a repeat answers
  `{ "received": true, "duplicate": true }` and changes nothing.
- `checkout.session.completed` / `checkout.session.async_payment_succeeded` with `payment_status: "paid"`: the order
  (found by the session id **we** stored, not by metadata) becomes `paid` and gets an entitlement
  `source: purchase`, `orderRef: <order id>`.
- `charge.refunded` with `refunded: true` (full refund): that order's entitlement becomes `refunded`. Partial refunds
  change nothing.
- `charge.dispute.created`: that order's entitlement becomes `revoked`.
- Other events: recorded and ignored. Stripe dashboard → Webhooks: subscribe to exactly these four.

Another provider is another `payments.js` object (`createCheckout`, `lookupPrice`, `verifyWebhook`, `interpret`) and
another webhook route; nothing else changes.

### Download grants

**`POST /v1/download-grants`** (signed in) `{ "gameId" }` → a signed envelope whose payload is:
```json
{ "typ": "tirgumim.download.v1", "grantId": "…", "accountId": "acc_…", "deviceId": "dev_…", "gameId": "my-game",
  "version": "1.2.0", "sha256": "<zip sha256>", "size": 123456, "url": "https://<server>/v1/files/<32 random bytes>",
  "issuedAt": 1800000000, "expiresAt": 1800000600 }
```
Errors: `403 NOT_ENTITLED`, `404 NO_PACK`, `429 RATE_LIMITED` (30 grants an hour per account).
**`GET /v1/files/<token>`**: the zip, if the grant exists (stored hashed), is under 10 minutes old, was used fewer
than 3 times, and the account **still** owns the game. Else `410 GRANT_EXPIRED` / `410 GRANT_USED` /
`403 NOT_ENTITLED`. In production, prefer returning a presigned private-bucket URL (R2/S3, 10-minute expiry) as
`url`; the contract is the same.

### Admin (the owner)

| Call | Body / query | Effect |
|---|---|---|
| `POST /v1/admin/approve` | `{ email, gameId, source?: "admin-approved" \| "grant", note? }` | Active entitlement (creates the account if the person never signed in). Idempotent: `alreadyActive: true`. |
| `POST /v1/admin/revoke` | `{ email, gameId, reason }` | Every active entitlement of that account for that game → `revoked`. |
| `POST /v1/admin/sessions/revoke` | `{ email }` | Ends all of the account's sessions (lost or stolen PC). |
| `GET /v1/admin/entitlements?email=` | | The account's entitlements. |
| `GET /v1/admin/audit?limit=100` | | Newest audit entries. |
| `PUT /v1/admin/products/<gameId>` | `{ priceId, amount?, currency?, active }` | Sets the price. With a provider, amount and currency are read from the provider's price. |
| `PUT /v1/admin/packs/<gameId>` | `{ version, file, changelog? }` | Publishes a paid pack already copied to `PACKS_DIR/<gameId>/<file>`; the server computes sha256 and size. |

`ADMIN_TOKEN` shorter than 32 characters or unset: every admin call answers `503 ADMIN_DISABLED`. Examples:
```sh
curl -X POST https://<server>/v1/admin/approve -H "Authorization: Bearer $ADMIN_TOKEN" \
     -H "Content-Type: application/json" -d '{"email":"tester@example.com","gameId":"my-game","note":"translator"}'
curl -X POST https://<server>/v1/admin/revoke  -H "Authorization: Bearer $ADMIN_TOKEN" \
     -H "Content-Type: application/json" -d '{"email":"tester@example.com","gameId":"my-game","reason":"left the team"}'
```

## 5. Signatures

- Algorithm: **Ed25519** (RFC 8032), strict verification (`verify_strict`).
- Envelope: `{ "kid": "k1", "payload": "<base64url, no padding, of the UTF-8 JSON bytes>", "sig": "<base64url of
  the 64-byte signature over exactly those bytes>" }`. Signing the bytes as sent avoids any JSON canonicalisation.
- `typ` inside the payload separates the two kinds (`tirgumim.entitlements.v1`, `tirgumim.download.v1`): a grant is
  never accepted as a list or the other way round.
- Keys: `npm run keygen` in `server/` prints the private seed (server env `ENTITLEMENT_SIGNING_KEY`) and the public
  key for the launcher (`ENTITLEMENT_KEYS`). **Rotation**: generate `k2`, ship a launcher with both `k1` and `k2`,
  wait until players have it, switch the server to `k2`, later drop `k1`. If the private key leaks: switch the
  server to a new key and ship a launcher without the old one (lists signed with the leaked key stop working on
  updated launchers).
- Interop is tested: `entitlements::tests::verifies_what_the_server_signs` verifies an envelope made by
  `server/src/crypto.js`.

**What the launcher checks on a list** (`entitlements::parse_doc`, `Snapshot::evaluate`): known `kid`; valid
signature; `typ`; `accountId` = the signed-in account; `deviceId` = this PC; `expiresAt > issuedAt`; not expired
(`max(now, highWater) < min(expiresAt, issuedAt + 14 d + 1 h)`); clock not turned back (`now + 5 min ≥
max(highWater, issuedAt)`).

**On a grant** (`entitlements::parse_grant`): known `kid`, signature, `typ`, account, device, the requested `gameId`,
lifetime ≤ 1 hour, `now` within `[issuedAt − 5 min, expiresAt + 5 min]`, a valid `sha256`, an https `url`. The
downloaded zip must match the grant's `sha256` before anything is extracted (the normal install check).

## 6. Lifetimes

| Token | Lifetime | Stored as |
|---|---|---|
| Sign-in link | 15 minutes, one use | SHA-256 on the server |
| Poll secret | the sign-in request | launcher memory; SHA-256 on the server |
| Session token | 90 days | Windows Credential Manager (launcher); SHA-256 on the server |
| Entitlement list | 14 days offline (server sets, launcher caps at 14 d + 1 h) | `data\account\entitlements.json` (signed) |
| Download grant and its link | 10 minutes, 3 downloads | SHA-256 of the link token on the server |
| Stripe webhook signature | 5 minutes tolerance | |

The launcher fetches a fresh list at start, after sign-in, on `entitlements(true)`, and whenever the cached one is
over an hour old (at most one try every 5 minutes while offline).

## 7. Records

- **Account**: `id` (`acc_<24 hex>`, random, never reused), `email` (lower-case, unique), `createdAt`. No password,
  no name, no payment details (the provider keeps those).
- **Entitlement**: `accountId`, `gameId`, `source` (`purchase` | `grant` | `admin-approved`), `status` (`active` |
  `revoked` | `refunded`), `grantedAt`, `orderRef` (our order id for purchases, `admin_<hex>` for owner grants;
  unique, so every grant is idempotent), `changedAt`, `note`. Rows are never deleted: status changes.
- **Order**: `id` (`ord_…`), account, game, provider, provider session id, payment id, status (`created` | `paid` |
  `refunded` | `disputed`), amount, currency.
- **Audit**: every account creation, sign-in approval, session, sign-out, checkout start, entitlement change (with the
  actor: `admin:<name>`, `webhook:stripe`, `account:<id>`, `system`), price change, pack publish and download grant.

## 8. The launcher

**Gate** (`Accounts::check_access`, used by `install`, `update` and `play`; `uninstall` is never gated, so a player
can always get their game back to the original):

| Situation | Free game | Paid game |
|---|---|---|
| No account server built in | works | `ACCOUNTS_NOT_CONFIGURED` |
| Signed out | works | `SIGN_IN_REQUIRED` |
| Signed in, list valid, owned | works | works |
| Signed in, not owned / revoked / refunded | works | `NOT_ENTITLED` |
| Signed in, no list yet and server unreachable | works | `ENTITLEMENTS_UNAVAILABLE` |
| List expired (offline > 14 days) | works | `ENTITLEMENTS_EXPIRED` |
| Clock turned back | works | `CLOCK_CHANGED` |

A paid game is "paid in the catalog **or** installed from a paid pack". Its install asks for a download grant and
uses the grant's link and checksum; the install record is then marked `paid`.

**Window API** (`src/api.ts`, `accountApi`): `accountState()`, `signIn(email)`, `cancelSignIn()`, `signOut()`,
`entitlements(refresh)`, `storeList()`, `startCheckout(gameId)` (then `api.openUrl(url)`), `onAccount(fn)` (events
`signedIn`, `signInExpired`, `signInFailed`, `signedOut`). `list()` now also gives every game `paid`, `priceId` and
`access` (`free` | `owned` | `notOwned` | `revoked` | `refunded` | `signedOut` | `checking` | `expired` |
`clockChanged` | `notConfigured`). The library should show a paid game only when `access === 'owned'` (or it is
installed); the Store tab lists `storeList().items`.

## 9. Error codes and suggested Hebrew texts (for `src/texts.ts`)

| Code | When | Suggested message |
|---|---|---|
| `NOT_ENTITLED` | Paid translation not in the account (or revoked/refunded) | התרגום הזה לא נמצא בחשבון שלך. אפשר לרכוש אותו בחנות. |
| `SIGN_IN_REQUIRED` | Paid action while signed out | צריך להתחבר לחשבון כדי להשתמש בתרגום הזה. |
| `SIGN_IN_EXPIRED` | The server ended the session | נותקת מהחשבון. התחבר/י שוב. |
| `ACCOUNTS_NOT_CONFIGURED` | Launcher built without an account server | תרגומים בתשלום עוד לא זמינים בגרסה הזו של המשגר. |
| `PAYMENTS_NOT_CONFIGURED` | No payment provider (or no server) | הרכישה עוד לא פתוחה. נסו שוב בקרוב. |
| `NOT_FOR_SALE` | No active price for the game | התרגום הזה עוד לא עומד למכירה. |
| `ALREADY_OWNED` | Checkout for an owned game | התרגום הזה כבר בחשבון שלך. |
| `ENTITLEMENTS_UNAVAILABLE` | No list yet, server unreachable | לא הצלחנו לבדוק את החשבון מול השרת. בדקו את החיבור לאינטרנט ונסו שוב. |
| `ENTITLEMENTS_EXPIRED` | Offline over 14 days | המשגר צריך להתחבר לאינטרנט פעם אחת כדי לאשר את התרגומים שלך. |
| `ENTITLEMENTS_INVALID` | List failed verification | התקבלה תשובה לא תקינה מהשרת. נסו שוב מאוחר יותר. |
| `CLOCK_CHANGED` | PC clock behind what was seen | נראה ששעון המחשב הוחזר לאחור. תקנו את התאריך והשעה ונסו שוב. |
| `GRANT_INVALID` | Download grant failed verification | אישור ההורדה מהשרת לא תקין. נסו שוב. |
| `ACCOUNT_SERVER` | Server unreachable or unexpected answer | אין חיבור לשרת החשבונות. נסו שוב מאוחר יותר. |
| `RATE_LIMITED` | Too many requests | יותר מדי ניסיונות. חכו כמה דקות ונסו שוב. |
| `BAD_EMAIL` | Sign-in with a malformed address | כתובת האימייל לא תקינה. |
| `SECURE_STORE` | Windows Credential Manager refused | לא הצלחנו לשמור את פרטי ההתחברות במחשב. |
| `AUTH_REQUEST_EXPIRED` | The emailed link wasn't confirmed in time (shown via `signInExpired`) | הקישור פג תוקף. שלחו קישור חדש. |
| `NO_PACK` | Paid game with no published pack yet | התרגום עוד לא פורסם. |

Account states for the UI: pending sign-in → "שלחנו קישור אל {email}. פתחו אותו ואשרו שהקוד {code} מופיע גם שם.";
`access: 'checking'` → "בודקים את החשבון…".

## 10. Running the reference server

```sh
cd server
npm run keygen                    # once: copy the lines into .env (server) and ENTITLEMENT_KEYS (launcher)
cp .env.example .env              # fill in ENTITLEMENT_SIGNING_KEY and ADMIN_TOKEN at least
npm start                         # Node 22.13+; SQLite file in ./data
npm test                          # node:test, no dependencies to install
```
Against a debug launcher on the same PC: `PUBLIC_URL=http://127.0.0.1:8787`, and start the launcher with
`TIRGUMIM_ACCOUNT_URL=http://127.0.0.1:8787` and `TIRGUMIM_ENTITLEMENT_KEYS=<k1:…>` in its environment.

**Before real players** (owner checklist):
1. Host it behind https (a VPS or Fly.io/Render with a persistent volume for the SQLite file, and daily backups of
   it); `PUBLIC_URL` https; `TRUST_PROXY=true` behind a proxy.
2. Signing key and secrets in the host's secret store, not in files on shared machines.
3. A mail provider (Resend, Postmark, SES): implement `sendSignIn` in `server/src/mail.js`, set up SPF/DKIM.
4. Paid packs in private storage; switch `/v1/files` to presigned URLs if the files are large.
5. Stripe: live keys, webhook endpoint `/v1/webhooks/stripe` with the four events, products and prices, then
   `PUT /v1/admin/products/<gameId>`.
6. Legal pages linked from the store and the checkout: terms of sale, refund policy, privacy policy (emails are
   personal data), Israeli consumer-protection cancellation rules for digital content, VAT invoices.
7. Build the launcher with `ACCOUNT_URL` and `ENTITLEMENT_KEYS` set, release it, and only then publish paid catalog
   entries with `minLauncherVersion` set to that release.
