'use strict';
/* global tirgumim */

const STATUS = {
  notFound: 'המשחק לא נמצא',
  noPack: 'תרגום בהכנה',
  ready: 'מוכן להתקנה',
  installed: 'התרגום מותקן',
  updateAvailable: 'יש עדכון',
  needsRepair: 'דרוש תיקון',
};
const SOURCE = { steam: 'Steam', epic: 'Epic Games', custom: 'תיקייה שנבחרה', installed: 'תיקיית ההתקנה' };
const STEP = { download: 'מוריד…', verify: 'בודק את החבילה…', backup: 'מגבה קבצים מקוריים…', copy: 'מתקין קבצים…', done: 'הסתיים' };
const ERRORS = {
  ZIP_HASH: 'קובץ החבילה פגום. נסו שוב.',
  ZIP_READ: 'לא ניתן לקרוא את קובץ החבילה.',
  NO_MANIFEST: 'החבילה לא תקינה (חסר קובץ הגדרות).',
  BAD_MANIFEST: 'החבילה לא תקינה.',
  MISSING_FILE: 'החבילה לא שלמה.',
  FILE_HASH: 'קובץ בתוך החבילה פגום.',
  WRONG_GAME: 'החבילה מיועדת למשחק אחר.',
  LAUNCHER_TOO_OLD: 'החבילה דורשת גרסה חדשה יותר של המשגר. עדכנו את תרגומים.',
  GAME_VERSION: 'גרסת המשחק שלכם אינה נתמכת בתרגום הזה.',
  NO_GAME_DIR: 'תיקיית המשחק לא נמצאה.',
  NOT_GAME_DIR: 'התיקייה שנבחרה אינה תיקיית המשחק.',
  ALREADY_INSTALLED: 'כבר מותקן תרגום למשחק הזה.',
  NOT_INSTALLED: 'לא מותקן תרגום למשחק הזה.',
  MISSING_BACKUP: 'חסר גיבוי של חלק מהקבצים המקוריים. מומלץ לאמת את קבצי המשחק ב-Steam/Epic.',
  NO_PACK: 'עדיין אין תרגום למשחק הזה.',
  CATALOG: 'לא ניתן לטעון את רשימת המשחקים. בדקו את החיבור לאינטרנט.',
  BUSY: 'פעולה אחרת עדיין רצה.',
};

const $ = (s) => document.querySelector(s);
const el = (tag, cls, text) => { const e = document.createElement(tag); if (cls) e.className = cls; if (text != null) e.textContent = text; return e; };

let progress = null; // { gameId, step, fraction }
let running = null;

function banner(msg, kind) {
  const b = $('#banner');
  if (!msg) { b.hidden = true; return; }
  b.hidden = false; b.className = `banner ${kind || ''}`; b.textContent = msg;
}

function errText(err) {
  const base = ERRORS[err.code] || 'אירעה שגיאה.';
  return err.code in ERRORS && err.code !== 'BAD_MANIFEST' ? base : `${base} (${err.message})`;
}

function confirmBox(title, body, okText) {
  const d = $('#confirm');
  $('#confirm-title').textContent = title;
  $('#confirm-body').textContent = body;
  $('#confirm-ok').textContent = okText || 'אישור';
  d.showModal();
  return new Promise((r) => d.addEventListener('close', () => r(d.returnValue === 'ok'), { once: true }));
}

function fmtSize(n) {
  if (!n) return '';
  // LRI…PDI keeps "12 MB" left-to-right inside the RTL line
  if (n < 1024 * 1024) return `⁦${Math.max(1, Math.round(n / 1024))} KB⁩`;
  return `⁦${(n / 1024 / 1024).toFixed(1)} MB⁩`;
}
function fmtDate(s) { try { return new Date(s).toLocaleDateString('he-IL'); } catch { return s; } }

function row(dl, label, value, cls) {
  dl.append(el('dt', null, label));
  const dd = el('dd', cls, value);
  dl.append(dd);
}

function poster(src, label, store) {
  const p = el('div', 'poster');
  const img = el('img');
  img.loading = 'lazy';
  img.alt = '';
  img.src = src;
  img.addEventListener('error', () => p.classList.add('failed'));
  p.append(img, el('div', 'ph', label));
  if (store) p.append(el('span', 'store', store === 'steam' ? 'Steam' : 'Epic'));
  return p;
}

function card(g) {
  const outer = el('article', 'card with-poster');
  outer.append(poster(g.poster, g.name));
  const c = el('div', 'card-body');
  outer.append(c);
  const head = el('div', 'card-head');
  const titles = el('div');
  titles.append(el('h2', null, g.nameHe || g.name));
  if (g.nameHe) titles.append(el('div', 'en', g.name));
  head.append(titles, el('span', `status ${g.status}`, STATUS[g.status] || g.status));
  c.append(head);

  const dl = el('dl', 'meta');
  if (g.location) {
    row(dl, 'מיקום', SOURCE[g.location.source] || g.location.source);
    row(dl, 'תיקייה', g.location.dir, 'path');
  }
  if (g.installed) row(dl, 'גרסה מותקנת', `${g.installed.version} · ${fmtDate(g.installed.installedAt)}`);
  if (g.available) row(dl, 'גרסה זמינה', [g.available.version, fmtSize(g.available.size)].filter(Boolean).join(' · '));
  if (g.available && g.available.changelog && g.status !== 'installed') row(dl, 'מה חדש', g.available.changelog);
  if (dl.children.length) c.append(dl);

  if (g.status === 'notFound') c.append(el('p', 'note', 'לא מצאנו את המשחק ב-Steam או ב-Epic. אם הוא מותקן במקום אחר, בחרו את התיקייה שלו.'));
  if (g.status === 'needsRepair') c.append(el('p', 'note', 'חלק מקבצי התרגום הוחלפו, כנראה אחרי עדכון של המשחק. התקינו מחדש כדי לתקן.'));
  if (g.installed && g.installed.forced) c.append(el('p', 'note', 'הותקן על גרסת משחק שלא נבדקה.'));

  if (progress && progress.gameId === g.id) {
    const p = el('div', `progress${progress.fraction == null ? ' indeterminate' : ''}`);
    const bar = el('span'); if (progress.fraction != null) bar.style.width = `${Math.round(progress.fraction * 100)}%`;
    p.append(bar);
    c.append(p, el('div', 'progress-label', STEP[progress.step] || ''));
  }

  const a = el('div', 'actions');
  const btn = (text, cls, fn, disabled) => {
    const b = el('button', `btn ${cls || ''}`, text);
    b.disabled = !!running || !!disabled;
    b.addEventListener('click', fn);
    a.append(b);
  };
  if (g.status === 'ready') btn('התקנת תרגום', 'primary', () => act('install', g));
  if (g.status === 'updateAvailable') btn('עדכון תרגום', 'primary', () => act('update', g));
  if (g.status === 'needsRepair') btn('התקנה מחדש', 'primary', () => act('update', g));
  if (g.installed) btn('הסרת תרגום', 'danger', () => act('uninstall', g));
  if (!g.installed) btn(g.location ? 'שינוי תיקייה' : 'בחירת תיקייה', g.location ? 'ghost' : '', () => pick(g));
  if (g.location) btn('פתיחת התיקייה', 'ghost', () => tirgumim.openFolder(g.id));
  c.append(a);
  return outer;
}

function tile(o) {
  const t = el('div', 'tile');
  t.title = `${o.name}\n${o.dir}`;
  t.append(poster(o.poster, o.name, o.store), el('div', 'name', o.name));
  return t;
}

const CATALOG_SRC = {
  github: 'רשימת התרגומים נטענה מ-GitHub',
  bundled: 'רשימה מקומית (אין חיבור ל-GitHub)',
  dev: 'רשימת בדיקה מקומית',
};

function matches(...names) {
  const q = $('#search').value.trim().toLowerCase();
  return !q || names.some((n) => n && n.toLowerCase().includes(q));
}

function render() {
  const tr = $('#translations');
  tr.replaceChildren();
  const shown = data.translations.filter((g) => matches(g.name, g.nameHe));
  if (!shown.length) tr.append(el('div', 'empty', data.translations.length ? 'אין תוצאות.' : 'אין עדיין תרגומים ברשימה.'));
  for (const g of shown) tr.append(card(g));

  const ot = $('#others');
  ot.replaceChildren();
  const others = data.others.filter((o) => matches(o.name));
  for (const o of others) ot.append(tile(o));
  $('#others-count').textContent = data.others.length ? `${data.others.length} משחקים נמצאו` : 'לא נמצאו משחקים של Steam או Epic';

  const src = data.catalog || {};
  $('#catalog-source').textContent = CATALOG_SRC[src.source] || '';
  $('#catalog-source').title = src.error || '';
}

let data = { translations: [], others: [], catalog: null };

async function load(reload) {
  const r = await tirgumim.list(reload);
  if (!r.ok) { banner(errText(r.error), ''); return; }
  data = r.result;
  render();
}

async function pick(g) {
  const r = await tirgumim.pickFolder(g.id);
  if (!r.ok) banner(errText(r.error));
  else if (r.result) banner('התיקייה נשמרה.', 'ok');
  await load(false);
}

async function act(kind, g, force = false) {
  const name = g.nameHe || g.name;
  if (kind === 'uninstall') {
    const ok = await confirmBox('הסרת התרגום', `להסיר את התרגום מ"${name}" ולהחזיר את הקבצים המקוריים?`, 'הסרה');
    if (!ok) return;
  }
  banner(null);
  running = kind;
  progress = { gameId: g.id, step: kind === 'uninstall' ? 'copy' : 'download', fraction: null };
  render();
  const r = kind === 'uninstall' ? await tirgumim.uninstall(g.id) : await tirgumim[kind](g.id, { force });
  running = null; progress = null;
  if (r.ok) {
    if (kind === 'uninstall') {
      const kept = r.result.skippedChanged.length;
      banner(kept ? `התרגום הוסר. ${kept} קבצים עודכנו בינתיים על ידי המשחק ונשארו כפי שהם.` : 'התרגום הוסר והקבצים המקוריים שוחזרו.', 'ok');
    } else {
      banner(kind === 'install' ? 'התרגום הותקן. משחק מהנה!' : 'התרגום עודכן.', 'ok');
    }
  } else if (r.error.code === 'GAME_VERSION' && r.error.allowUnknown && !force) {
    await load(false);
    const ok = await confirmBox('גרסת משחק לא מוכרת', 'התרגום הוכן לגרסה אחרת של המשחק. בדרך כלל זה עובד, אבל ייתכן שחלק מהטקסטים יופיעו באנגלית. אפשר תמיד להסיר את התרגום ולשחזר את הקבצים המקוריים. להתקין בכל זאת?', 'התקנה בכל זאת');
    if (ok) return act(kind, g, true);
  } else {
    banner(errText(r.error));
  }
  await load(false);
}

tirgumim.onProgress((p) => {
  if (!running) return;
  if (p) { progress = p; render(); }
});

async function openSettings() {
  const s = await tirgumim.getSettings();
  const d = $('#settings-dlg');
  $('#catalog-url').value = s.catalogUrl;
  $('#catalog-url').placeholder = s.defaultCatalogUrl;
  d.showModal();
  d.addEventListener('close', async () => {
    if (d.returnValue !== 'ok') return;
    const r = await tirgumim.setSettings({ catalogUrl: $('#catalog-url').value });
    if (!r.ok) { banner('הכתובת צריכה להתחיל ב-https://'); return; }
    await load(true);
  }, { once: true });
}

window.__refresh = () => load(false);

(async () => {
  const info = await tirgumim.info();
  $('#version').textContent = `תרגומים ${info.version}`;
  $('#dev-badge').hidden = !info.dev;
  $('#reload').addEventListener('click', async () => { banner('סורק את המחשב…', 'ok'); await load(true); banner(null); });
  $('#search').addEventListener('input', render);
  $('#settings').addEventListener('click', openSettings);
  await load(false);
})();
