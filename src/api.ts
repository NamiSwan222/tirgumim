// What the Rust side (src-tauri/src/lib.rs) answers, and the calls the window makes.
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'

export type Status = 'notFound' | 'noPack' | 'unavailable' | 'ready' | 'installed' | 'updateAvailable' | 'needsRepair' | 'playing' | 'needsLauncher'

export interface Pack {
  version: string
  url: string
  sha256: string
  size: number | null
  changelog: string | null
  publishedAt: string | null
  source: 'catalog' | 'github'
}

export interface Game {
  id: string
  name: string
  nameHe: string | null
  aboutHe: string | null
  engine: string | null
  inProgress: boolean
  cover: string
  hero: string
  status: Status
  location: { dir: string; source: 'installed' | 'custom' | 'steam' | 'epic' } | null
  available: Pack | null
  packError: string | null
  /** The game's program that is running now, if any. */
  running: string | null
  /** Launch-only translation: Hebrew only when started from the launcher. */
  launchOnly: 'args' | 'swap' | null
  minLauncherVersion: string | null
  needs: { message: string; url: string | null }[]
  installed: {
    version: string
    installedAt: string
    gameVersion: string | null
    forced: boolean
    changed: string[]
    files: number
  } | null
}

export interface PcGame {
  store: 'steam' | 'epic'
  id: string
  name: string
  dir: string
  cover: string
  hero: string
}

export interface Library {
  translations: Game[]
  others: PcGame[]
  catalog: { source: 'github' | 'bundled' | 'dev' | 'custom' | null; error: string | null; updated: string | null }
}

export interface PackError {
  code: string
  message: string
  allowUnknown?: boolean
  details?: string[]
}

export interface Progress {
  gameId: string
  step: 'download' | 'verify' | 'patch' | 'backup' | 'copy' | 'done'
  fraction: number | null
}

export interface Session {
  gameId: string
  state: 'started' | 'running' | 'restored' | 'restoreFailed' | 'neverStarted'
  leftChanged?: string[]
  error?: PackError
}

export interface UninstallResult {
  restored: string[]
  deleted: string[]
  skippedChanged: string[]
}

export const api = {
  info: () => invoke<{ version: string; dev: boolean }>('app_info'),
  list: (reload: boolean) => invoke<Library>('list', { reload }),
  install: (gameId: string, force = false) => invoke('install', { gameId, force }),
  update: (gameId: string, force = false) => invoke('update', { gameId, force }),
  uninstall: (gameId: string) => invoke<UninstallResult>('uninstall', { gameId }),
  play: (gameId: string) => invoke<{ startingSteam: boolean; via: 'steam' | 'direct'; watched: boolean }>('play', { gameId }),
  closeWhenGameExits: () => invoke('close_when_game_exits'),
  onSession: (fn: (s: Session) => void) => listen<Session>('session', (e) => fn(e.payload)),
  onCloseBlocked: (fn: () => void) => listen('close-blocked', () => fn()),
  pickFolder: (gameId: string) => invoke<string | null>('pick_folder', { gameId }),
  openFolder: (gameId: string) => invoke('open_folder', { gameId }),
  openUrl: (url: string) => invoke('open_url', { url }),
  getSettings: () => invoke<{ catalogUrl: string; defaultCatalogUrl: string }>('get_settings'),
  setSettings: (catalogUrl: string) => invoke('set_settings', { catalogUrl }),
  onProgress: (fn: (p: Progress) => void) => listen<Progress>('progress', (e) => fn(e.payload)),
}

export function asError(e: unknown): PackError {
  if (e && typeof e === 'object' && 'code' in e) return e as PackError
  return { code: 'ERROR', message: String(e) }
}

// ---------- accounts, paid translations, store (ENTITLEMENTS.md) ----------
// Free games need none of this: with no account server built in, everything here answers "not configured" and the
// rest of the launcher works as before.

/** Whether this account may use a translation. Free games are always 'free'. */
export type Access =
  | 'free'
  | 'owned'
  | 'notOwned'
  | 'revoked'
  | 'refunded'
  | 'signedOut'
  | 'checking'
  | 'expired'
  | 'clockChanged'
  | 'notConfigured'

// Fields `list()` now also sends for every translation (declaration merging with `Game` above).
export interface Game {
  /** A paid translation: install, update and play need `access === 'owned'`. */
  paid: boolean
  /** The provider's price id from the catalog (a hint; the store answer is what counts). */
  priceId: string | null
  access: Access
}

export interface AccountState {
  /** The launcher has an account server and its public keys built in. */
  configured: boolean
  status: 'notConfigured' | 'signedOut' | 'pending' | 'signedIn'
  email: string | null
  accountId: string | null
  /** While waiting for the email link: the code the browser page also shows (ask the player to compare). */
  pending: { email: string; code: string; expiresAt: number } | null
}

export interface AccountEvent {
  event: 'signedIn' | 'signInExpired' | 'signInFailed' | 'signedOut'
  state: AccountState
  error?: PackError
}

export interface Entitlement {
  gameId: string
  source: 'purchase' | 'grant' | 'admin-approved'
  status: 'active' | 'revoked' | 'refunded'
  grantedAt: string
  orderRef: string | null
}

export interface Entitlements {
  state: 'notConfigured' | 'signedOut' | 'none' | 'valid' | 'expired' | 'clockChanged'
  /** Unix seconds (server clock). */
  issuedAt: number | null
  /** Until when the list works offline (unix seconds). */
  expiresAt: number | null
  items: Entitlement[]
  /** Set when `refresh` was asked and the server couldn't be reached or refused. */
  error: PackError | null
}

export interface StoreItem {
  gameId: string
  name: string
  nameHe: string | null
  aboutHe: string | null
  cover: string
  hero: string
  priceId: string | null
  /** Minor units (agorot / cents), from the server. null: no price yet. */
  price: { amount: number; currency: string } | null
  priceUnset: boolean
  /** A buy button works: payments are set up and the translation has a price. */
  forSale: boolean
  access: Access
}

export interface Store {
  /** The launcher has an account server. false: show every item as "not for sale yet". */
  configured: boolean
  paymentsConfigured: boolean
  signedIn: boolean
  /** The server couldn't be asked (prices then unknown). */
  error: PackError | null
  items: StoreItem[]
}

/** Formats a store price for display, e.g. 2990 ILS -> "₪29.90". */
export function formatPrice(p: { amount: number; currency: string } | null): string | null {
  if (!p) return null
  try {
    return new Intl.NumberFormat('he-IL', { style: 'currency', currency: p.currency }).format(p.amount / 100)
  } catch {
    return `${(p.amount / 100).toFixed(2)} ${p.currency}`
  }
}

export const accountApi = {
  accountState: () => invoke<AccountState>('account_state'),
  /** Emails a sign-in link; answers with status 'pending'. `onAccount` says when it completes. */
  signIn: (email: string) => invoke<AccountState>('sign_in', { email }),
  cancelSignIn: () => invoke<AccountState>('cancel_sign_in'),
  signOut: () => invoke<AccountState>('sign_out'),
  /** `refresh`: ask the server first (do this after a checkout, or on a refresh button). */
  entitlements: (refresh = false) => invoke<Entitlements>('entitlements', { refresh }),
  storeList: () => invoke<Store>('store_list'),
  /** Returns the hosted checkout page; open it with `api.openUrl(url)`. Errors: PAYMENTS_NOT_CONFIGURED, NOT_FOR_SALE, ALREADY_OWNED, SIGN_IN_REQUIRED. */
  startCheckout: (gameId: string) => invoke<{ url: string }>('start_checkout', { gameId }),
  onAccount: (fn: (e: AccountEvent) => void) => listen<AccountEvent>('account', (e) => fn(e.payload)),
}
