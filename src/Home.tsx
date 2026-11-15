// The home screen is a stage: the chosen game fills the window, with its big button, and the translations wait in
// a rail along the bottom. Clicking a cover brings that game forward; clicking it again opens its page.
import { useEffect, useRef, useState } from 'react'
import type { Game, Progress } from './api.ts'
import { mainAction, type Act } from './GamePage.tsx'
import { Art, Icon, ICON, StatusPill } from './ui.tsx'
import { SOURCE, STEP, ltr, size, title } from './texts.ts'

function StageArt({ g }: { g: Game }) {
  const [failed, setFailed] = useState(false)
  useEffect(() => setFailed(false), [g.id])
  return (
    <div className="stage-art" key={g.id}>
      {!failed && <img src={g.hero} alt="" draggable={false} onError={() => setFailed(true)} />}
      {failed && <img className="as-cover" src={g.cover} alt="" draggable={false} onError={(e) => (e.currentTarget.style.display = 'none')} />}
    </div>
  )
}

export function Home({ games, othersCount, selected, onSelect, busy, progress, onOpen, onAct, onPick, onShowPc }: {
  games: Game[]
  othersCount: number
  selected: Game | null
  onSelect: (g: Game) => void
  busy: boolean
  progress: Progress | null
  onOpen: (g: Game) => void
  onAct: (kind: Act, g: Game) => void
  onPick: (g: Game) => void
  onShowPc: () => void
}) {
  const rail = useRef<HTMLDivElement>(null)

  // Arrow keys walk the rail (right is "previous" in a right-to-left window), Enter opens the game.
  useEffect(() => {
    const key = (e: KeyboardEvent) => {
      if (!selected || (e.target as HTMLElement)?.tagName === 'INPUT' || document.querySelector('.scrim')) return
      const i = games.findIndex((x) => x.id === selected.id)
      if (e.key === 'ArrowLeft' && i < games.length - 1) onSelect(games[i + 1])
      else if (e.key === 'ArrowRight' && i > 0) onSelect(games[i - 1])
      else if (e.key === 'Enter' && document.activeElement === document.body) onOpen(selected)
    }
    window.addEventListener('keydown', key)
    return () => window.removeEventListener('keydown', key)
  }, [games, selected, onSelect, onOpen])

  if (!selected) {
    return (
      <div className="page stage empty-stage">
        <div className="empty"><Icon g={ICON.game} /><p>אין עדיין תרגומים ברשימה. לחצו Ctrl+F5 כדי לבדוק שוב.</p></div>
      </div>
    )
  }
  const g = selected
  const main = mainAction(g, busy, (k) => onAct(k, g), () => onPick(g))
  const mine = progress && progress.gameId === g.id ? progress : null
  const facts = [
    g.available?.version && `גרסה ${ltr(g.available.version)}`,
    size(g.available?.size),
    g.location ? `נמצא ב-${SOURCE[g.location.source] ?? g.location.source}` : null,
  ].filter(Boolean) as string[]

  return (
    <div className="page stage">
      <StageArt g={g} />
      <div className="stage-shade" />

      <section className="stage-copy" key={g.id}>
        {g.nameHe && <div className="en stage-en">{g.name}</div>}
        <h1 className="stage-title">{title(g)}</h1>
        <div className="stage-meta">
          <StatusPill g={g} />
          {facts.map((f) => <span key={f} className="fact-chip">{f}</span>)}
        </div>
        {g.aboutHe && <p className="stage-about">{g.aboutHe}</p>}
        <div className="actions">
          {main && (
            <button className="btn primary big glow" onClick={main.onClick} disabled={main.disabled}>
              <Icon g={main.icon} /> {main.label}
            </button>
          )}
          {g.status === 'installed' && !g.launchOnly && <span className="done-note"><Icon g={ICON.ok} /> התרגום מותקן. משחק מהנה!</span>}
          <button className="btn glass big" onClick={() => onOpen(g)}>לעמוד המשחק</button>
        </div>
        {mine && (
          <div className="progress-wrap">
            <div className={`progress ${mine.fraction == null ? 'indeterminate' : ''}`}>
              <span style={mine.fraction != null ? { width: `${Math.round(mine.fraction * 100)}%` } : undefined} />
            </div>
            <div className="progress-label">{STEP[mine.step] ?? ''}{mine.fraction != null && mine.step === 'download' ? ` ${ltr(`${Math.round(mine.fraction * 100)}%`)}` : ''}</div>
          </div>
        )}
      </section>

      <section className="rail-wrap">
        <header>
          <h2>התרגומים שלנו</h2>
          <span className="count">{games.length}</span>
          <span className="grow" />
          {othersCount > 0 && (
            <button className="link" onClick={onShowPc}>עוד {othersCount} משחקים במחשב שלך <Icon g={ICON.next} /></button>
          )}
        </header>
        <div className="rail" ref={rail} role="listbox" aria-label="התרגומים שלנו">
          {games.map((x) => (
            <button key={x.id} role="option" aria-selected={x.id === g.id} className={`rail-item ${x.id === g.id ? 'on' : ''}`}
              onClick={() => (x.id === g.id ? onOpen(x) : onSelect(x))} onDoubleClick={() => onOpen(x)} title={title(x)}>
              <Art src={x.cover} label={x.name} className="cover" />
              <span className="rail-name">{title(x)}</span>
            </button>
          ))}
        </div>
      </section>
    </div>
  )
}
