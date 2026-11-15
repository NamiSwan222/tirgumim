// Payment providers. The rest of the server only uses this interface, so another provider (Paddle, Lemon Squeezy,
// a local Israeli processor) is one more object with the same methods:
//   configured: boolean
//   name: string
//   createCheckout({ order, product, account }) -> { url, providerRef }
//   lookupPrice(priceId) -> { amount, currency } | null
//   verifyWebhook(rawBody: Buffer, headers, now) -> event | throws
//   interpret(event) -> { kind: 'paid' | 'refunded' | 'disputed' | 'ignore', providerRef?, paymentRef?, amount?, currency? }
import crypto from 'node:crypto'
import { STRIPE_TOLERANCE } from './config.js'

export function notConfigured() {
  return {
    configured: false,
    name: 'none',
    async createCheckout() {
      throw Object.assign(new Error('No payment provider'), { code: 'PAYMENTS_NOT_CONFIGURED', status: 503 })
    },
    async lookupPrice() {
      return null
    },
    verifyWebhook() {
      throw Object.assign(new Error('No payment provider'), { code: 'PAYMENTS_NOT_CONFIGURED', status: 503 })
    },
    interpret() {
      return { kind: 'ignore' }
    },
  }
}

/** Stripe Checkout through its REST API (no SDK). `fetchImpl` is replaceable for tests. */
export function stripe({ secretKey, webhookSecret, successUrl, cancelUrl }, fetchImpl = fetch) {
  if (!secretKey || !webhookSecret) return notConfigured()
  const api = async (method, path, form) => {
    const res = await fetchImpl(`https://api.stripe.com${path}`, {
      method,
      headers: { Authorization: `Bearer ${secretKey}`, 'Content-Type': 'application/x-www-form-urlencoded' },
      body: form ? new URLSearchParams(form).toString() : undefined,
    })
    const body = await res.json().catch(() => ({}))
    if (!res.ok) throw Object.assign(new Error(body?.error?.message || `Stripe ${res.status}`), { code: 'PAYMENT_PROVIDER', status: 502 })
    return body
  }
  return {
    configured: true,
    name: 'stripe',
    async createCheckout({ order, product, account }) {
      const meta = { orderId: order.id, accountId: account.id, gameId: order.game_id }
      const form = {
        mode: 'payment',
        'line_items[0][price]': product.price_id,
        'line_items[0][quantity]': '1',
        client_reference_id: order.id,
        customer_email: account.email,
        success_url: successUrl,
        cancel_url: cancelUrl,
      }
      for (const [k, v] of Object.entries(meta)) {
        form[`metadata[${k}]`] = v
        form[`payment_intent_data[metadata][${k}]`] = v
      }
      const s = await api('POST', '/v1/checkout/sessions', form)
      return { url: s.url, providerRef: s.id }
    },
    async lookupPrice(priceId) {
      const p = await api('GET', `/v1/prices/${encodeURIComponent(priceId)}`)
      return typeof p.unit_amount === 'number' ? { amount: p.unit_amount, currency: String(p.currency).toUpperCase() } : null
    },
    /** Stripe-Signature: t=<unix>,v1=<hex HMAC-SHA256 of "<t>.<raw body>">. */
    verifyWebhook(rawBody, headers, now) {
      const header = String(headers['stripe-signature'] || '')
      const parts = header.split(',').map((p) => p.split('='))
      const t = Number(parts.find(([k]) => k === 't')?.[1])
      const sigs = parts.filter(([k]) => k === 'v1').map(([, v]) => v)
      if (!t || !sigs.length) throw Object.assign(new Error('No Stripe signature'), { code: 'BAD_SIGNATURE', status: 400 })
      if (Math.abs(now - t) > STRIPE_TOLERANCE) throw Object.assign(new Error('Stripe signature too old'), { code: 'BAD_SIGNATURE', status: 400 })
      const expected = crypto.createHmac('sha256', webhookSecret).update(`${t}.`).update(rawBody).digest()
      const ok = sigs.some((s) => {
        const got = Buffer.from(s, 'hex')
        return got.length === expected.length && crypto.timingSafeEqual(got, expected)
      })
      if (!ok) throw Object.assign(new Error('Stripe signature does not match'), { code: 'BAD_SIGNATURE', status: 400 })
      return JSON.parse(rawBody.toString('utf8'))
    },
    interpret(event) {
      const o = event?.data?.object || {}
      switch (event.type) {
        case 'checkout.session.completed':
        case 'checkout.session.async_payment_succeeded':
          if (o.payment_status !== 'paid') return { kind: 'ignore' }
          return { kind: 'paid', providerRef: o.id, paymentRef: o.payment_intent || null, amount: o.amount_total, currency: o.currency?.toUpperCase() }
        case 'charge.refunded':
          // Only a full refund takes the translation away; a partial one (a goodwill discount) doesn't.
          if (!o.refunded) return { kind: 'ignore' }
          return { kind: 'refunded', paymentRef: o.payment_intent }
        case 'charge.dispute.created':
          return { kind: 'disputed', paymentRef: o.payment_intent }
        default:
          return { kind: 'ignore' }
      }
    },
  }
}
