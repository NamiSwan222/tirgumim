// A game's page: wide art that fades into a blurred copy of itself, the facts in four boxes, one main button with
// the rest under "More", and what the game still needs before the translation can go in.
import { useEffect, useRef, useState } from 'react'
import type { Game, PcGame, Progress } from './api.ts'
import { Art, Icon, ICON, StatusPill } from './ui.tsx'
import { SOURCE, STEP, date, ltr, size, title } from './texts.ts'

export type Act = 'install' | 'update' | 'uninstall' | 'play'

function Hero({ hero, cover, label, children }: { hero: string; cover: string; label: string; children: React.ReactNode }) {
  const [heroFailed, setHeroFailed] = useState(false)
  return (
    <div className={`hero ${heroFailed ? 'no-hero' : ''}`}>
      <div className="hero-bg">
        {!heroFailed && <img src={hero} alt="" draggable={false} onError={() => setHeroFailed(true)} />}
        {heroFailed && <img src={cover} alt="" draggable={false} className="blurred" onError={(e) => (e.currentTarget.style.display = 'none')} />}
      </div>
      <div className="hero-fade" />
      <div className="hero-content">
        <Art src={cover} label={label} className="hero-cover" />
        <div className="hero-text">{children}</div>
      </div>
    </div>
  )
}

function Stat({ label, value, sub }: { label: string; value: string; sub?: string }) {
  return (
    <div className="fact">
      <dt>{label}</dt>
      <dd>{value || '—'}{sub && <span className="fact-sub">{sub}</span>}</dd>
    </div>
  )
}

/** A line with any Hebrew in it reads right to left; one without (an English section) left to right. */
const dirOf = (s: string) => (/[֐-׿]/.test(s) ? 'rtl' : 'ltr')

/** Release notes are written in Markdown on GitHub. Show their headings, lists and paragraphs, without the marks. */
function Changelog({ text }: { text: string }) {
  const clean = (s: string) => s.replace(/\*\*|__|`/g, '').replace(/\[([^\]]+)\]\([^)]*\)/g, '$1').trim()
  const out: React.ReactNode[] = []
  let list: string[] = []
  let para: string[] = []
  const flush = () => {
    if (para.length) out.push(<p key={out.length} dir={dirOf(para.join(' '))}>{para.join(' ')}</p>)
    if (list.length) out.push(<ul key={out.length} dir={dirOf(list.join(' '))}>{list.map((l, i) => <li key={i} dir={dirOf(l)}>{l}</li>)}</ul>)
    para = []; list = []
  }
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim()
    if (/^tirgumim-min-launcher:/i.test(line)) continue
    if (!line) { flush(); continue }
    if (/^([-*_])\1{2,}$/.test(line)) { flush(); out.push(<hr key={out.length} />); continue }
    const h = line.match(/^#{1,6}\s+(.*)$/) ?? line.match(/^\*\*([^*]+)\*\*:?$/)
    const li = line.match(/^[-*+]\s+(.*)$/)
    if (h) { flush(); out.push(<h4 key={out.length} dir={dirOf(h[1])}>{clean(h[1])}</h4>) }
    else if (li) { if (para.length) { const p = para; para = []; out.push(<p key={out.length} dir={dirOf(p.join(' '))}>{p.join(' ')}</p>) } list.push(clean(li[1])) }
    else { if (list.length) flush(); para.push(clean(line)) }
  }
  flush()
  return <div className="changelog">{out}</div>
}

function More({ items, disabled }: { items: { label: string; icon: string; danger?: boolean; onClick: () => void }[]; disabled: boolean }) {
  const [open, setOpen] = useState(false)
  const ref = useRef<HTMLDivElement>(null)
  useEffect(() => {
    if (!open) return
    const close = (e: MouseEvent) => { if (!ref.current?.contains(e.target as Node)) setOpen(false) }
    window.addEventListener('mousedown', close)
    return () => window.removeEventListener('mousedown', close)
  }, [open])
  if (!items.length) return null
  return (
    <div className="more" ref={ref}>
      <button className="btn ghost" onClick={() => setOpen(!open)} disabled={disabled} aria-expanded={open}>
        <Icon g={ICON.more} /> עוד
      </button>
      {open && (
        <div className="menu" role="menu">
          {items.map((it) => (
            <button key={it.label} role="menuitem" className={it.danger ? 'danger' : ''} onClick={() => { setOpen(false); it.onClick() }}>
              <Icon g={it.icon} /> {it.label}
            </button>
          ))}
        </div>
      )}
    </div>
  )
}

export function GamePage({ g, busy, progress, onBack, onAct, onPick, onOpenFolder, onOpenUrl }: {
  g: Game
  busy: boolean
  progress: Progress | null
  onBack: () => void
  onAct: (kind: Act) => void
  onPick: () => void
  onOpenFolder: () => void
  onOpenUrl: (url: string) => void
}) {
  const mine = progress && progress.gameId === g.id ? progress : null
  const blocked = busy || !!g.running || g.needs.length > 0

  let main: { label: string; icon: string; onClick: () => void; disabled?: boolean } | null = null
  if (g.status === 'ready') main = { label: 'התקנת התרגום', icon: ICON.download, onClick: () => onAct('install'), disabled: blocked }
  else if (g.status === 'updateAvailable') main = { label: `עדכון לגרסה ${g.available?.version ?? ''}`, icon: ICON.update, onClick: () => onAct('update'), disabled: blocked }
  else if (g.status === 'needsRepair') main = { label: 'התקנה מחדש', icon: ICON.update, onClick: () => onAct('update'), disabled: blocked || !g.available }
  else if (g.status === 'notFound') main = { label: 'בחירת תיקיית המשחק', icon: ICON.folder, onClick: onPick, disabled: busy }
  else if (g.status === 'installed' && g.launchOnly) main = { label: 'שחק בעברית', icon: ICON.play, onClick: () => onAct('play'), disabled: busy || !!g.running }
  else if (g.status === 'playing') main = { label: 'המשחק פועל בעברית', icon: ICON.play, onClick: () => {}, disabled: true }

  const more = [
    ...(g.location ? [{ label: 'פתיחת תיקיית המשחק', icon: ICON.folder, onClick: onOpenFolder }] : []),
    ...(!g.installed && g.status !== 'notFound' ? [{ label: g.location ? 'שינוי תיקיית המשחק' : 'בחירת תיקיית המשחק', icon: ICON.folder, onClick: onPick }] : []),
    ...(g.installed && g.status !== 'playing' ? [{ label: 'הסרת התרגום', icon: ICON.remove, danger: true, onClick: () => onAct('uninstall') }] : []),
  ]

  // What still stands between the player and the translation.
  const todo: { text: string; url?: string | null; kind: 'warn' | 'info' }[] = []
  if (g.status === 'notFound') todo.push({ kind: 'warn', text: 'לא מצאנו את המשחק ב-Steam או ב-Epic. אם הוא מותקן במקום אחר, בחרו את התיקייה שלו.' })
  if (g.running && g.status !== 'playing') todo.push({ kind: 'warn', text: g.launchOnly && g.installed
    ? `המשחק פתוח (${ltr(g.running)}) בלי התרגום. כדי לשחק בעברית, סגרו אותו ולחצו "שחק בעברית".`
    : `המשחק פתוח (${ltr(g.running)}). סגרו אותו כדי להתקין או להסיר את התרגום.` })
  if (g.status === 'playing' && g.launchOnly === 'swap') todo.push({ kind: 'info', text: 'קבצי התרגום נמצאים עכשיו במשחק. כשתסגרו את המשחק, המשגר יחזיר את הקבצים המקוריים. השאירו את המשגר פתוח עד אז.' })
  for (const n of g.needs) todo.push({ kind: 'warn', text: n.message, url: n.url })
  if (g.status === 'needsRepair') todo.push({ kind: 'warn', text: 'חלק מקבצי התרגום הוחלפו, כנראה אחרי עדכון של המשחק. התקינו מחדש כדי לתקן.' })
  if (g.status === 'needsLauncher') todo.push({ kind: 'warn', text: `התרגום למשחק הזה דורש גרסה חדשה יותר של המשגר (${ltr(g.minLauncherVersion ?? '')}). כשיופיע "גרסה חדשה" בשורת המצב, לחצו עליו, ואז תוכלו להתקין.` })
  if (g.status === 'unavailable') todo.push({ kind: 'info', text: 'לא הצלחנו לקבל את פרטי התרגום מ-GitHub. נסו לרענן בעוד כמה דקות.' })
  if (g.status === 'noPack' || (g.inProgress && !g.available)) todo.push({ kind: 'info', text: 'אנחנו עדיין עובדים על התרגום למשחק הזה. כשהוא יהיה מוכן הוא יופיע כאן.' })
  if (g.installed?.forced) todo.push({ kind: 'info', text: 'התרגום הותקן על גרסת משחק שלא נבדקה. אם משהו לא נראה טוב, אפשר להסיר אותו.' })

  return (
    <div className="page game-page">
      <Hero hero={g.hero} cover={g.cover} label={g.name}>
        <button className="back" onClick={onBack}><Icon g={ICON.back} /> חזרה</button>
        {g.nameHe && <div className="en">{g.name}</div>}
        <h1>{title(g)}</h1>
        <div className="hero-meta">
          <StatusPill g={g} />
          {g.engine && g.engine !== 'custom' && <span className="chip">{g.engine === 'unreal' ? 'Unreal Engine' : g.engine === 'unity' ? 'Unity' : g.engine}</span>}
        </div>
        {g.aboutHe && <p className="about subtitle">{g.aboutHe}</p>}
        <div className="actions">
          {main && (
            <button className="btn primary big" onClick={main.onClick} disabled={main.disabled}>
              <Icon g={main.icon} /> {main.label}
            </button>
          )}
          {g.status === 'installed' && !g.launchOnly && <span className="done-note"><Icon g={ICON.ok} /> התרגום מותקן. משחק מהנה!</span>}
          {g.status === 'installed' && g.launchOnly && <span className="done-note"><Icon g={ICON.ok} /> התרגום מוכן. הוא פועל רק כשמפעילים את המשחק מכאן. מ-Steam המשחק נפתח כרגיל.</span>}
          <More items={more} disabled={busy} />
        </div>
        {mine && (
          <div className="progress-wrap">
            <div className={`progress ${mine.fraction == null ? 'indeterminate' : ''}`}>
              <span style={mine.fraction != null ? { width: `${Math.round(mine.fraction * 100)}%` } : undefined} />
            </div>
            <div className="progress-label">{STEP[mine.step] ?? ''}{mine.fraction != null && mine.step === 'download' ? ` ${ltr(`${Math.round(mine.fraction * 100)}%`)}` : ''}</div>
          </div>
        )}
      </Hero>

      <div className="page-body">
        <dl className="facts">
          <Stat label="גרסת התרגום" value={g.installed?.version ?? g.available?.version ?? ''} sub={g.installed ? `הותקנה ב-${date(g.installed.installedAt)}` : g.available?.publishedAt ? `פורסמה ב-${date(g.available.publishedAt)}` : undefined} />
          <Stat label="גודל ההורדה" value={size(g.available?.size)} sub={g.available ? (g.available.source === 'github' ? 'מ-GitHub' : undefined) : undefined} />
          <Stat label="המשחק נמצא ב-" value={g.location ? SOURCE[g.location.source] ?? g.location.source : 'לא נמצא'} />
          <Stat label="גרסת המשחק" value={g.installed?.gameVersion ?? (g.installed?.forced ? 'לא מוכרת' : '')} sub={g.installed ? `${g.installed.files} קבצים הותקנו` : undefined} />
        </dl>

        {todo.length > 0 && (
          <section className="todo">
            <h3>{g.status === 'ready' || g.status === 'updateAvailable' || g.status === 'needsRepair' ? 'לפני ההתקנה' : 'מצב'}</h3>
            <ul>
              {todo.map((t, i) => (
                <li key={i} className={t.kind}>
                  <Icon g={t.kind === 'warn' ? ICON.warn : ICON.info} />
                  <span>{t.text}</span>
                  {t.url && <button className="link" onClick={() => onOpenUrl(t.url!)}>לפרטים <Icon g={ICON.link} /></button>}
                </li>
              ))}
            </ul>
          </section>
        )}

        {g.available?.changelog && (
          <section className="notes">
            <h3>מה חדש בגרסה {g.available.version}</h3>
            <Changelog text={g.available.changelog} />
          </section>
        )}

        {g.location && (
          <section className="where">
            <h3>תיקיית המשחק</h3>
            <div className="path" dir="ltr">{g.location.dir}</div>
          </section>
        )}
      </div>
    </div>
  )
}

export function PcGamePage({ o, onBack }: { o: PcGame; onBack: () => void }) {
  return (
    <div className="page game-page">
      <Hero hero={o.hero} cover={o.cover} label={o.name}>
        <button className="back" onClick={onBack}><Icon g={ICON.back} /> חזרה</button>
        <h1 dir="auto">{o.name}</h1>
        <div className="hero-meta"><span className={`chip store-chip ${o.store}`}>{o.store === 'steam' ? 'Steam' : 'Epic Games'}</span></div>
        <p className="about">עדיין אין לנו תרגום למשחק הזה. כשיהיה, הוא יופיע ב"התרגומים שלנו".</p>
      </Hero>
      <div className="page-body">
        <section className="where">
          <h3>תיקיית המשחק</h3>
          <div className="path" dir="ltr">{o.dir}</div>
        </section>
      </div>
    </div>
  )
}
