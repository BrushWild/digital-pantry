import { DbConnection, tables, type ErrorContext } from './module_bindings';
import './style.css';

const HOST = 'wss://maincloud.spacetimedb.com';
const DB_NAME = 'digital-pantry';
const TOKEN_KEY = 'digital-pantry-token';

const app = document.getElementById('app')!;
void app; // container reference reserved
let activeFilter = 'all';
let activeConn: DbConnection | null = null;

// ── Filter buttons ─────────────────────────────────────────────────────
const filterButtons = Array.from(document.querySelectorAll<HTMLButtonElement>('.filter'));
filterButtons.forEach((btn) => {
  btn.addEventListener('click', () => {
    activeFilter = btn.dataset.filter ?? 'all';
    filterButtons.forEach((b) => {
      b.classList.toggle('active', b.dataset.filter === activeFilter);
    });
    if (activeConn) render(activeConn);
  });
});

// ── Render ─────────────────────────────────────────────────────────────
const STATUS_LABEL: Record<string, string> = {
  Unopened: 'Unopened',
  Opened: 'Opened',
  ExpiringSoon: 'Expiring soon',
  Depleted: 'Depleted',
};

function fmtExpiry(ts: bigint): string {
  const secs = Number(ts);
  if (secs <= 0) return 'No expiry set';
  const nowSecs = Math.floor(Date.now() / 1000);
  const diff = secs - nowSecs;
  const dayMs = 86_400_000;
  if (diff < 0) {
    const days = Math.floor(-diff / 86_400);
    return days <= 1 ? 'Expired' : `Expired ${days}d ago`;
  }
  const days = Math.ceil(diff / 86_400);
  if (days <= 1) return 'Expires today';
  return `Expires in ${days}d`;
}

function fmtQty(q: number, unit: string): string {
  if (!unit) return String(q);
  const qStr = Number.isInteger(q) ? String(q) : q.toFixed(1);
  return `${qStr} ${unit}`;
}

function esc(text: string): string {
  const div = document.createElement('div');
  div.textContent = text;
  return div.innerHTML;
}

function render(conn: DbConnection) {
  const items = Array.from(conn.db.item.iter());
  const active = items.filter((i) => i.status !== 'Depleted');
  const depleted = items.filter((i) => i.status === 'Depleted');
  void depleted; // reserved for a "show depleted" filter later

  // Apply active filter
  const filtered = active.filter((i) => {
    if (activeFilter === 'all') return true;
    if (activeFilter === 'expiringsoon') return i.status === 'ExpiringSoon';
    if (activeFilter === 'opened') return i.status === 'Opened';
    if (activeFilter === 'unopened') return i.status === 'Unopened';
    return true;
  });

  // Sort: expiring soon first, then by expiry date
  const sorted = [...filtered].sort((a, b) => {
    const aSoon = a.status === 'ExpiringSoon' ? 0 : 1;
    const bSoon = b.status === 'ExpiringSoon' ? 0 : 1;
    if (aSoon !== bSoon) return aSoon - bSoon;
    return Number(a.estExpiryTs) - Number(b.estExpiryTs);
  });

  const countEl = document.getElementById('count');
  if (countEl) countEl.textContent = String(active.length);

  const grid = document.getElementById('items');
  if (!grid) return;

  if (filtered.length === 0) {
    grid.innerHTML =
      '<div class="empty">No active items. Add food via Discord and it will appear here in real time.</div>';
    return;
  }

  grid.innerHTML = sorted
    .map((i) => {
      const status = STATUS_LABEL[i.status] ?? i.status;
      const price = i.price > 0 ? ` · $${i.price.toFixed(2)}${i.currency ? ' ' + i.currency : ''}` : '';
      const expiry = fmtExpiry(i.estExpiryTs);
      return `
      <div class="item status-${i.status.toLowerCase()}">
        <div class="item-head">
          <span class="item-name">${esc(i.displayName)}</span>
          <span class="badge badge-${i.status.toLowerCase()}">${status}</span>
        </div>
        <div class="item-meta">
          <span class="loc">${esc(i.location)}</span>
          <span class="qty">${fmtQty(i.quantity, i.unit)}</span>
        </div>
        <div class="item-expiry">${expiry}${price}</div>
      </div>`;
    })
    .join('');
}

// ── Connect ────────────────────────────────────────────────────────────
const conn = DbConnection.builder()
  .withUri(HOST)
  .withDatabaseName(DB_NAME)
  .withToken(localStorage.getItem(TOKEN_KEY) ?? undefined)
  .onConnect((conn: DbConnection, _identity, token: string) => {
    localStorage.setItem(TOKEN_KEY, token);
    activeConn = conn;
    const status = document.getElementById('status')!;
    status.textContent = 'Connected';
    status.className = 'status ok';

    conn
      .subscriptionBuilder()
      .onApplied(() => render(conn))
      .subscribe(tables.item);

    conn.db.item.onInsert(() => render(conn));
    conn.db.item.onUpdate(() => render(conn));
    conn.db.item.onDelete(() => render(conn));
  })
  .onDisconnect(() => {
    const status = document.getElementById('status')!;
    status.textContent = 'Reconnecting…';
    status.className = 'status warn';
  })
  .onConnectError((_ctx: ErrorContext, error: Error) => {
    const status = document.getElementById('status')!;
    status.textContent = 'Error: ' + error.message;
    status.className = 'status err';
    console.error('Connection error:', error);
  })
  .build();
