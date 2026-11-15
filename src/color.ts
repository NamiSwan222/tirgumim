// The window takes its color from the game in front of it: the most vivid hue in the game's cover.
// Reading pixels needs the art server to allow it (an Access-Control-Allow-Origin header). Until it does, the canvas
// is "tainted", reading throws, and the window keeps its own colors; the blurred art behind everything still
// carries the game's colors either way.
import { useEffect, useState } from 'react'

export interface GameColor { h: number; s: number; l: number }

const cache = new Map<string, GameColor | null>()

function rgbToHsl(r: number, g: number, b: number): GameColor {
  r /= 255; g /= 255; b /= 255
  const max = Math.max(r, g, b), min = Math.min(r, g, b)
  const l = (max + min) / 2
  if (max === min) return { h: 0, s: 0, l }
  const d = max - min
  const s = l > 0.5 ? d / (2 - max - min) : d / (max + min)
  const h = max === r ? (g - b) / d + (g < b ? 6 : 0) : max === g ? (b - r) / d + 2 : (r - g) / d + 4
  return { h: h * 60, s, l }
}

function pick(img: HTMLImageElement): GameColor | null {
  const c = document.createElement('canvas')
  c.width = 64; c.height = 36
  const ctx = c.getContext('2d', { willReadFrequently: true })
  if (!ctx) return null
  ctx.drawImage(img, 0, 0, c.width, c.height)
  const data = ctx.getImageData(0, 0, c.width, c.height).data // throws when the image may not be read
  // Hue buckets weighted by how vivid each pixel is, so a small bright logo beats a large grey sky.
  const buckets = new Array(24).fill(0).map(() => ({ w: 0, h: 0, s: 0 }))
  for (let i = 0; i < data.length; i += 4) {
    const p = rgbToHsl(data[i], data[i + 1], data[i + 2])
    if (p.s < 0.45 || p.l < 0.14 || p.l > 0.85) continue
    const w = p.s * (1 - Math.abs(p.l - 0.5) * 1.6)
    const b = buckets[Math.floor(p.h / 15) % 24]
    b.w += w; b.h += p.h * w; b.s += p.s * w
  }
  const best = buckets.reduce((a, b) => (b.w > a.w ? b : a))
  if (best.w < 4) return null
  // Bright and saturated enough to glow on the dark window, and to carry dark text on a button.
  return { h: Math.round(best.h / best.w), s: Math.min(0.95, Math.max(0.7, best.s / best.w)), l: 0.64 }
}

export function useGameColor(src: string | null | undefined): GameColor | null {
  const [color, setColor] = useState<GameColor | null>(src ? cache.get(src) ?? null : null)
  useEffect(() => {
    if (!src) { setColor(null); return }
    if (cache.has(src)) { setColor(cache.get(src)!); return }
    let live = true
    const img = new Image()
    img.crossOrigin = 'anonymous'
    img.onload = () => {
      let c: GameColor | null = null
      try { c = pick(img) } catch { c = null }
      cache.set(src, c)
      if (live) setColor(c)
    }
    img.onerror = () => { cache.set(src, null); if (live) setColor(null) }
    img.src = src
    return () => { live = false }
  }, [src])
  return color
}

/** CSS variables for a game's color, or nothing (the stylesheet's own colors stay). */
export function colorVars(c: GameColor | null): React.CSSProperties {
  if (!c) return {}
  return { '--game-h': String(c.h), '--game-s': `${Math.round(c.s * 100)}%`, '--game-l': `${Math.round(c.l * 100)}%` } as React.CSSProperties
}
