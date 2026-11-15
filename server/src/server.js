// Starts the account server: `npm start` (reads .env when there is one).
import { createServer } from 'node:http'
import { loadConfig } from './config.js'
import { makeSigner } from './crypto.js'
import { openDb } from './db.js'
import { logMailer } from './mail.js'
import { stripe } from './payments.js'
import { createApp } from './app.js'

const config = loadConfig()
const signer = makeSigner(config.signingKey, config.keyId)
const payments = stripe(config.stripe)
const mailer = logMailer()
if (payments.configured && (!config.stripe.successUrl.startsWith('https://') || !config.stripe.cancelUrl.startsWith('https://'))) {
  throw new Error('CHECKOUT_SUCCESS_URL and CHECKOUT_CANCEL_URL must be https when Stripe is set up')
}
const app = createApp({ config, db: openDb(config.dbPath), signer, payments, mailer })

createServer(app).listen(config.port, () => {
  console.log(`Tirgumim account server on :${config.port} (${config.publicUrl})`)
  console.log(`  launcher key: TIRGUMIM_ENTITLEMENT_KEYS=${signer.kid}:${signer.publicKey}`)
  console.log(`  payments: ${payments.configured ? payments.name : 'not configured (store shows "not for sale yet")'}`)
  console.log(`  admin API: ${config.adminToken ? 'on' : 'off (set ADMIN_TOKEN, 32+ characters)'}`)
  if (!mailer.configured) console.log('  mail: no provider, sign-in links are printed here. Not for real players.')
  if (!config.publicUrl.startsWith('https://')) console.log('  WARNING: PUBLIC_URL is not https. Only for testing on this PC.')
})
