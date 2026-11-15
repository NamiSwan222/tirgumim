// SQLite (node:sqlite, no native modules to install). One file; WAL so reads don't wait on writes.
import { DatabaseSync } from 'node:sqlite'
import { mkdirSync } from 'node:fs'
import { dirname } from 'node:path'

const SCHEMA = `
CREATE TABLE IF NOT EXISTS accounts (
  id TEXT PRIMARY KEY,
  email TEXT NOT NULL UNIQUE,
  created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS auth_requests (
  id TEXT PRIMARY KEY,
  email TEXT NOT NULL,
  link_hash TEXT NOT NULL UNIQUE,
  poll_hash TEXT NOT NULL,
  code TEXT NOT NULL,
  device_id TEXT NOT NULL,
  device_name TEXT,
  ip TEXT,
  status TEXT NOT NULL,              -- pending | approved | consumed
  account_id TEXT,
  created_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS sessions (
  token_hash TEXT PRIMARY KEY,       -- sha256 of the token: a database leak gives no usable sessions
  account_id TEXT NOT NULL REFERENCES accounts(id),
  device_id TEXT NOT NULL,
  device_name TEXT,
  created_at INTEGER NOT NULL,
  last_used_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL,
  revoked_at INTEGER
);
CREATE TABLE IF NOT EXISTS entitlements (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  account_id TEXT NOT NULL REFERENCES accounts(id),
  game_id TEXT NOT NULL,
  source TEXT NOT NULL,              -- purchase | grant | admin-approved
  status TEXT NOT NULL,              -- active | revoked | refunded
  granted_at INTEGER NOT NULL,
  order_ref TEXT NOT NULL UNIQUE,    -- our order id, or admin-<id>: makes every grant idempotent
  changed_at INTEGER NOT NULL,
  note TEXT
);
CREATE INDEX IF NOT EXISTS entitlements_account ON entitlements(account_id, game_id);
CREATE TABLE IF NOT EXISTS products (
  game_id TEXT PRIMARY KEY,
  price_id TEXT,
  amount INTEGER,                    -- minor units
  currency TEXT,
  active INTEGER NOT NULL DEFAULT 0,
  updated_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS packs (
  game_id TEXT NOT NULL,
  version TEXT NOT NULL,
  file TEXT NOT NULL,
  sha256 TEXT NOT NULL,
  size INTEGER NOT NULL,
  changelog TEXT,
  published_at INTEGER NOT NULL,
  PRIMARY KEY (game_id, version)
);
CREATE TABLE IF NOT EXISTS orders (
  id TEXT PRIMARY KEY,
  account_id TEXT NOT NULL REFERENCES accounts(id),
  game_id TEXT NOT NULL,
  provider TEXT NOT NULL,
  provider_ref TEXT UNIQUE,          -- Stripe Checkout Session id
  payment_ref TEXT,                  -- Stripe PaymentIntent id (refunds and disputes name it)
  status TEXT NOT NULL,              -- created | paid | refunded | disputed
  amount INTEGER,
  currency TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS orders_payment ON orders(payment_ref);
CREATE TABLE IF NOT EXISTS download_grants (
  id TEXT PRIMARY KEY,
  account_id TEXT NOT NULL,
  device_id TEXT NOT NULL,
  game_id TEXT NOT NULL,
  version TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL,
  downloads INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS webhook_events (
  id TEXT PRIMARY KEY,               -- the provider's event id: each event is applied once
  provider TEXT NOT NULL,
  type TEXT NOT NULL,
  received_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS audit (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  at INTEGER NOT NULL,
  actor TEXT NOT NULL,               -- admin:<name> | webhook:stripe | account:<id> | system
  action TEXT NOT NULL,
  account_id TEXT,
  game_id TEXT,
  detail TEXT
);
`

export function openDb(path) {
  if (path !== ':memory:') mkdirSync(dirname(path), { recursive: true })
  const db = new DatabaseSync(path)
  db.exec('PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000;')
  db.exec(SCHEMA)
  return db
}

/** Runs `fn` in one transaction (all or nothing). */
export function tx(db, fn) {
  db.exec('BEGIN IMMEDIATE')
  try {
    const r = fn()
    db.exec('COMMIT')
    return r
  } catch (e) {
    db.exec('ROLLBACK')
    throw e
  }
}
