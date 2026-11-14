// Small pieces the pages share: icons, game art with a fallback, cards, tiles, shelves.
import { useEffect, useRef, useState, type ReactNode } from 'react'
import type { Game, PcGame } from './api.ts'
import { STATUS, title } from './texts.ts'

/** Segoe Fluent Icons glyphs (Windows 11). */
export const ICON = {
  home: '',
  library: '',
  installed: '',
  update: '',
  progress: '',
  pc: '',
  settings: '',
  search: '',
  refresh: '',
  back: '', // points right: "back" in a right-to-left window
  min: '',
  max: '',
  close: '',
  folder: '',
  more: '',
  download: '',
  remove: '',
  warn: '',
  ok: '',
  info: '',
  link: '',
  next: '', // chevron pointing left: "next" in RTL
  prev: '',
  game: '',
}

export function Icon({ g, className }: { g: string; className?: string }) {
  return <span className={`icon ${className ?? ''}`} aria-hidden="true">{g}</span>
}

/** Game art; when the image can't be had, the game's name on a quiet panel. */
export function Art({ src, label, className }: { src: string; label: string; className?: string }) {
  const [failed, setFailed] = useState(false)
  return (
    <div className={`art ${className ?? ''} ${failed ? 'failed' : ''}`}>
      {!failed && <img src={src} alt="" loading="lazy" draggable={false} onError={() => setFailed(true)} />}
      {failed && <span className="art-label">{label}</span>}
    </div>
  )
}

export function StatusPill({ g }: { g: Game }) {
  const s = g.inProgress && (g.status === 'noPack' || g.status === 'notFound') ? 'noPack' : g.status
  return <span className={`pill ${s}`}>{g.running ? 'המשחק פתוח' : STATUS[s]}</span>
}

export function GameCard({ g, onOpen }: { g: Game; onOpen: () => void }) {
  return (
    <button className="card" onClick={onOpen} title={title(g)}>
      <Art src={g.cover} label={g.name} className="cover" />
      <div className="card-body">
        <div className="card-name">{title(g)}</div>
        {g.nameHe && <div className="card-en">{g.name}</div>}
        <StatusPill g={g} />
      </div>
    </button>
  )
}

export function PcTile({ o, onOpen }: { o: PcGame; onOpen: () => void }) {
  return (
    <button className="tile" onClick={onOpen} title={o.name}>
      <Art src={o.cover} label={o.name} className="cover" />
      <span className={`store ${o.store}`}>{o.store === 'steam' ? 'Steam' : 'Epic'}</span>
      <div className="tile-name">{o.name}</div>
    </button>
  )
}

/** A row that pages sideways, with arrows at its ends. */
export function Shelf({ heading, count, onAll, children }: { heading: string; count?: number; onAll?: () => void; children: ReactNode }) {
  const row = useRef<HTMLDivElement>(null)
  const [overflows, setOverflows] = useState(false)
  useEffect(() => {
    const el = row.current
    if (!el) return
    const check = () => setOverflows(el.scrollWidth > el.clientWidth + 4)
    check()
    const ro = new ResizeObserver(check)
    ro.observe(el)
    return () => ro.disconnect()
  }, [children])
  // In RTL scrollLeft runs from 0 to negative; scrollBy with a negative left moves toward the end.
  const page = (dir: 1 | -1) => row.current?.scrollBy({ left: -dir * row.current.clientWidth * 0.85, behavior: 'smooth' })
  return (
    <section className="shelf">
      <header>
        <h2>{heading}</h2>
        {count != null && <span className="count">{count}</span>}
        <span className="grow" />
        {onAll && <button className="link" onClick={onAll}>הצג הכל</button>}
        {overflows && <button className="round" onClick={() => page(-1)} aria-label="הקודם"><Icon g={ICON.prev} /></button>}
        {overflows && <button className="round" onClick={() => page(1)} aria-label="הבא"><Icon g={ICON.next} /></button>}
      </header>
      <div className="shelf-row" ref={row}>{children}</div>
    </section>
  )
}
