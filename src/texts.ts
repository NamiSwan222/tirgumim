// Every Hebrew text the window shows, in one place.
import type { Game, PackError, Status } from './api.ts'

export const STATUS: Record<Status, string> = {
  notFound: 'המשחק לא נמצא',
  noPack: 'תרגום בהכנה',
  unavailable: 'התרגום לא זמין כרגע',
  ready: 'מוכן להתקנה',
  installed: 'התרגום מותקן',
  updateAvailable: 'יש עדכון',
  needsRepair: 'דרוש תיקון',
  playing: 'פועל בעברית עכשיו',
}

export const SOURCE: Record<string, string> = {
  steam: 'Steam',
  epic: 'Epic Games',
  custom: 'תיקייה שבחרתם',
  installed: 'תיקיית ההתקנה',
}

export const STEP: Record<string, string> = {
  download: 'מוריד את התרגום…',
  verify: 'בודק את החבילה…',
  patch: 'מכין את הקבצים המתורגמים…',
  backup: 'מגבה את הקבצים המקוריים…',
  copy: 'מתקין קבצים…',
  done: 'הסתיים',
  uninstall: 'מסיר את התרגום…',
}

export const CATALOG_SOURCE: Record<string, string> = {
  github: 'הרשימה נטענה מ-GitHub',
  custom: 'הרשימה נטענה מכתובת מותאמת',
  bundled: 'רשימה מקומית (אין חיבור ל-GitHub)',
  dev: 'רשימת בדיקה מקומית',
}

const ERRORS: Record<string, string> = {
  ZIP_HASH: 'קובץ התרגום שהורד פגום. נסו שוב.',
  ZIP_READ: 'לא ניתן לקרוא את קובץ התרגום.',
  NO_MANIFEST: 'חבילת התרגום לא תקינה.',
  BAD_MANIFEST: 'חבילת התרגום לא תקינה.',
  MISSING_FILE: 'חבילת התרגום לא שלמה.',
  FILE_HASH: 'קובץ בתוך חבילת התרגום פגום.',
  WRONG_GAME: 'החבילה מיועדת למשחק אחר.',
  LAUNCHER_TOO_OLD: 'התרגום דורש גרסה חדשה יותר של המשגר. עדכנו את תרגומים.',
  GAME_VERSION: 'גרסת המשחק שלכם אינה נתמכת בתרגום הזה.',
  GAME_RUNNING: 'המשחק פתוח. סגרו אותו ונסו שוב.',
  REQUIRES: 'חסר משהו בתיקיית המשחק:',
  PATCH_SOURCE: 'קבצי המשחק שלכם שונים מאלה שהתרגום הוכן עבורם (גרסה אחרת, או קובץ ששונה כבר). אמתו את קבצי המשחק ב-Steam או ב-Epic ונסו שוב. הקבצים:',
  PATCH_FAILED: 'לא הצלחנו להחיל את התרגום על קבצי המשחק. לא שונה דבר במשחק.',
  NO_GAME_DIR: 'תיקיית המשחק לא נמצאה.',
  NOT_GAME_DIR: 'התיקייה שבחרתם אינה תיקיית המשחק.',
  ALREADY_INSTALLED: 'כבר מותקן תרגום למשחק הזה.',
  NOT_INSTALLED: 'לא מותקן תרגום למשחק הזה.',
  MISSING_BACKUP: 'חסר גיבוי של חלק מהקבצים המקוריים. מומלץ לאמת את קבצי המשחק ב-Steam או ב-Epic.',
  NO_PACK: 'עדיין אין תרגום למשחק הזה.',
  PACK_UNAVAILABLE: 'התרגום לא זמין כרגע. נסו לרענן בעוד כמה דקות.',
  DOWNLOAD: 'ההורדה נכשלה. בדקו את החיבור לאינטרנט ונסו שוב.',
  BUSY: 'פעולה אחרת עדיין רצה.',
  BAD_URL: 'הכתובת צריכה להתחיל ב-https://',
  IO: 'לא ניתן לכתוב לתיקיית המשחק. ייתכן שקובץ פתוח או שאין הרשאה.',
  GAME_ALREADY_RUNNING: 'המשחק כבר פתוח (כנראה הופעל מ-Steam). סגרו אותו ולחצו שוב על "שחק בעברית".',
  SESSION_ACTIVE: 'המשחק כבר פועל בעברית.',
  STEAM_MISSING: 'Steam לא מותקן במחשב הזה. כדי לשחק בעברית צריך להתקין את Steam ולהתחבר אליו.',
  STEAM_NO_APPID: 'חסרים פרטי Steam למשחק הזה ברשימת התרגומים.',
  EXE_MISSING: 'קובץ ההפעלה של המשחק לא נמצא בתיקיית המשחק.',
  LAUNCH_FAILED: 'לא הצלחנו להפעיל את המשחק.',
  NOT_LAUNCH_ONLY: 'התרגום הזה פועל תמיד. הפעילו את המשחק כרגיל.',
  SWAP_ACTIVE: 'המשחק פועל עכשיו בעברית. סגרו אותו קודם.',
  SWAP_FAILED: 'לא הצלחנו להכניס את קבצי התרגום. קבצי המשחק המקוריים הוחזרו.',
  RESTORE_FAILED: 'לא הצלחנו להחזיר את כל קבצי המשחק המקוריים. סגרו את המשחק ופתחו את המשגר שוב, הוא ינסה שוב.',
  GAME_CHANGED: 'המשחק התעדכן מאז שהתרגום הוכן. התקינו את התרגום מחדש.',
  NEEDS_REPAIR: 'חלק מקבצי התרגום חסרים או השתנו. התקינו את התרגום מחדש.',
  WOULD_REPLACE: 'חבילת התרגום מנסה להחליף קבצים של המשחק, וזה לא מותר בתרגום שפועל רק מהמשגר.',
}

export function errorText(e: PackError): string {
  const base = ERRORS[e.code] ?? 'אירעה שגיאה.'
  if ((e.code === 'REQUIRES' || e.code === 'PATCH_SOURCE' || e.code === 'GAME_CHANGED') && e.details?.length) return `${base} ${e.details.join(' · ')}`
  return e.code in ERRORS ? base : `${base} (${e.message})`
}

export const title = (g: Game) => g.nameHe || g.name

/** LRI…PDI keeps "12 MB" left-to-right inside a Hebrew line. */
export function size(n: number | null | undefined): string {
  if (!n) return ''
  if (n < 1024 * 1024) return `⁦${Math.max(1, Math.round(n / 1024))} KB⁩`
  return `⁦${(n / 1024 / 1024).toFixed(1)} MB⁩`
}

export function date(s: string | null | undefined): string {
  if (!s) return ''
  try {
    return new Date(s).toLocaleDateString('he-IL')
  } catch {
    return s
  }
}

export const ltr = (s: string) => `⁦${s}⁩`
