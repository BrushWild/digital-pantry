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

// ── Ingest panel (receipt photo → submit_ingest) ─────────────────────
const panel = document.getElementById('ingest-panel')!;
const fileInput = document.getElementById('ingest-file') as HTMLInputElement;
const preview = document.getElementById('ingest-preview') as HTMLImageElement;
const ingestStatus = document.getElementById('ingest-status')!;
const submitBtn = document.getElementById('ingest-submit') as HTMLButtonElement;
let pendingDataUrl: string | null = null;

function setIngestStatus(text: string) {
  ingestStatus.textContent = text;
}

function closeIngestPanel() {
  panel.hidden = true;
  fileInput.value = '';
  pendingDataUrl = null;
  preview.hidden = true;
  preview.src = '';
  submitBtn.hidden = true;
  setIngestStatus('');
}

function pickPhoto(capture: boolean) {
  // ponytail: native `capture` attribute = camera on mobile, plain picker on
  // desktop; getUserMedia is skipped until someone needs in-page focus/zoom.
  if (capture) fileInput.setAttribute('capture', 'environment');
  else fileInput.removeAttribute('capture');
  fileInput.click();
}

// ponytail: fixed 1600px long side + JPEG 0.85; add a quality/size knob if
// payload size ever exceeds the reducer message limit.
function fileToDataUrl(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const url = URL.createObjectURL(file);
    const img = new Image();
    img.onload = () => {
      URL.revokeObjectURL(url);
      const scale = Math.min(1, 1600 / Math.max(img.width, img.height));
      const canvas = document.createElement('canvas');
      canvas.width = Math.round(img.width * scale);
      canvas.height = Math.round(img.height * scale);
      canvas.getContext('2d')!.drawImage(img, 0, 0, canvas.width, canvas.height);
      resolve(canvas.toDataURL('image/jpeg', 0.85));
    };
    img.onerror = () => {
      URL.revokeObjectURL(url);
      reject(new Error('could not read image'));
    };
    img.src = url;
  });
}

document.getElementById('add-receipt')!.addEventListener('click', () => {
  panel.hidden = !panel.hidden;
  if (panel.hidden) closeIngestPanel();
});
document.getElementById('ingest-cancel')!.addEventListener('click', closeIngestPanel);
document.getElementById('ingest-upload')!.addEventListener('click', () => pickPhoto(false));
document.getElementById('ingest-capture')!.addEventListener('click', () => pickPhoto(true));
fileInput.addEventListener('change', async () => {
  const file = fileInput.files?.[0];
  if (!file) return;
  try {
    pendingDataUrl = await fileToDataUrl(file);
  } catch (e) {
    setIngestStatus('Could not read that file: ' + (e as Error).message);
    return;
  }
  preview.src = pendingDataUrl;
  preview.hidden = false;
  submitBtn.hidden = false;
  setIngestStatus('Ready — hit "Submit receipt" to queue it.');
});
submitBtn.addEventListener('click', async () => {
  if (!pendingDataUrl || !activeConn) return;
  submitBtn.disabled = true;
  setIngestStatus('Submitting…');
  try {
    // ponytail: generated DbView type omits `reducers` (it's a top-level
    // connection property); cast until the bindings surface it.
    await (activeConn as {
      reducers: { submitIngest: (p: { kind: string; payload: string }) => Promise<void> };
    }).reducers.submitIngest({ kind: 'receipt_photo', payload: pendingDataUrl });
    setIngestStatus('Queued ✓ — items appear here once the agent processes the receipt.');
    setTimeout(closeIngestPanel, 4000);
  } catch (e) {
    setIngestStatus('Submit failed: ' + (e as Error).message);
  } finally {
    submitBtn.disabled = false;
  }
});

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
