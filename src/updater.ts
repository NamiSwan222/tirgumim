// The launcher updating itself from our GitHub releases (latest.json on the "updater" release, signed with our key).
// - Checks at start and every hour, and right away from "בדוק עדכונים".
// - Automatic mode (default on): a new version downloads in the background; the banner then only says "restart".
// - Manual mode: the banner offers "עדכן עכשיו", which downloads, checks the signature, installs and restarts.
// Not used in the dev catalog mode.
import { useCallback, useEffect, useRef, useState } from 'react'
import { check, type Update } from '@tauri-apps/plugin-updater'
import { relaunch } from '@tauri-apps/plugin-process'

const EVERY = 60 * 60 * 1000
const AUTO_KEY = 'tirgumim.autoUpdate'

export type UpdateState = 'idle' | 'checking' | 'upToDate' | 'available' | 'downloading' | 'ready' | 'installing' | 'error'

export interface SelfUpdate {
  state: UpdateState
  version: string | null
  notes: string | null
  /** 0..1 while downloading, null when the size is unknown. */
  progress: number | null
  error: string | null
  auto: boolean
  setAuto: (on: boolean) => void
  /** Ask GitHub now. `manual`: the player pressed the button (so "you're up to date" is shown). */
  checkNow: (manual?: boolean) => Promise<void>
  /** Download if needed, install, restart. */
  updateNow: () => Promise<void>
}

function readAuto(): boolean {
  try {
    return localStorage.getItem(AUTO_KEY) !== 'off'
  } catch {
    return true
  }
}

export function useSelfUpdate(enabled: boolean): SelfUpdate {
  const [state, setState] = useState<UpdateState>('idle')
  const [update, setUpdate] = useState<Update | null>(null)
  const [progress, setProgress] = useState<number | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [auto, setAutoState] = useState(readAuto)
  const busy = useRef(false)
  const downloaded = useRef(false)
  const autoRef = useRef(auto)
  autoRef.current = auto

  const download = useCallback(async (u: Update) => {
    let total = 0
    let got = 0
    setProgress(null)
    setState('downloading')
    await u.download((e) => {
      if (e.event === 'Started') total = e.data.contentLength ?? 0
      else if (e.event === 'Progress') { got += e.data.chunkLength; setProgress(total ? got / total : null) }
    })
    downloaded.current = true
    setState('ready')
  }, [])

  const checkNow = useCallback(async (manual = false) => {
    if (!enabled || busy.current) return
    busy.current = true
    if (manual) { setState('checking'); setError(null) }
    try {
      const u = await check()
      if (!u) {
        setUpdate(null)
        setState(manual ? 'upToDate' : 'idle')
        return
      }
      // Same version already found (and maybe downloaded): keep it.
      if (update && update.version === u.version && (state === 'ready' || state === 'downloading')) return
      setUpdate(u)
      downloaded.current = false
      setState('available')
      if (autoRef.current) await download(u)
    } catch (e) {
      // Quietly retried on the next round unless the player asked.
      if (manual) { setError(String(e)); setState('error') }
      else setState((s) => (s === 'checking' ? 'idle' : s))
    } finally {
      busy.current = false
    }
  }, [enabled, update, state, download])

  useEffect(() => {
    if (!enabled) return
    checkNow(false)
    const t = setInterval(() => checkNow(false), EVERY)
    return () => clearInterval(t)
    // Only on start; checkNow changes with state.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [enabled])

  const updateNow = useCallback(async () => {
    if (!update) return
    setError(null)
    try {
      if (!downloaded.current) await download(update)
      setState('installing')
      await update.install()
      await relaunch()
    } catch (e) {
      setError(String(e))
      setState('error')
    }
  }, [update, download])

  const setAuto = useCallback((on: boolean) => {
    setAutoState(on)
    try { localStorage.setItem(AUTO_KEY, on ? 'on' : 'off') } catch { /* kept for this run only */ }
    if (on && update && !downloaded.current && state === 'available') download(update).catch((e) => { setError(String(e)); setState('error') })
  }, [update, state, download])

  return {
    state,
    version: update?.version ?? null,
    notes: update?.body?.trim() || null,
    progress,
    error,
    auto,
    setAuto,
    checkNow,
    updateNow,
  }
}
