// The window: its own title bar, the sidebar, the page (home, a list, or a game), messages floating above the
// status bar, and the dialogs.
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { api, asError, type Game, type Library, type PcGame, type Progress } from './api.ts'
import { GamePage, PcGamePage, type Act } from './GamePage.tsx'
import { GameCard, Icon, ICON, PcTile, Shelf, StatusPill } from './ui.tsx'
import { CATALOG_SOURCE, errorText, title } from './texts.ts'
import { useSelfUpdate } from './updater.ts'

type Cat = 'home' | 'ours' | 'installed' | 'updates' | 'soon' | 'pc'
type Page = { kind: 'cat'; cat: Cat } | { kind: 'game'; id: string } | { kind: 'pc'; store: string; id: string }

const CATS: { id: Cat; label: string; icon: string }[] = [
  { id: 'home', label: 'בית', icon: ICON.home },
  { id: 'ours', label: 'התרגומים שלנו', icon: ICON.library },
  { id: 'installed', label: 'מותקנים אצלי', icon: ICON.installed },
  { id: 'updates', label: 'עדכונים', icon: ICON.update },
  { id: 'soon', label: 'בקרוב', icon: ICON.progress },
  { id: 'pc', label: 'משחקים במחשב', icon: ICON.pc },
]

interface Toast { id: number; text: string; kind: 'ok' | 'error' | 'info' }

const isInstalled = (g: Game) => !!g.installed
const hasUpdate = (g: Game) => g.status === 'updateAvailable' || g.status === 'needsRepair'
const isSoon = (g: Game) => g.inProgress && !g.available

function matches(q: string, ...names: (string | null | undefined)[]) {
  const s = q.trim().toLowerCase()
  return !s || names.some((n) => n && n.toLowerCase().includes(s))
}

function TitleBar({ query, setQuery, searchRef, onRefresh, refreshing }: {
  query: string; setQuery: (q: string) => void; searchRef: React.RefObject<HTMLInputElement | null>; onRefresh: () => void; refreshing: boolean
}) {
  const win = getCurrentWindow()
  return (
    <header className="titlebar" data-tauri-drag-region>
      <div className="brand" data-tauri-drag-region>
        <img src="/icon.png" alt="" draggable={false} />
        <span data-tauri-drag-region>תרגומים</span>
      </div>
      <label className="search">
        <Icon g={ICON.search} />
        <input ref={searchRef} value={query} onChange={(e) => setQuery(e.target.value)} placeholder="חיפוש משחק" spellCheck={false} />
        <kbd dir="ltr">Ctrl K</kbd>
      </label>
      <button className={`tb-btn ${refreshing ? 'spin' : ''}`} onClick={onRefresh} title="רענון: סריקת המחשב ובדיקת תרגומים חדשים" disabled={refreshing}>
        <Icon g={ICON.refresh} />
      </button>
      <div className="grow" data-tauri-drag-region />
      <div className="win-controls">
        <button onClick={() => win.minimize()} aria-label="מזעור"><Icon g={ICON.min} /></button>
        <button onClick={() => win.toggleMaximize()} aria-label="הגדלה"><Icon g={ICON.max} /></button>
        <button className="close" onClick={() => win.close()} aria-label="סגירה"><Icon g={ICON.close} /></button>
      </div>
    </header>
  )
}

/** The wide art, or, for a game without one, its cover blurred behind a sharp copy. */
function BannerArt({ hero, cover }: { hero: string; cover: string }) {
  const [heroFailed, setHeroFailed] = useState(false)
  if (!heroFailed) return <div className="banner-art"><img src={hero} alt="" draggable={false} onError={() => setHeroFailed(true)} /></div>
  return (
    <div className="banner-art no-hero">
      <img className="blurred" src={cover} alt="" draggable={false} onError={(e) => (e.currentTarget.style.display = 'none')} />
      <img className="sharp" src={cover} alt="" draggable={false} onError={(e) => (e.currentTarget.style.display = 'none')} />
    </div>
  )
}

function Banner({ games, onOpen }: { games: Game[]; onOpen: (g: Game) => void }) {
  const [i, setI] = useState(0)
  useEffect(() => {
    if (games.length < 2) return
    const t = setInterval(() => setI((x) => (x + 1) % games.length), 8000)
    return () => clearInterval(t)
  }, [games.length])
  const g = games[i % Math.max(1, games.length)]
  if (!g) return null
  return (
    <div className="banner" onClick={() => onOpen(g)} role="button" tabIndex={0}>
      <BannerArt key={g.id} hero={g.hero} cover={g.cover} />
      <div className="banner-fade" />
      <div className="banner-text">
        <div className="eyebrow">{g.available ? 'תרגום לעברית' : 'תרגום בדרך'}</div>
        <h1>{title(g)}</h1>
        {g.nameHe && <div className="en">{g.name}</div>}
        {g.aboutHe && <p>{g.aboutHe}</p>}
        <StatusPill g={g} />
      </div>
      {games.length > 1 && (
        <div className="dots" onClick={(e) => e.stopPropagation()}>
          {games.map((x, n) => <button key={x.id} className={n === i % games.length ? 'on' : ''} onClick={() => setI(n)} aria-label={title(x)} />)}
        </div>
      )}
    </div>
  )
}

function Empty({ text }: { text: string }) {
  return <div className="empty"><Icon g={ICON.game} /><p>{text}</p></div>
}

export default function App() {
  const [lib, setLib] = useState<Library | null>(null)
  const [info, setInfo] = useState<{ version: string; dev: boolean } | null>(null)
  const [page, setPage] = useState<Page>({ kind: 'cat', cat: 'home' })
  const [, setHistory] = useState<Page[]>([])
  const [query, setQuery] = useState('')
  const [busy, setBusy] = useState(false)
  const [refreshing, setRefreshing] = useState(false)
  const [progress, setProgress] = useState<Progress | null>(null)
  const [toasts, setToasts] = useState<Toast[]>([])
  const [confirm, setConfirm] = useState<{ title: string; body: string; ok: string; danger?: boolean; resolve: (v: boolean) => void } | null>(null)
  const [settings, setSettings] = useState<{ url: string; placeholder: string } | null>(null)
  const searchRef = useRef<HTMLInputElement>(null)
  const mainRef = useRef<HTMLElement>(null)

  const toast = useCallback((text: string, kind: Toast['kind'] = 'info') => {
    const id = Date.now() + Math.random()
    setToasts((t) => [...t.slice(-2), { id, text, kind }])
    setTimeout(() => setToasts((t) => t.filter((x) => x.id !== id)), kind === 'error' ? 9000 : 5000)
  }, [])

  const self = useSelfUpdate(!!info && !info.dev, (text) => toast(text, 'error'))

  async function updateLauncher() {
    if (!self.available) return
    if (busy) { toast('אפשר לעדכן את המשגר אחרי שהפעולה הנוכחית תסתיים.', 'info'); return }
    const notes = self.available.notes ? `

מה חדש:
${self.available.notes}` : ''
    const ok = await ask('גרסה חדשה של המשגר', `גרסה ${self.available.version} זמינה. המשגר יוריד אותה, יתקין אותה ויופעל מחדש. התרגומים שהתקנתם נשארים כמו שהם.${notes}`, 'עדכון עכשיו')
    if (ok) await self.install()
  }

  const load = useCallback(async (reload: boolean) => {
    try {
      setLib(await api.list(reload))
    } catch (e) {
      toast(errorText(asError(e)), 'error')
    }
  }, [toast])

  useEffect(() => {
    api.info().then(setInfo)
    load(false)
    const un = api.onProgress((p) => setProgress(p))
    // While the window is open, notice a game being opened or closed, and new packs (cached answers, so cheap).
    const t = setInterval(() => load(false), 15000)
    return () => { un.then((f) => f()); clearInterval(t) }
  }, [load])

  const go = (p: Page) => {
    setHistory((h) => [...h.slice(-20), page])
    setPage(p)
    mainRef.current?.scrollTo(0, 0)
  }
  const back = useCallback(() => {
    setHistory((h) => {
      const prev = h[h.length - 1]
      setPage(prev ?? { kind: 'cat', cat: 'home' })
      return h.slice(0, -1)
    })
  }, [])

  useEffect(() => {
    const key = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'k') { e.preventDefault(); searchRef.current?.focus(); searchRef.current?.select() }
      else if (e.key === 'Escape' && !confirm && !settings) { if (query) setQuery(''); else if (page.kind !== 'cat') back() }
      else if (e.key === 'F5') { e.preventDefault(); refresh() }
    }
    window.addEventListener('keydown', key)
    return () => window.removeEventListener('keydown', key)
  })

  const ask = (title: string, body: string, ok: string, danger = false) =>
    new Promise<boolean>((resolve) => setConfirm({ title, body, ok, danger, resolve }))

  async function refresh() {
    setRefreshing(true)
    await load(true)
    setRefreshing(false)
    toast('הרשימה עודכנה', 'ok')
  }

  async function act(kind: Act, g: Game, force = false): Promise<void> {
    if (kind === 'uninstall' && !(await ask('הסרת התרגום', `להסיר את התרגום מ"${title(g)}" ולהחזיר את הקבצים המקוריים של המשחק?`, 'הסרה', true))) return
    setBusy(true)
    setProgress({ gameId: g.id, step: kind === 'uninstall' ? 'copy' : 'download', fraction: null })
    try {
      if (kind === 'uninstall') {
        const r = await api.uninstall(g.id)
        const kept = r.skippedChanged.length
        toast(kept ? `התרגום הוסר. ${kept} קבצים עודכנו בינתיים על ידי המשחק ונשארו כפי שהם.` : 'התרגום הוסר והקבצים המקוריים שוחזרו.', 'ok')
      } else {
        await (kind === 'install' ? api.install(g.id, force) : api.update(g.id, force))
        toast(kind === 'install' ? `התרגום ל"${title(g)}" הותקן. משחק מהנה!` : 'התרגום עודכן.', 'ok')
      }
    } catch (e) {
      const err = asError(e)
      if (err.code === 'GAME_VERSION' && err.allowUnknown && !force) {
        setBusy(false); setProgress(null)
        const ok = await ask('גרסת משחק לא מוכרת', 'התרגום הוכן לגרסה אחרת של המשחק. בדרך כלל זה עובד, אבל ייתכן שחלק מהטקסטים יופיעו באנגלית. אפשר תמיד להסיר את התרגום ולשחזר את הקבצים המקוריים. להתקין בכל זאת?', 'התקנה בכל זאת')
        if (ok) return act(kind, g, true)
      } else {
        toast(errorText(err), 'error')
      }
    } finally {
      setBusy(false)
      setProgress(null)
      await load(false)
    }
  }

  async function pick(g: Game) {
    try {
      if (await api.pickFolder(g.id)) toast('התיקייה נשמרה.', 'ok')
    } catch (e) {
      toast(errorText(asError(e)), 'error')
    }
    await load(false)
  }

  async function openSettings() {
    const s = await api.getSettings()
    setSettings({ url: s.catalogUrl, placeholder: s.defaultCatalogUrl })
  }
  async function saveSettings() {
    if (!settings) return
    try {
      await api.setSettings(settings.url)
      setSettings(null)
      await load(true)
    } catch (e) {
      toast(errorText(asError(e)), 'error')
    }
  }

  const games = lib?.translations ?? []
  const others = lib?.others ?? []
  const counts: Record<Cat, number | null> = {
    home: null,
    ours: games.length,
    installed: games.filter(isInstalled).length,
    updates: games.filter(hasUpdate).length,
    soon: games.filter(isSoon).length,
    pc: others.length,
  }
  const featured = useMemo(() => {
    const order = (g: Game) => (g.status === 'ready' || g.status === 'updateAvailable' ? 0 : g.installed ? 1 : g.location ? 2 : 3)
    return [...games].sort((a, b) => order(a) - order(b)).slice(0, 5)
  }, [games])

  const gFiltered = games.filter((g) => matches(query, g.name, g.nameHe))
  const oFiltered = others.filter((o) => matches(query, o.name))
  const openGame = (g: Game) => go({ kind: 'game', id: g.id })
  const openPc = (o: PcGame) => go({ kind: 'pc', store: o.store, id: o.id })

  function catPage(cat: Cat) {
    if (query.trim()) {
      return (
        <div className="page list-page">
          <h1 className="page-title">חיפוש: <span dir="auto">{query}</span></h1>
          {gFiltered.length > 0 && <><h2 className="sub">התרגומים שלנו</h2><div className="grid">{gFiltered.map((g) => <GameCard key={g.id} g={g} onOpen={() => openGame(g)} />)}</div></>}
          {oFiltered.length > 0 && <><h2 className="sub">משחקים במחשב</h2><div className="grid tiles">{oFiltered.map((o) => <PcTile key={o.store + o.id} o={o} onOpen={() => openPc(o)} />)}</div></>}
          {!gFiltered.length && !oFiltered.length && <Empty text="לא נמצא משחק בשם הזה." />}
        </div>
      )
    }
    if (cat === 'home') {
      const mine = games.filter((g) => g.installed || g.location)
      return (
        <div className="page home">
          <Banner games={featured} onOpen={openGame} />
          {mine.length > 0 && (
            <Shelf heading="המשחקים שלך עם תרגום" count={mine.length}>
              {mine.map((g) => <GameCard key={g.id} g={g} onOpen={() => openGame(g)} />)}
            </Shelf>
          )}
          <Shelf heading="התרגומים שלנו" count={games.length} onAll={() => go({ kind: 'cat', cat: 'ours' })}>
            {games.map((g) => <GameCard key={g.id} g={g} onOpen={() => openGame(g)} />)}
          </Shelf>
          {others.length > 0 && (
            <Shelf heading="משחקים במחשב שלך" count={others.length} onAll={() => go({ kind: 'cat', cat: 'pc' })}>
              {others.slice(0, 40).map((o) => <PcTile key={o.store + o.id} o={o} onOpen={() => openPc(o)} />)}
            </Shelf>
          )}
        </div>
      )
    }
    const label = CATS.find((c) => c.id === cat)!.label
    if (cat === 'pc') {
      return (
        <div className="page list-page">
          <h1 className="page-title">{label}<span className="count">{others.length}</span></h1>
          <p className="lead">המשחקים שמצאנו ב-Steam וב-Epic במחשב הזה. עדיין אין לנו תרגום אליהם.</p>
          {others.length ? <div className="grid tiles">{others.map((o) => <PcTile key={o.store + o.id} o={o} onOpen={() => openPc(o)} />)}</div> : <Empty text="לא נמצאו משחקים של Steam או Epic." />}
        </div>
      )
    }
    const list = games.filter(cat === 'installed' ? isInstalled : cat === 'updates' ? hasUpdate : cat === 'soon' ? isSoon : () => true)
    const empty = { ours: 'אין עדיין תרגומים ברשימה.', installed: 'עוד לא התקנתם תרגום.', updates: 'כל התרגומים שלכם מעודכנים.', soon: 'אין כרגע תרגומים בהכנה.' }[cat as 'ours']
    return (
      <div className="page list-page">
        <h1 className="page-title">{label}<span className="count">{list.length}</span></h1>
        {list.length ? <div className="grid">{list.map((g) => <GameCard key={g.id} g={g} onOpen={() => openGame(g)} />)}</div> : <Empty text={empty} />}
      </div>
    )
  }

  let content: React.ReactNode
  if (!lib) content = <div className="page loading"><div className="spinner" /><p>סורק את המחשב ובודק תרגומים…</p></div>
  else if (page.kind === 'game' && !query.trim()) {
    const g = games.find((x) => x.id === page.id)
    content = g ? (
      <GamePage g={g} busy={busy} progress={progress} onBack={back} onAct={(k) => act(k, g)} onPick={() => pick(g)}
        onOpenFolder={() => api.openFolder(g.id)} onOpenUrl={(u) => api.openUrl(u)} />
    ) : <Empty text="המשחק לא נמצא ברשימה." />
  } else if (page.kind === 'pc' && !query.trim()) {
    const o = others.find((x) => x.store === page.store && x.id === page.id)
    content = o ? <PcGamePage o={o} onBack={back} /> : <Empty text="המשחק לא נמצא." />
  } else content = catPage(page.kind === 'cat' ? page.cat : 'home')

  const activeCat = page.kind === 'cat' ? page.cat : page.kind === 'pc' ? 'pc' : null
  const progGame = progress && games.find((g) => g.id === progress.gameId)

  return (
    <div className="app">
      <TitleBar query={query} setQuery={setQuery} searchRef={searchRef} onRefresh={refresh} refreshing={refreshing} />
      <div className="body">
        <nav className="sidebar">
          {CATS.map((c) => (
            <button key={c.id} className={`nav ${activeCat === c.id && !query ? 'on' : ''}`} onClick={() => { setQuery(''); go({ kind: 'cat', cat: c.id }) }}>
              <Icon g={c.icon} />
              <span>{c.label}</span>
              {counts[c.id] != null && counts[c.id]! > 0 && <span className={`badge ${c.id === 'updates' ? 'hot' : ''}`}>{counts[c.id]}</span>}
            </button>
          ))}
          <div className="grow" />
          <button className="nav" onClick={openSettings}><Icon g={ICON.settings} /><span>הגדרות</span></button>
        </nav>
        <main ref={mainRef}>{content}</main>
      </div>

      <div className="island" aria-live="polite">
        {toasts.map((t) => (
          <div key={t.id} className={`toast ${t.kind}`}>
            <Icon g={t.kind === 'error' ? ICON.warn : t.kind === 'ok' ? ICON.ok : ICON.info} />
            <span>{t.text}</span>
          </div>
        ))}
      </div>

      <footer className="statusbar">
        <span className={`dot ${lib?.catalog.source ?? ''}`} />
        <span title={lib?.catalog.error ?? ''}>{lib?.catalog.source ? CATALOG_SOURCE[lib.catalog.source] : 'טוען…'}</span>
        {lib && <span className="sep">{games.length} תרגומים · {others.length} משחקים במחשב</span>}
        {progGame && <span className="sep busy-note">{title(progGame)}: עובד…</span>}
        <span className="grow" />
        {info?.dev && <span className="dev">מצב בדיקה</span>}
        {self.available && (
          <button className="self-update" onClick={updateLauncher} disabled={self.progress !== undefined}>
            <Icon g={ICON.update} />
            {self.progress === undefined
              ? <span>גרסה חדשה <span dir="ltr">{self.available.version}</span>: עדכון</span>
              : <span>מעדכן…{self.progress != null && <span dir="ltr"> {Math.round(self.progress * 100)}%</span>}</span>}
          </button>
        )}
        {info && <span dir="ltr">Tirgumim {info.version}</span>}
      </footer>

      {confirm && (
        <div className="scrim" onMouseDown={() => { confirm.resolve(false); setConfirm(null) }}>
          <div className="dialog" role="dialog" aria-modal="true" onMouseDown={(e) => e.stopPropagation()}>
            <h2>{confirm.title}</h2>
            <p>{confirm.body}</p>
            <div className="dialog-actions">
              <button className={`btn ${confirm.danger ? 'danger' : 'primary'}`} autoFocus onClick={() => { confirm.resolve(true); setConfirm(null) }}>{confirm.ok}</button>
              <button className="btn ghost" onClick={() => { confirm.resolve(false); setConfirm(null) }}>ביטול</button>
            </div>
          </div>
        </div>
      )}

      {settings && (
        <div className="scrim" onMouseDown={() => setSettings(null)}>
          <div className="dialog" role="dialog" aria-modal="true" onMouseDown={(e) => e.stopPropagation()}>
            <h2>הגדרות</h2>
            <label className="field">
              <span>כתובת רשימת התרגומים</span>
              <input dir="ltr" value={settings.url} placeholder={settings.placeholder} onChange={(e) => setSettings({ ...settings, url: e.target.value })} spellCheck={false} />
              <small>השאירו ריק כדי להשתמש ברשימה הרגילה מ-GitHub.</small>
            </label>
            <div className="dialog-actions">
              <button className="btn primary" onClick={saveSettings}>שמירה</button>
              <button className="btn ghost" onClick={() => setSettings(null)}>ביטול</button>
            </div>
          </div>
        </div>
      )}
    </div>
  )
}
