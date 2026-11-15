// The store: translations that are sold, with their price and one button. A bought translation moves to the library.
import { formatPrice, type AccountState, type Store as StoreData, type StoreItem } from './api.ts'
import { Art, Icon, ICON } from './ui.tsx'
import { errorText } from './texts.ts'
import { asError } from './api.ts'

function Item({ it, onBuy }: { it: StoreItem; onBuy: () => void }) {
  const price = formatPrice(it.price)
  const owned = it.access === 'owned'
  return (
    <article className="shop-item">
      <Art src={it.cover} label={it.name} className="cover" />
      <div className="shop-info">
        {it.nameHe && <div className="en shop-en">{it.name}</div>}
        <h3>{it.nameHe || it.name}</h3>
        {it.aboutHe && <p>{it.aboutHe}</p>}
        <div className="shop-buy">
          {owned ? (
            <span className="done-note"><Icon g={ICON.ok} /> בחשבון שלך. אפשר למצוא אותו בספרייה.</span>
          ) : it.forSale ? (
            <button className="btn primary big glow" onClick={onBuy}>{price ? `לרכישה ${price}` : 'לרכישה'}</button>
          ) : (
            <span className="shop-soon">{it.priceUnset ? 'עוד לא עומד למכירה' : 'לא זמין כרגע'}</span>
          )}
        </div>
      </div>
    </article>
  )
}

export function StorePage({ store, account, onBuy, onSignIn, onReload }: {
  store: StoreData | null
  account: AccountState | null
  onBuy: (gameId: string) => void
  onSignIn: () => void
  onReload: () => void
}) {
  if (!store) return <div className="page loading"><div className="spinner" /><p>טוען את החנות…</p></div>
  return (
    <div className="page list-page">
      <h1 className="page-title">חנות<span className="count">{store.items.length}</span></h1>
      <p className="lead">תרגומים בתשלום. אחרי הרכישה התרגום מופיע בספרייה שלכם, בכל מחשב שמחובר לחשבון.</p>

      {store.configured && !store.signedIn && (
        <div className="account-note">
          <Icon g={ICON.account} />
          <span>כדי לרכוש ולקבל תרגום בספרייה, צריך להתחבר עם אימייל.</span>
          <button className="btn primary" onClick={onSignIn}>התחברות</button>
        </div>
      )}
      {store.error && (
        <div className="account-note warn">
          <Icon g={ICON.warn} />
          <span>{errorText(asError(store.error))}</span>
          <button className="btn ghost" onClick={onReload}>נסו שוב</button>
        </div>
      )}
      {!store.configured && <div className="account-note"><Icon g={ICON.info} /><span>החנות עוד לא נפתחה. כשתיפתח, התרגומים בתשלום יופיעו כאן.</span></div>}
      {store.configured && !store.paymentsConfigured && !store.error && <div className="account-note"><Icon g={ICON.info} /><span>הרכישה עוד לא פתוחה. נעדכן כאן כשהיא תיפתח.</span></div>}

      {store.items.length > 0 ? (
        <div className="shop-grid">{store.items.map((it) => <Item key={it.gameId} it={it} onBuy={() => (account?.status === 'signedIn' ? onBuy(it.gameId) : onSignIn())} />)}</div>
      ) : store.configured && !store.error && (
        <div className="empty"><Icon g={ICON.store} /><p>אין עדיין תרגומים בחנות.</p></div>
      )}
    </div>
  )
}
