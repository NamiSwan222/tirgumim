// The launcher updating itself: checks our GitHub releases (latest.json on the "updater" release, signed with our
// key), downloads the new installer, and restarts. Not used in the dev catalog mode.
import { useCallback, useEffect, useRef, useState } from 'react'
import { check, type Update } from '@tauri-apps/plugin-updater'
import { relaunch } from '@tauri-apps/plugin-process'

// Hourly, so a new version shows up soon after it is released.
const EVERY = 60 * 60 * 1000

export interface SelfUpdate {
  /** The newer version, if there is one. */
  available: { version: string; notes: string | null } | null
  /** 0..1 while downloading, null when the size is unknown; undefined when not installing. */
  progress: number | null | undefined
  install: () => Promise<void>
}

export function useSelfUpdate(enabled: boolean, onError: (text: string) => void): SelfUpdate {
  const [update, setUpdate] = useState<Update | null>(null)
  const [progress, setProgress] = useState<number | null | undefined>(undefined)
  const errorRef = useRef(onError)
  errorRef.current = onError

  useEffect(() => {
    if (!enabled) return
    let stop = false
    const look = async () => {
      try {
        const u = await check()
        if (!stop) setUpdate(u)
      } catch {
        // No network, GitHub down, or no release yet: try again later, quietly.
      }
    }
    look()
    const t = setInterval(look, EVERY)
    return () => { stop = true; clearInterval(t) }
  }, [enabled])

  const install = useCallback(async () => {
    if (!update || progress !== undefined) return
    let total = 0
    let got = 0
    setProgress(null)
    try {
      await update.downloadAndInstall((e) => {
        if (e.event === 'Started') total = e.data.contentLength ?? 0
        else if (e.event === 'Progress') { got += e.data.chunkLength; setProgress(total ? got / total : null) }
      })
      await relaunch()
    } catch (e) {
      setProgress(undefined)
      errorRef.current(`העדכון נכשל: ${String(e)}`)
    }
  }, [update, progress])

  return {
    available: update ? { version: update.version, notes: update.body?.trim() || null } : null,
    progress,
    install,
  }
}
