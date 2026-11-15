// Prints a new Ed25519 signing key for the account server and the public key to build into the launcher.
//   npm run keygen [-- k2]
import crypto from 'node:crypto'
import { makeSigner } from '../src/crypto.js'

const kid = process.argv[2] || 'k1'
const seed = crypto.randomBytes(32).toString('base64url')
const { publicKey } = makeSigner(seed, kid)
console.log(`# Server (secret: .env or the host's secret manager, never git):
ENTITLEMENT_SIGNING_KEY=${seed}
ENTITLEMENT_KEY_ID=${kid}

# Launcher (public: write into ENTITLEMENT_KEYS in src-tauri/src/core/account.rs, or set at build time):
TIRGUMIM_ENTITLEMENT_KEYS=${kid}:${publicKey}`)
