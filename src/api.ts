// What the Rust side (src-tauri/src/lib.rs) answers, and the calls the window makes.
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'

export type Status = 'notFound' | 'noPack' | 'unavailable' | 'ready' | 'installed' | 'updateAvailable' | 'needsRepair'

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
  step: 'download' | 'verify' | 'backup' | 'copy' | 'done'
  fraction: number | null
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
