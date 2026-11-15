import { test } from 'node:test'
import assert from 'node:assert/strict'
import crypto from 'node:crypto'
import { createServer } from 'node:http'
import { mkdtempSync, mkdirSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { createApp, ENTITLEMENTS_TYP, GRANT_TYP } from '../src/app.js'
import { loadConfig } from '../src/config.js'
import { makeSigner, verifyEnvelope } from '../src/crypto.js'
import { openDb } from '../src/db.js'
import { notConfigured, stripe } from '../src/payments.js'

const ADMIN = 'a'.repeat(40)
const WEBHOOK_SECRET = 'whsec_test_secret'
const SEED = Buffer.alloc(32, 7).toString('base64url') // test key only

/** A server on a free port with an in-memory database and a clock the test moves. */
async function start({ withStripe = false } = {}) {
  const packsDir = mkdtempSync(join(tmpdir(), 'tirgumim-packs-'))
  const clock = { now: 1_800_000_000 }
  const mails = []
  const stripeCalls = []
  const fakeFetch = async (url, init) => {
    stripeCalls.push({ url, init })
    if (url.includes('/v1/checkout/sessions')) return new Response(JSON.stringify({ id: `cs_test_${stripeCalls.length}`, url: 'https://checkout.stripe.com/c/pay/cs_test' }))
    if (url.includes('/v1/prices/')) return new Response(JSON.stringify({ id: 'price_1', unit_amount: 2990, currency: 'ils' }))
    return new Response('{}', { status: 404 })
  }
  const config = {
    ...loadConfig({ ADMIN_TOKEN: ADMIN, PACKS_DIR: packsDir, PUBLIC_URL: 'http://127.0.0.1:1' }),
    stripe: { secretKey: 'sk_test_x', webhookSecret: WEBHOOK_SECRET, successUrl: 'https://x/done', cancelUrl: 'https://x/cancel' },
  }
  const signer = makeSigner(SEED, 'k1')
  const app = createApp({
    config,
    db: openDb(':memory:'),
    signer,
    payments: withStripe ? stripe(config.stripe, fakeFetch) : notConfigured(),
    mailer: { configured: true, sendSignIn: async (m) => mails.push(m) },
    now: () => clock.now,
    log: () => {},
  })
  const server = createServer(app)
  await new Promise((r) => server.listen(0, '127.0.0.1', r))
  const base = `http://127.0.0.1:${server.address().port}`
  config.publicUrl = base
  const call = async (method, path, { body, token, raw, headers = {} } = {}) => {
    const res = await fetch(base + path, {
      method,
      headers: { ...(token ? { Authorization: `Bearer ${token}` } : {}), ...(body ? { 'Content-Type': 'application/json' } : {}), ...headers },
      body: raw ?? (body ? JSON.stringify(body) : undefined),
    })
    const text = await res.text()
    let json = null
    try {
      json = JSON.parse(text)
    } catch {}
    return { status: res.status, json, text, res }
  }
  const adminCall = (method, path, body) => call(method, path, { body, token: ADMIN })
  const signIn = async (email = 'player@example.com', deviceId = 'dev_0123456789abcdef') => {
    const secret = crypto.randomBytes(32).toString('hex')
    const pollHash = crypto.createHash('sha256').update(secret).digest('hex')
    const s = await call('POST', '/v1/auth/start', { body: { email, deviceId, pollHash, deviceName: 'PC' } })
    assert.equal(s.status, 200, s.text)
    const link = new URL(mails.at(-1).link)
    const t = link.searchParams.get('t')
    const page = await call('GET', `/v1/auth/verify?t=${t}`)
    assert.ok(page.text.includes(s.json.code), 'the page shows the same code')
    assert.equal((await call('POST', '/v1/auth/poll', { body: { requestId: s.json.requestId, pollSecret: secret } })).json.status, 'pending')
    const ok = await call('POST', '/v1/auth/verify', { raw: `t=${t}`, headers: { 'Content-Type': 'application/x-www-form-urlencoded' } })
    assert.equal(ok.status, 200)
    const p = await call('POST', '/v1/auth/poll', { body: { requestId: s.json.requestId, pollSecret: secret } })
    assert.equal(p.json.status, 'approved')
    return { token: p.json.sessionToken, accountId: p.json.accountId, deviceId, requestId: s.json.requestId, secret, t }
  }
  return { base, call, adminCall, signIn, mails, clock, signer, packsDir, stripeCalls, close: () => server.close() }
}

function stripeEvent(s, event) {
  const raw = JSON.stringify(event)
  const t = s.clock.now
  const sig = crypto.createHmac('sha256', WEBHOOK_SECRET).update(`${t}.${raw}`).digest('hex')
  return s.call('POST', '/v1/webhooks/stripe', { raw, headers: { 'Stripe-Signature': `t=${t},v1=${sig}`, 'Content-Type': 'application/json' } })
}

test('email sign-in: link, code, one-time poll, sign out', async () => {
  const s = await start()
  try {
    const a = await s.signIn()
    assert.match(a.accountId, /^acc_/)
    assert.equal((await s.call('GET', '/v1/me', { token: a.token })).json.email, 'player@example.com')
    // the poll secret is needed, and the session is handed out once
    assert.equal((await s.call('POST', '/v1/auth/poll', { body: { requestId: a.requestId, pollSecret: 'wrong' } })).status, 404)
    assert.equal((await s.call('POST', '/v1/auth/poll', { body: { requestId: a.requestId, pollSecret: a.secret } })).status, 410)
    // the link works once
    assert.equal((await s.call('GET', `/v1/auth/verify?t=${a.t}`)).status, 410)
    // signing out ends the session on the server
    assert.equal((await s.call('POST', '/v1/auth/signout', { token: a.token })).status, 200)
    assert.equal((await s.call('GET', '/v1/me', { token: a.token })).status, 401)
    assert.equal((await s.call('GET', '/v1/me', { token: 'made-up' })).status, 401)
    assert.equal((await s.call('POST', '/v1/auth/start', { body: { email: 'nope', deviceId: 'dev_0123456789abcdef', pollHash: 'a'.repeat(64) } })).json.error.code, 'BAD_EMAIL')
  } finally {
    s.close()
  }
})

test('sign-in links expire and requests are rate limited', async () => {
  const s = await start()
  try {
    const body = { email: 'slow@example.com', deviceId: 'dev_0123456789abcdef', pollHash: 'b'.repeat(64) }
    const r = await s.call('POST', '/v1/auth/start', { body })
    s.clock.now += 16 * 60
    const t = new URL(s.mails.at(-1).link).searchParams.get('t')
    assert.equal((await s.call('POST', '/v1/auth/verify', { raw: `t=${t}`, headers: { 'Content-Type': 'application/x-www-form-urlencoded' } })).status, 410)
    assert.equal((await s.call('POST', '/v1/auth/poll', { body: { requestId: r.json.requestId, pollSecret: 'x' } })).status, 404)
    // the first request is out of the 15-minute window now: five more are allowed
    for (let i = 0; i < 5; i++) assert.equal((await s.call('POST', '/v1/auth/start', { body })).status, 200)
    assert.equal((await s.call('POST', '/v1/auth/start', { body })).json.error.code, 'RATE_LIMITED')
  } finally {
    s.close()
  }
})

test('owner approves and revokes; signed list; audit trail', async () => {
  const s = await start()
  try {
    // admin API needs the token
    assert.equal((await s.call('POST', '/v1/admin/approve', { body: { email: 'player@example.com', gameId: 'paid-game' } })).status, 401)
    assert.equal((await s.call('POST', '/v1/admin/approve', { body: { email: 'player@example.com', gameId: 'paid-game' }, token: 'b'.repeat(40) })).status, 401)
    // approving before the player ever signed in creates the account
    const ap = await s.adminCall('POST', '/v1/admin/approve', { email: 'Player@Example.com', gameId: 'paid-game', note: 'tester' })
    assert.equal(ap.json.source, 'admin-approved')
    assert.equal((await s.adminCall('POST', '/v1/admin/approve', { email: 'player@example.com', gameId: 'paid-game' })).json.alreadyActive, true)

    const a = await s.signIn()
    assert.equal(a.accountId, ap.json.accountId)
    const env = (await s.call('GET', '/v1/entitlements', { token: a.token })).json
    const doc = verifyEnvelope(env, s.signer.publicKey)
    assert.equal(doc.typ, ENTITLEMENTS_TYP)
    assert.equal(doc.deviceId, a.deviceId)
    assert.equal(doc.expiresAt - doc.issuedAt, 14 * 86400)
    assert.deepEqual(doc.entitlements.map((e) => [e.gameId, e.source, e.status]), [['paid-game', 'admin-approved', 'active']])
    // a changed payload no longer verifies
    assert.throws(() => verifyEnvelope({ ...env, payload: Buffer.from(JSON.stringify({ ...doc, accountId: 'acc_x' })).toString('base64url') }, s.signer.publicKey))

    assert.equal((await s.adminCall('POST', '/v1/admin/revoke', { email: 'player@example.com', gameId: 'paid-game', reason: 'chargeback' })).json.revoked, 1)
    const after = verifyEnvelope((await s.call('GET', '/v1/entitlements', { token: a.token })).json, s.signer.publicKey)
    assert.equal(after.entitlements[0].status, 'revoked')

    const audit = (await s.adminCall('GET', '/v1/admin/audit?limit=50')).json.entries.map((e) => e.action)
    for (const action of ['account-created', 'entitlement-granted', 'sign-in-approved', 'session-created', 'entitlement-revoked']) assert.ok(audit.includes(action), action)
    assert.equal((await s.adminCall('GET', '/v1/admin/entitlements?email=player@example.com')).json.entitlements.length, 1)
    // the owner can end every session of an account (stolen PC)
    assert.equal((await s.adminCall('POST', '/v1/admin/sessions/revoke', { email: 'player@example.com' })).json.revoked, 1)
    assert.equal((await s.call('GET', '/v1/entitlements', { token: a.token })).status, 401)
  } finally {
    s.close()
  }
})

test('download grants: only for owners, short-lived, checksum signed', async () => {
  const s = await start()
  try {
    mkdirSync(join(s.packsDir, 'paid-game'))
    const zip = Buffer.from('PK fake zip bytes')
    writeFileSync(join(s.packsDir, 'paid-game', 'paid-game-he-1.0.0.zip'), zip)
    assert.equal((await s.adminCall('PUT', '/v1/admin/packs/paid-game', { version: '1.0.0', file: '../escape.zip' })).status, 400)
    const pub = await s.adminCall('PUT', '/v1/admin/packs/paid-game', { version: '1.0.0', file: 'paid-game-he-1.0.0.zip', changelog: 'first' })
    assert.equal(pub.json.sha256, crypto.createHash('sha256').update(zip).digest('hex'))
    // public facts about the pack (no link)
    const info = (await s.call('GET', '/v1/packs/paid-game')).json
    assert.equal(info.version, '1.0.0')
    assert.equal(info.url, undefined)

    const a = await s.signIn()
    assert.equal((await s.call('POST', '/v1/download-grants', { token: a.token, body: { gameId: 'paid-game' } })).json.error.code, 'NOT_ENTITLED')
    await s.adminCall('POST', '/v1/admin/approve', { email: 'player@example.com', gameId: 'paid-game' })
    const env = (await s.call('POST', '/v1/download-grants', { token: a.token, body: { gameId: 'paid-game' } })).json
    const g = verifyEnvelope(env, s.signer.publicKey)
    assert.equal(g.typ, GRANT_TYP)
    assert.equal(g.sha256, pub.json.sha256)
    assert.equal(g.expiresAt - g.issuedAt, 600)
    const path = new URL(g.url).pathname
    const file = await fetch(s.base + path)
    assert.equal(file.status, 200)
    assert.deepEqual(Buffer.from(await file.arrayBuffer()), zip)
    // a made-up link, an expired one, and one for a revoked owner all fail
    assert.equal((await s.call('GET', '/v1/files/' + 'x'.repeat(32))).status, 410)
    s.clock.now += 601
    assert.equal((await s.call('GET', path)).json.error.code, 'GRANT_EXPIRED')
    const env2 = (await s.call('POST', '/v1/download-grants', { token: a.token, body: { gameId: 'paid-game' } })).json
    await s.adminCall('POST', '/v1/admin/revoke', { email: 'player@example.com', gameId: 'paid-game' })
    assert.equal((await s.call('GET', new URL(verifyEnvelope(env2, s.signer.publicKey).url).pathname)).json.error.code, 'NOT_ENTITLED')
  } finally {
    s.close()
  }
})

test('no payment provider: store says so, checkout refuses clearly', async () => {
  const s = await start()
  try {
    await s.adminCall('PUT', '/v1/admin/products/paid-game', { priceId: null, active: true })
    const store = (await s.call('GET', '/v1/store')).json
    assert.equal(store.paymentsConfigured, false)
    assert.deepEqual(store.products, []) // no price: not active
    const a = await s.signIn()
    assert.equal((await s.call('POST', '/v1/checkout', { token: a.token, body: { gameId: 'paid-game' } })).json.error.code, 'PAYMENTS_NOT_CONFIGURED')
    assert.equal((await s.call('POST', '/v1/webhooks/stripe', { raw: '{}' })).status, 503)
  } finally {
    s.close()
  }
})

test('stripe: checkout, verified idempotent webhook, refund revokes', async () => {
  const s = await start({ withStripe: true })
  try {
    // the price is read from Stripe, not typed in
    const prod = await s.adminCall('PUT', '/v1/admin/products/paid-game', { priceId: 'price_1', amount: 1, currency: 'USD', active: true })
    assert.deepEqual([prod.json.amount, prod.json.currency, prod.json.active], [2990, 'ILS', true])
    assert.deepEqual((await s.call('GET', '/v1/store')).json.products, [{ gameId: 'paid-game', priceId: 'price_1', amount: 2990, currency: 'ILS', active: true }])

    const a = await s.signIn()
    assert.equal((await s.call('POST', '/v1/checkout', { token: a.token, body: { gameId: 'other-game' } })).json.error.code, 'NOT_FOR_SALE')
    const co = await s.call('POST', '/v1/checkout', { token: a.token, body: { gameId: 'paid-game' } })
    assert.equal(co.json.url, 'https://checkout.stripe.com/c/pay/cs_test')
    const sent = new URLSearchParams(s.stripeCalls.at(-1).init.body)
    assert.equal(sent.get('line_items[0][price]'), 'price_1')
    assert.equal(sent.get('metadata[accountId]'), a.accountId)
    const sessionId = `cs_test_${s.stripeCalls.length}`

    const paid = { id: 'evt_1', type: 'checkout.session.completed', data: { object: { id: sessionId, payment_status: 'paid', payment_intent: 'pi_1', amount_total: 2990, currency: 'ils' } } }
    // a forged or stale signature is refused
    const bad = await s.call('POST', '/v1/webhooks/stripe', { raw: JSON.stringify(paid), headers: { 'Stripe-Signature': `t=${s.clock.now},v1=${'0'.repeat(64)}` } })
    assert.equal(bad.status, 400)
    assert.equal((await stripeEvent(s, paid)).status, 200)
    assert.equal((await stripeEvent(s, paid)).json.duplicate, true)
    let doc = verifyEnvelope((await s.call('GET', '/v1/entitlements', { token: a.token })).json, s.signer.publicKey)
    assert.equal(doc.entitlements.length, 1, 'one entitlement however often the event comes')
    assert.deepEqual([doc.entitlements[0].source, doc.entitlements[0].status], ['purchase', 'active'])
    assert.equal((await s.call('POST', '/v1/checkout', { token: a.token, body: { gameId: 'paid-game' } })).json.error.code, 'ALREADY_OWNED')

    // a partial refund keeps it; a full one takes it away
    assert.equal((await stripeEvent(s, { id: 'evt_2', type: 'charge.refunded', data: { object: { payment_intent: 'pi_1', refunded: false } } })).status, 200)
    doc = verifyEnvelope((await s.call('GET', '/v1/entitlements', { token: a.token })).json, s.signer.publicKey)
    assert.equal(doc.entitlements[0].status, 'active')
    await stripeEvent(s, { id: 'evt_3', type: 'charge.refunded', data: { object: { payment_intent: 'pi_1', refunded: true } } })
    doc = verifyEnvelope((await s.call('GET', '/v1/entitlements', { token: a.token })).json, s.signer.publicKey)
    assert.equal(doc.entitlements[0].status, 'refunded')
    const audit = (await s.adminCall('GET', '/v1/admin/audit')).json.entries.map((e) => `${e.actor} ${e.action}`)
    assert.ok(audit.includes('webhook:stripe entitlement-granted'))
    assert.ok(audit.includes('webhook:stripe entitlement-refunded'))

    // an old signature (replay) is refused
    const raw = JSON.stringify({ ...paid, id: 'evt_9' })
    const t = s.clock.now - 3600
    const sig = crypto.createHmac('sha256', WEBHOOK_SECRET).update(`${t}.${raw}`).digest('hex')
    assert.equal((await s.call('POST', '/v1/webhooks/stripe', { raw, headers: { 'Stripe-Signature': `t=${t},v1=${sig}` } })).status, 400)
  } finally {
    s.close()
  }
})

test('the signature format matches what the launcher verifies', () => {
  // Same test seed as the launcher's Rust tests (`test_keys::signing(7)`): Ed25519 is deterministic, so both sides
  // must produce this exact public key.
  const signer = makeSigner(SEED, 'k1')
  assert.equal(signer.publicKey, '6kpsY-KcUgq-9VB7Ey7F-ZVHdq6-vnuSQh7qaRRG0iw')
  const env = signer.sign({ typ: ENTITLEMENTS_TYP, n: 1 })
  assert.deepEqual(Object.keys(env), ['kid', 'payload', 'sig'])
  assert.equal(Buffer.from(env.sig, 'base64url').length, 64)
})
