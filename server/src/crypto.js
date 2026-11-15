// Signing (Ed25519), random tokens, hashing. Everything secret comes from the environment.
import crypto from 'node:crypto'

// DER prefixes that wrap a raw 32-byte Ed25519 seed / public key.
const PKCS8_PREFIX = Buffer.from('302e020100300506032b657004220420', 'hex')
const SPKI_PREFIX_LEN = 12

export const b64url = (buf) => Buffer.from(buf).toString('base64url')
export const randomToken = (bytes = 32) => crypto.randomBytes(bytes).toString('base64url')
export const randomId = (prefix, bytes = 12) => `${prefix}_${crypto.randomBytes(bytes).toString('hex')}`
export const sha256hex = (s) => crypto.createHash('sha256').update(s).digest('hex')

/** Constant-time compare of two strings (by their hashes, so lengths don't leak either). */
export function safeEqual(a, b) {
  if (typeof a !== 'string' || typeof b !== 'string') return false
  return crypto.timingSafeEqual(crypto.createHash('sha256').update(a).digest(), crypto.createHash('sha256').update(b).digest())
}

/** A signer from a base64url 32-byte seed (ENTITLEMENT_SIGNING_KEY). */
export function makeSigner(seedB64, kid) {
  const seed = Buffer.from(String(seedB64 || '').trim(), 'base64url')
  if (seed.length !== 32) throw new Error('ENTITLEMENT_SIGNING_KEY must be 32 bytes, base64url (npm run keygen)')
  const privateKey = crypto.createPrivateKey({ key: Buffer.concat([PKCS8_PREFIX, seed]), format: 'der', type: 'pkcs8' })
  const publicRaw = crypto.createPublicKey(privateKey).export({ format: 'der', type: 'spki' }).subarray(SPKI_PREFIX_LEN)
  return {
    kid,
    publicKey: b64url(publicRaw),
    /** The envelope the launcher verifies: base64url JSON bytes + a signature over exactly those bytes. */
    sign(payload) {
      const bytes = Buffer.from(JSON.stringify(payload), 'utf8')
      return { kid, payload: b64url(bytes), sig: b64url(crypto.sign(null, bytes, privateKey)) }
    },
  }
}

/** For tests and tools: checks an envelope against a raw base64url public key. */
export function verifyEnvelope(env, publicKeyB64) {
  const key = crypto.createPublicKey({ key: Buffer.concat([Buffer.from('302a300506032b6570032100', 'hex'), Buffer.from(publicKeyB64, 'base64url')]), format: 'der', type: 'spki' })
  const bytes = Buffer.from(env.payload, 'base64url')
  if (!crypto.verify(null, bytes, key, Buffer.from(env.sig, 'base64url'))) throw new Error('bad signature')
  return JSON.parse(bytes.toString('utf8'))
}
