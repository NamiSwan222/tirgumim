// Settings, all from environment variables (see .env.example). Nothing secret has a default.

export function loadConfig(env = process.env) {
  const port = Number(env.PORT || 8787)
  const adminToken = String(env.ADMIN_TOKEN || '')
  return {
    port,
    publicUrl: String(env.PUBLIC_URL || `http://127.0.0.1:${port}`).replace(/\/+$/, ''),
    trustProxy: env.TRUST_PROXY === 'true',
    dbPath: env.DATABASE_PATH || './data/tirgumim.db',
    packsDir: env.PACKS_DIR || './packs',
    signingKey: env.ENTITLEMENT_SIGNING_KEY || '',
    keyId: env.ENTITLEMENT_KEY_ID || 'k1',
    entitlementTtl: Math.min(Number(env.ENTITLEMENT_TTL_SECONDS || 14 * 86400), 14 * 86400),
    // A short admin token is as good as none: the admin API stays off.
    adminToken: adminToken.length >= 32 ? adminToken : '',
    stripe: {
      secretKey: env.STRIPE_SECRET_KEY || '',
      webhookSecret: env.STRIPE_WEBHOOK_SECRET || '',
      successUrl: env.CHECKOUT_SUCCESS_URL || '',
      cancelUrl: env.CHECKOUT_CANCEL_URL || '',
    },
    mailFrom: env.MAIL_FROM || '',
  }
}

// Lifetimes (seconds).
export const SIGN_IN_LINK_TTL = 15 * 60
export const SESSION_TTL = 90 * 86400
export const GRANT_TTL = 10 * 60
export const GRANT_MAX_DOWNLOADS = 3
export const STRIPE_TOLERANCE = 5 * 60
