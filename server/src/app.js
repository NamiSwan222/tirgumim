// The account server's HTTP API (contract: ../ENTITLEMENTS.md). Plain node:http, no framework.
import { createReadStream, statSync } from 'node:fs'
import { createHash, randomInt } from 'node:crypto'
import { join, resolve, sep } from 'node:path'
import { pipeline } from 'node:stream/promises'
import { GRANT_MAX_DOWNLOADS, GRANT_TTL, SESSION_TTL, SIGN_IN_LINK_TTL } from './config.js'
import { randomId, randomToken, safeEqual, sha256hex } from './crypto.js'
import { tx } from './db.js'

export const ENTITLEMENTS_TYP = 'tirgumim.entitlements.v1'
export const GRANT_TYP = 'tirgumim.download.v1'
const GAME_ID = /^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/
const EMAIL = /^[^\s@]{1,64}@[^\s@]+\.[^\s@.]{2,}$/
const MAX_BODY = 64 * 1024

class HttpError extends Error {
  constructor(status, code, message) {
    super(message)
    this.status = status
    this.code = code
  }
}
const fail = (status, code, message) => {
  throw new HttpError(status, code, message)
}

/** A sliding-window counter per key (in memory: one server process). */
function rateLimiter() {
  const hits = new Map()
  return (key, max, windowSecs, now) => {
    const list = (hits.get(key) || []).filter((t) => t > now - windowSecs)
    if (list.length >= max) fail(429, 'RATE_LIMITED', 'Too many requests, try again later')
    list.push(now)
    hits.set(key, list)
    if (hits.size > 50_000) hits.clear()
  }
}

const esc = (s) => String(s).replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c])

function page(title, body) {
  return `<!doctype html><html lang="he" dir="rtl"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>${esc(title)}</title><style>body{font-family:system-ui,sans-serif;max-width:28rem;margin:4rem auto;padding:0 1rem;line-height:1.6}
.code{font-size:2rem;letter-spacing:.3em;font-weight:700}button{font-size:1.1rem;padding:.6rem 1.4rem;cursor:pointer}</style></head>
<body><h1>${esc(title)}</h1>${body}</body></html>`
}

/**
 * deps: { config, db, signer, payments, mailer, now?: () => unix seconds, log? }
 * Returns a node:http request handler.
 */
export function createApp({ config, db, signer, payments, mailer, now = () => Math.floor(Date.now() / 1000), log = console.log }) {
  const limit = rateLimiter()
  const q = (sql) => db.prepare(sql)

  const audit = (actor, action, accountId, gameId, detail) =>
    q('INSERT INTO audit (at, actor, action, account_id, game_id, detail) VALUES (?, ?, ?, ?, ?, ?)').run(now(), actor, action, accountId ?? null, gameId ?? null, detail == null ? null : JSON.stringify(detail))

  const accountByEmail = (email) => q('SELECT * FROM accounts WHERE email = ?').get(email)
  const ensureAccount = (email) => {
    let a = accountByEmail(email)
    if (!a) {
      a = { id: randomId('acc'), email, created_at: now() }
      q('INSERT INTO accounts (id, email, created_at) VALUES (?, ?, ?)').run(a.id, a.email, a.created_at)
      audit('system', 'account-created', a.id, null, { email })
    }
    return a
  }
  const ownsActive = (accountId, gameId) => !!q("SELECT 1 FROM entitlements WHERE account_id = ? AND game_id = ? AND status = 'active'").get(accountId, gameId)
  const latestPack = (gameId) => q('SELECT * FROM packs WHERE game_id = ? ORDER BY published_at DESC, rowid DESC LIMIT 1').get(gameId)

  const clientIp = (req) => (config.trustProxy && String(req.headers['x-forwarded-for'] || '').split(',')[0].trim()) || req.socket.remoteAddress || '?'

  function session(req) {
    const m = /^Bearer (\S+)$/.exec(String(req.headers.authorization || ''))
    if (!m) fail(401, 'UNAUTHORIZED', 'Sign in first')
    const s = q('SELECT * FROM sessions WHERE token_hash = ? AND revoked_at IS NULL AND expires_at > ?').get(sha256hex(m[1]), now())
    if (!s) fail(401, 'UNAUTHORIZED', 'Session expired or signed out')
    if (now() - s.last_used_at > 3600) q('UPDATE sessions SET last_used_at = ? WHERE token_hash = ?').run(now(), s.token_hash)
    return { ...s, account: q('SELECT * FROM accounts WHERE id = ?').get(s.account_id) }
  }

  function admin(req) {
    if (!config.adminToken) fail(503, 'ADMIN_DISABLED', 'Set ADMIN_TOKEN (32+ characters) to use the admin API')
    const m = /^Bearer (\S+)$/.exec(String(req.headers.authorization || ''))
    if (!m || !safeEqual(m[1], config.adminToken)) fail(401, 'UNAUTHORIZED', 'Admin token required')
    return `admin:${String(req.headers['x-admin-name'] || 'owner').slice(0, 40)}`
  }

  const gameIdOf = (v) => {
    if (typeof v !== 'string' || !GAME_ID.test(v)) fail(400, 'BAD_GAME_ID', 'Bad gameId')
    return v
  }
  const emailOf = (v) => {
    const e = String(v || '').trim().toLowerCase()
    if (e.length > 254 || !EMAIL.test(e)) fail(400, 'BAD_EMAIL', 'Bad email')
    return e
  }

  // ---------- entitlement records ----------

  function entitlementList(accountId) {
    return q('SELECT game_id, source, status, granted_at, order_ref FROM entitlements WHERE account_id = ? ORDER BY granted_at, id').all(accountId).map((e) => ({
      gameId: e.game_id,
      source: e.source,
      status: e.status,
      grantedAt: new Date(e.granted_at * 1000).toISOString().replace(/\.\d{3}Z$/, 'Z'),
      orderRef: e.order_ref,
    }))
  }

  function grantEntitlement({ accountId, gameId, source, orderRef, actor, note }) {
    const existing = q('SELECT * FROM entitlements WHERE order_ref = ?').get(orderRef)
    if (existing) return existing
    q('INSERT INTO entitlements (account_id, game_id, source, status, granted_at, order_ref, changed_at, note) VALUES (?, ?, ?, ?, ?, ?, ?, ?)').run(accountId, gameId, source, 'active', now(), orderRef, now(), note ?? null)
    audit(actor, 'entitlement-granted', accountId, gameId, { source, orderRef, note })
    return q('SELECT * FROM entitlements WHERE order_ref = ?').get(orderRef)
  }

  function setStatus(where, args, status, actor, detail) {
    const rows = q(`SELECT * FROM entitlements WHERE ${where} AND status = 'active'`).all(...args)
    for (const r of rows) {
      q('UPDATE entitlements SET status = ?, changed_at = ?, note = ? WHERE id = ?').run(status, now(), detail?.reason ?? r.note, r.id)
      audit(actor, `entitlement-${status}`, r.account_id, r.game_id, { orderRef: r.order_ref, ...detail })
    }
    return rows.length
  }

  // ---------- routes ----------

  const routes = []
  const route = (method, pattern, handler, opts = {}) => routes.push({ method, pattern, handler, ...opts })

  route('GET', /^\/v1\/health$/, () => ({ ok: true, payments: payments.configured, mail: mailer.configured }))

  // Sign-in, step 1: the launcher asks for a link. It sends only the hash of its poll secret.
  route('POST', /^\/v1\/auth\/start$/, async ({ body, req }) => {
    const email = emailOf(body.email)
    const deviceId = String(body.deviceId || '')
    const pollHash = String(body.pollHash || '')
    if (!/^[A-Za-z0-9_-]{16,80}$/.test(deviceId)) fail(400, 'BAD_REQUEST', 'Bad deviceId')
    if (!/^[0-9a-f]{64}$/.test(pollHash)) fail(400, 'BAD_REQUEST', 'Bad pollHash')
    limit(`start:email:${email}`, 5, 900, now())
    limit(`start:ip:${clientIp(req)}`, 30, 900, now())
    const linkToken = randomToken(32)
    const code = String(randomInt(1000, 10000))
    const r = { id: randomId('req'), expires_at: now() + SIGN_IN_LINK_TTL }
    q('INSERT INTO auth_requests (id, email, link_hash, poll_hash, code, device_id, device_name, ip, status, created_at, expires_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)').run(
      r.id, email, sha256hex(linkToken), pollHash, code, deviceId, String(body.deviceName || '').slice(0, 80), clientIp(req), 'pending', now(), r.expires_at,
    )
    await mailer.sendSignIn({ to: email, code, link: `${config.publicUrl}/v1/auth/verify?t=${linkToken}` })
    return { requestId: r.id, code, expiresAt: r.expires_at }
  })

  // Step 2: the link opens a page that shows the code and asks to confirm (a GET never signs in, so mail scanners
  // that open links don't, and the player sees which code they are approving).
  const pendingByLink = (t) => {
    const r = typeof t === 'string' && t ? q('SELECT * FROM auth_requests WHERE link_hash = ?').get(sha256hex(t)) : null
    if (!r || r.status !== 'pending' || r.expires_at < now()) return null
    return r
  }
  route('GET', /^\/v1\/auth\/verify$/, ({ url }) => {
    const t = url.searchParams.get('t')
    const r = pendingByLink(t)
    if (!r) return { html: page('הקישור לא בתוקף', '<p>הקישור כבר נוצל או שפג תוקפו. בקשו קישור חדש מהמשגר.</p>'), status: 410 }
    return {
      html: page('כניסה למשגר תרגומים', `<p>מאשרים כניסה בתור <b>${esc(r.email)}</b>?</p><p>ודאו שהמשגר מציג את אותו קוד:</p>
<p class="code">${esc(r.code)}</p><form method="post" action="/v1/auth/verify"><input type="hidden" name="t" value="${esc(t)}"><button type="submit">אישור כניסה</button></form>
<p>לא ביקשתם להיכנס? סגרו את הדף.</p>`),
    }
  })
  route('POST', /^\/v1\/auth\/verify$/, ({ form }) => {
    const r = pendingByLink(form.get('t'))
    if (!r) return { html: page('הקישור לא בתוקף', '<p>בקשו קישור חדש מהמשגר.</p>'), status: 410 }
    tx(db, () => {
      const a = ensureAccount(r.email)
      q("UPDATE auth_requests SET status = 'approved', account_id = ? WHERE id = ? AND status = 'pending'").run(a.id, r.id)
      audit(`account:${a.id}`, 'sign-in-approved', a.id, null, { requestId: r.id, deviceName: r.device_name })
    })
    return { html: page('נכנסתם', '<p>אפשר לחזור למשגר.</p>') }
  }, { form: true })

  // Step 3: the launcher polls with its secret. The session is made here, once, for that launcher only.
  route('POST', /^\/v1\/auth\/poll$/, ({ body }) => {
    const r = q('SELECT * FROM auth_requests WHERE id = ?').get(String(body.requestId || ''))
    if (!r || !safeEqual(sha256hex(String(body.pollSecret || '')), r.poll_hash)) fail(404, 'AUTH_REQUEST_UNKNOWN', 'No such sign-in')
    if (r.status === 'consumed' || (r.status === 'pending' && r.expires_at < now())) fail(410, 'AUTH_REQUEST_EXPIRED', 'Sign-in expired')
    if (r.status === 'pending') return { status: 'pending' }
    const token = randomToken(32)
    const expiresAt = now() + SESSION_TTL
    const account = q('SELECT * FROM accounts WHERE id = ?').get(r.account_id)
    tx(db, () => {
      const c = q("UPDATE auth_requests SET status = 'consumed' WHERE id = ? AND status = 'approved'").run(r.id)
      if (c.changes !== 1) fail(410, 'AUTH_REQUEST_EXPIRED', 'Sign-in expired')
      q('INSERT INTO sessions (token_hash, account_id, device_id, device_name, created_at, last_used_at, expires_at) VALUES (?, ?, ?, ?, ?, ?, ?)').run(sha256hex(token), account.id, r.device_id, r.device_name, now(), now(), expiresAt)
      audit(`account:${account.id}`, 'session-created', account.id, null, { deviceId: r.device_id })
    })
    return { status: 'approved', sessionToken: token, accountId: account.id, email: account.email, expiresAt }
  })

  route('POST', /^\/v1\/auth\/signout$/, ({ req }) => {
    const s = session(req)
    q('UPDATE sessions SET revoked_at = ? WHERE token_hash = ?').run(now(), s.token_hash)
    audit(`account:${s.account_id}`, 'signed-out', s.account_id, null, null)
    return { ok: true }
  })

  route('GET', /^\/v1\/me$/, ({ req }) => {
    const s = session(req)
    return { accountId: s.account.id, email: s.account.email }
  })

  // The signed list the launcher caches for offline play.
  route('GET', /^\/v1\/entitlements$/, ({ req }) => {
    const s = session(req)
    const issuedAt = now()
    return signer.sign({
      typ: ENTITLEMENTS_TYP,
      accountId: s.account_id,
      deviceId: s.device_id,
      issuedAt,
      expiresAt: issuedAt + config.entitlementTtl,
      entitlements: entitlementList(s.account_id),
    })
  })

  // ---------- store, packs, checkout ----------

  route('GET', /^\/v1\/store$/, () => ({
    paymentsConfigured: payments.configured,
    products: q('SELECT * FROM products WHERE active = 1 ORDER BY game_id').all().map((p) => ({
      gameId: p.game_id, priceId: p.price_id, amount: p.amount, currency: p.currency, active: !!p.active,
    })),
  }))

  route('GET', /^\/v1\/packs\/([^/]+)$/, ({ match }) => {
    const p = latestPack(gameIdOf(decodeURIComponent(match[1])))
    if (!p) fail(404, 'NO_PACK', 'No pack yet')
    return { gameId: p.game_id, version: p.version, sha256: p.sha256, size: p.size, changelog: p.changelog, publishedAt: new Date(p.published_at * 1000).toISOString() }
  })

  route('POST', /^\/v1\/checkout$/, async ({ req, body }) => {
    const s = session(req)
    const gameId = gameIdOf(body.gameId)
    if (!payments.configured) fail(503, 'PAYMENTS_NOT_CONFIGURED', 'Payments are not set up yet')
    const product = q('SELECT * FROM products WHERE game_id = ? AND active = 1').get(gameId)
    if (!product || !product.price_id) fail(404, 'NOT_FOR_SALE', 'Not for sale yet')
    if (ownsActive(s.account_id, gameId)) fail(409, 'ALREADY_OWNED', 'Already in your account')
    limit(`checkout:${s.account_id}`, 10, 3600, now())
    const order = { id: randomId('ord'), game_id: gameId }
    q('INSERT INTO orders (id, account_id, game_id, provider, status, amount, currency, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)').run(order.id, s.account_id, gameId, payments.name, 'created', product.amount, product.currency, now(), now())
    const { url, providerRef } = await payments.createCheckout({ order, product, account: s.account })
    q('UPDATE orders SET provider_ref = ?, updated_at = ? WHERE id = ?').run(providerRef, now(), order.id)
    audit(`account:${s.account_id}`, 'checkout-started', s.account_id, gameId, { orderId: order.id, providerRef })
    return { url, orderId: order.id }
  })

  route('GET', /^\/v1\/checkout\/(done|cancelled)$/, ({ match }) => ({
    html: match[1] === 'done' ? page('תודה!', '<p>התשלום התקבל. חזרו למשגר: התרגום יופיע בספרייה תוך רגע.</p>') : page('התשלום בוטל', '<p>לא חויבתם. אפשר לחזור למשגר.</p>'),
  }))

  // Payment provider webhook: signature checked on the raw bytes, each event applied once.
  route('POST', /^\/v1\/webhooks\/stripe$/, ({ raw, req }) => {
    if (!payments.configured || payments.name !== 'stripe') fail(503, 'PAYMENTS_NOT_CONFIGURED', 'Payments are not set up')
    const event = payments.verifyWebhook(raw, req.headers, now())
    if (typeof event?.id !== 'string') fail(400, 'BAD_REQUEST', 'No event id')
    return tx(db, () => {
      const seen = q('INSERT OR IGNORE INTO webhook_events (id, provider, type, received_at) VALUES (?, ?, ?, ?)').run(event.id, 'stripe', String(event.type), now())
      if (seen.changes === 0) return { received: true, duplicate: true }
      const ev = payments.interpret(event)
      const actor = 'webhook:stripe'
      if (ev.kind === 'paid') {
        const order = q('SELECT * FROM orders WHERE provider_ref = ?').get(ev.providerRef)
        if (!order) {
          audit(actor, 'webhook-unknown-order', null, null, { eventId: event.id, providerRef: ev.providerRef })
          return { received: true, ignored: 'unknown order' }
        }
        q("UPDATE orders SET status = 'paid', payment_ref = ?, amount = ?, currency = ?, updated_at = ? WHERE id = ?").run(ev.paymentRef, ev.amount ?? order.amount, ev.currency ?? order.currency, now(), order.id)
        grantEntitlement({ accountId: order.account_id, gameId: order.game_id, source: 'purchase', orderRef: order.id, actor, note: `event ${event.id}` })
      } else if (ev.kind === 'refunded' || ev.kind === 'disputed') {
        const order = ev.paymentRef ? q('SELECT * FROM orders WHERE payment_ref = ?').get(ev.paymentRef) : null
        if (order) {
          q('UPDATE orders SET status = ?, updated_at = ? WHERE id = ?').run(ev.kind, now(), order.id)
          setStatus('order_ref = ?', [order.id], ev.kind === 'refunded' ? 'refunded' : 'revoked', actor, { reason: ev.kind, eventId: event.id })
        } else {
          audit(actor, 'webhook-unknown-payment', null, null, { eventId: event.id, paymentRef: ev.paymentRef })
        }
      }
      return { received: true }
    })
  }, { raw: true })

  // ---------- downloads ----------

  route('POST', /^\/v1\/download-grants$/, ({ req, body }) => {
    const s = session(req)
    const gameId = gameIdOf(body.gameId)
    if (!ownsActive(s.account_id, gameId)) fail(403, 'NOT_ENTITLED', 'This translation is not in your account')
    const p = latestPack(gameId)
    if (!p) fail(404, 'NO_PACK', 'No pack yet')
    limit(`grant:${s.account_id}`, 30, 3600, now())
    const id = randomToken(24)
    const issuedAt = now()
    q('INSERT INTO download_grants (id, account_id, device_id, game_id, version, created_at, expires_at) VALUES (?, ?, ?, ?, ?, ?, ?)').run(sha256hex(id), s.account_id, s.device_id, gameId, p.version, issuedAt, issuedAt + GRANT_TTL)
    audit(`account:${s.account_id}`, 'download-grant', s.account_id, gameId, { version: p.version, deviceId: s.device_id })
    return signer.sign({
      typ: GRANT_TYP, grantId: sha256hex(id).slice(0, 16), accountId: s.account_id, deviceId: s.device_id, gameId,
      version: p.version, sha256: p.sha256, size: p.size, url: `${config.publicUrl}/v1/files/${id}`, issuedAt, expiresAt: issuedAt + GRANT_TTL,
    })
  })

  // The grant's link. In production this is better a presigned storage link (R2/S3) with the same lifetime.
  route('GET', /^\/v1\/files\/([A-Za-z0-9_-]{20,64})$/, ({ match }) => {
    const g = q('SELECT * FROM download_grants WHERE id = ?').get(sha256hex(match[1]))
    if (!g || g.expires_at < now()) fail(410, 'GRANT_EXPIRED', 'Download link expired')
    if (g.downloads >= GRANT_MAX_DOWNLOADS) fail(410, 'GRANT_USED', 'Download link already used')
    if (!ownsActive(g.account_id, g.game_id)) fail(403, 'NOT_ENTITLED', 'No longer in your account')
    const p = q('SELECT * FROM packs WHERE game_id = ? AND version = ?').get(g.game_id, g.version)
    if (!p) fail(404, 'NO_PACK', 'Pack removed')
    q('UPDATE download_grants SET downloads = downloads + 1 WHERE id = ?').run(g.id)
    return { file: packFile(g.game_id, p.file), size: p.size, name: p.file }
  })

  // ---------- admin (the owner) ----------

  route('POST', /^\/v1\/admin\/approve$/, ({ req, body }) => {
    const actor = admin(req)
    const email = emailOf(body.email)
    const gameId = gameIdOf(body.gameId)
    const source = body.source === 'grant' ? 'grant' : 'admin-approved'
    return tx(db, () => {
      const a = ensureAccount(email)
      const already = q("SELECT * FROM entitlements WHERE account_id = ? AND game_id = ? AND status = 'active'").get(a.id, gameId)
      if (already) return { accountId: a.id, gameId, alreadyActive: true, orderRef: already.order_ref }
      const e = grantEntitlement({ accountId: a.id, gameId, source, orderRef: randomId('admin'), actor, note: body.note ? String(body.note).slice(0, 200) : null })
      return { accountId: a.id, gameId, source, orderRef: e.order_ref }
    })
  })

  route('POST', /^\/v1\/admin\/revoke$/, ({ req, body }) => {
    const actor = admin(req)
    const a = accountByEmail(emailOf(body.email))
    const gameId = gameIdOf(body.gameId)
    if (!a) fail(404, 'NO_ACCOUNT', 'No account with that email')
    const n = tx(db, () => setStatus('account_id = ? AND game_id = ?', [a.id, gameId], 'revoked', actor, { reason: String(body.reason || 'admin').slice(0, 200) }))
    return { accountId: a.id, gameId, revoked: n }
  })

  route('POST', /^\/v1\/admin\/sessions\/revoke$/, ({ req, body }) => {
    const actor = admin(req)
    const a = accountByEmail(emailOf(body.email))
    if (!a) fail(404, 'NO_ACCOUNT', 'No account with that email')
    const r = q('UPDATE sessions SET revoked_at = ? WHERE account_id = ? AND revoked_at IS NULL').run(now(), a.id)
    audit(actor, 'sessions-revoked', a.id, null, { count: Number(r.changes) })
    return { accountId: a.id, revoked: Number(r.changes) }
  })

  route('GET', /^\/v1\/admin\/entitlements$/, ({ req, url }) => {
    admin(req)
    const a = accountByEmail(emailOf(url.searchParams.get('email')))
    if (!a) fail(404, 'NO_ACCOUNT', 'No account with that email')
    return { accountId: a.id, email: a.email, entitlements: entitlementList(a.id) }
  })

  route('GET', /^\/v1\/admin\/audit$/, ({ req, url }) => {
    admin(req)
    const n = Math.min(Math.max(Number(url.searchParams.get('limit') || 100), 1), 1000)
    return { entries: q('SELECT * FROM audit ORDER BY id DESC LIMIT ?').all(n).map((e) => ({ ...e, detail: e.detail ? JSON.parse(e.detail) : null })) }
  })

  // A price: { priceId, amount (minor units), currency, active }. With a provider, amount and currency are read
  // from the provider's price so the store never shows something other than what is charged.
  route('PUT', /^\/v1\/admin\/products\/([^/]+)$/, async ({ req, body, match }) => {
    const actor = admin(req)
    const gameId = gameIdOf(decodeURIComponent(match[1]))
    const priceId = body.priceId ? String(body.priceId).slice(0, 100) : null
    let amount = Number.isInteger(body.amount) && body.amount >= 0 ? body.amount : null
    let currency = typeof body.currency === 'string' && /^[A-Za-z]{3}$/.test(body.currency) ? body.currency.toUpperCase() : null
    if (priceId && payments.configured) {
      const p = await payments.lookupPrice(priceId)
      if (!p) fail(400, 'BAD_PRICE', 'The provider has no such one-off price')
      ;({ amount, currency } = p)
    }
    const active = body.active === true && !!priceId && amount != null && !!currency ? 1 : 0
    q('INSERT INTO products (game_id, price_id, amount, currency, active, updated_at) VALUES (?, ?, ?, ?, ?, ?) ON CONFLICT(game_id) DO UPDATE SET price_id = excluded.price_id, amount = excluded.amount, currency = excluded.currency, active = excluded.active, updated_at = excluded.updated_at').run(gameId, priceId, amount, currency, active, now())
    audit(actor, 'product-set', null, gameId, { priceId, amount, currency, active: !!active })
    return { gameId, priceId, amount, currency, active: !!active }
  })

  // A paid pack: the zip must already be at PACKS_DIR/<gameId>/<file>; its checksum and size are computed here.
  route('PUT', /^\/v1\/admin\/packs\/([^/]+)$/, async ({ req, body, match }) => {
    const actor = admin(req)
    const gameId = gameIdOf(decodeURIComponent(match[1]))
    const version = String(body.version || '')
    const file = String(body.file || '')
    if (!/^\d+(\.\d+){0,3}$/.test(version)) fail(400, 'BAD_VERSION', 'version like 1.2.0')
    if (!/^[A-Za-z0-9][A-Za-z0-9._-]{0,120}\.zip$/.test(file)) fail(400, 'BAD_FILE', 'file is a .zip name inside PACKS_DIR/<gameId>/')
    const path = packFile(gameId, file)
    let size
    try {
      size = statSync(path).size
    } catch {
      fail(404, 'NO_FILE', `Put the zip at ${path} first`)
    }
    const h = createHash('sha256')
    for await (const c of createReadStream(path)) h.update(c)
    const sha256 = h.digest('hex')
    q('INSERT INTO packs (game_id, version, file, sha256, size, changelog, published_at) VALUES (?, ?, ?, ?, ?, ?, ?) ON CONFLICT(game_id, version) DO UPDATE SET file = excluded.file, sha256 = excluded.sha256, size = excluded.size, changelog = excluded.changelog, published_at = excluded.published_at').run(gameId, version, file, sha256, size, body.changelog ? String(body.changelog).slice(0, 2000) : null, now())
    audit(actor, 'pack-published', null, gameId, { version, file, sha256, size })
    return { gameId, version, sha256, size }
  })

  function packFile(gameId, file) {
    const root = resolve(config.packsDir)
    const p = resolve(join(root, gameId, file))
    if (!p.startsWith(root + sep)) fail(400, 'BAD_FILE', 'Bad path')
    return p
  }

  // ---------- plumbing ----------

  async function readBody(req) {
    const chunks = []
    let n = 0
    for await (const c of req) {
      n += c.length
      if (n > MAX_BODY) fail(413, 'TOO_LARGE', 'Body too large')
      chunks.push(c)
    }
    return Buffer.concat(chunks)
  }

  const send = (res, status, body, headers = {}) => {
    res.writeHead(status, { 'Cache-Control': 'no-store', 'X-Content-Type-Options': 'nosniff', ...headers })
    res.end(body)
  }

  return async function handle(req, res) {
    const url = new URL(req.url, 'http://x')
    try {
      const r = routes.find((r) => r.method === req.method && r.pattern.test(url.pathname))
      if (!r) fail(404, 'NOT_FOUND', 'No such endpoint')
      const match = url.pathname.match(r.pattern)
      const raw = req.method === 'GET' ? Buffer.alloc(0) : await readBody(req)
      let body = {}
      let form = new URLSearchParams()
      if (r.form) form = new URLSearchParams(raw.toString('utf8'))
      else if (!r.raw && raw.length) {
        try {
          body = JSON.parse(raw.toString('utf8'))
        } catch {
          fail(400, 'BAD_JSON', 'Body is not JSON')
        }
        if (!body || typeof body !== 'object' || Array.isArray(body)) fail(400, 'BAD_JSON', 'Body must be an object')
      }
      const out = await r.handler({ req, url, match, body, form, raw })
      if (out?.html) {
        return send(res, out.status || 200, out.html, {
          'Content-Type': 'text/html; charset=utf-8',
          'Content-Security-Policy': "default-src 'none'; style-src 'unsafe-inline'; form-action 'self'; frame-ancestors 'none'",
          'Referrer-Policy': 'no-referrer',
        })
      }
      if (out?.file) {
        res.writeHead(200, { 'Content-Type': 'application/zip', 'Content-Length': out.size, 'Cache-Control': 'no-store', 'Content-Disposition': `attachment; filename="${out.name}"` })
        return await pipeline(createReadStream(out.file), res)
      }
      send(res, 200, JSON.stringify(out), { 'Content-Type': 'application/json' })
    } catch (e) {
      const known = e instanceof HttpError || (Number.isInteger(e.status) && /^[A-Z_]+$/.test(e.code || ''))
      const status = known ? e.status : 500
      if (!known) log(`[error] ${req.method} ${url.pathname}: ${e.stack || e}`)
      if (res.headersSent) return res.destroy()
      send(res, status, JSON.stringify({ error: known ? { code: e.code, message: e.message } : { code: 'SERVER_ERROR', message: 'Server error' } }), { 'Content-Type': 'application/json' })
    }
  }
}
