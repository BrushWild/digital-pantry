import { useLayoutEffect, useMemo, useRef, useState } from 'react';
import {
  SpacetimeDBProvider,
  useSpacetimeDB,
  useTable,
  useReducer,
} from 'spacetimedb/react';
import { DbConnection, tables, reducers } from './module_bindings';
import type { Item } from './module_bindings/types';

const HOST = 'wss://maincloud.spacetimedb.com';
const DB_NAME = 'digital-pantry';
const TOKEN_KEY = 'digital-pantry-token';

// The generated SDK types model unit-variant enums (status/location) as
// {tag: "Unopened"} objects. The raw subscription path historically decoded
// them to plain strings; the React adapter follows the generated types.
// Normalize once at the boundary so the app works either way.
// ponytail: upstream SDK should decode consistently; until then, this map.
const statusOf = (s: unknown): string =>
  typeof s === 'string' ? s : String((s as { tag?: unknown } | null)?.tag ?? 'Unopened');
const locOf = (l: unknown): string =>
  typeof l === 'string' ? l : String((l as { tag?: unknown } | null)?.tag ?? 'Shelf');

type ItemRow = Omit<Item, 'status' | 'location'> & {
  status: 'Unopened' | 'Opened' | 'ExpiringSoon' | 'Depleted';
  location: string;
};
const asItemRows = (rows: readonly Item[]): ItemRow[] =>
  rows.map((r): ItemRow => ({
    ...(r as Omit<Item, 'status' | 'location'>),
    status: statusOf(r.status) as ItemRow['status'],
    location: locOf(r.location),
  }));

// ── helpers ────────────────────────────────────────────────────────────
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

// ── status pill ────────────────────────────────────────────────────────
function StatusPill() {
  const s = useSpacetimeDB();
  if (!s.isActive) {
    return (
      <span className={`status ${s.connectionError ? 'err' : 'warn'}`}>
        {s.connectionError ? 'Error' : 'Connecting…'}
      </span>
    );
  }
  return <span className="status ok">Connected</span>;
}

// ── filter tabs with sliding pill (transitions.dev: "Tabs sliding") ────
const FILTERS: { key: string; label: string }[] = [
  { key: 'all', label: 'All' },
  { key: 'expiringsoon', label: 'Expiring' },
  { key: 'opened', label: 'Opened' },
  { key: 'unopened', label: 'Unopened' },
];

function FilterTabs({ active, onChange }: { active: string; onChange: (k: string) => void }) {
  const refs = useRef(new Map<string, HTMLButtonElement>());
  const [pill, setPill] = useState({ left: 0, width: 0 });

  useLayoutEffect(() => {
    const measure = () => {
      const el = refs.current.get(active);
      if (el) setPill({ left: el.offsetLeft, width: el.offsetWidth });
    };
    measure();
    window.addEventListener('resize', measure);
    return () => window.removeEventListener('resize', measure);
  }, [active]);

  return (
    <div className="filters">
      <span
        className="filter-pill"
        style={{ width: pill.width, transform: `translateX(${pill.left}px)` }}
        aria-hidden
      />
      {FILTERS.map((f) => (
        <button
          key={f.key}
          ref={(el) => {
            if (el) refs.current.set(f.key, el);
          }}
          className={`filter${active === f.key ? ' active' : ''}`}
          onClick={() => onChange(f.key)}
        >
          {f.label}
        </button>
      ))}
    </div>
  );
}

// ── item card ──────────────────────────────────────────────────────────
function ItemCard({ item }: { item: ItemRow }) {
  const status = STATUS_LABEL[item.status] ?? item.status;
  const price =
    item.price > 0
      ? ` · $${item.price.toFixed(2)}${item.currency ? ' ' + item.currency : ''}`
      : '';
  return (
    <div className={`item status-${item.status.toLowerCase()}`}>
      <div className="item-head">
        <span className="item-name">{item.displayName}</span>
        {/* key=status re-runs the "text states swap" animation on change */}
        <span key={item.status} className={`badge swap badge-${item.status.toLowerCase()}`}>
          {status}
        </span>
      </div>
      <div className="item-meta">
        <span className="loc">{item.location}</span>
        <span className="qty">{fmtQty(item.quantity, item.unit)}</span>
      </div>
      <div className="item-expiry">
        {fmtExpiry(item.estExpiryTs)}
        {price}
      </div>
    </div>
  );
}

// ── ingest panel ───────────────────────────────────────────────────────
function IngestPanel({ onClose }: { onClose: () => void }) {
  const [dataUrl, setDataUrl] = useState<string | null>(null);
  const [status, setStatus] = useState('');
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState(0); // bump key → retrigger shake
  const fileRef = useRef<HTMLInputElement>(null);
  const submitIngest = useReducer(reducers.submitIngest);

  function close() {
    setDataUrl(null);
    setStatus('');
    if (fileRef.current) fileRef.current.value = '';
    onClose();
  }

  function pickPhoto(capture: boolean) {
    const el = fileRef.current;
    if (!el) return;
    // ponytail: native `capture` attribute = camera on mobile, plain picker on
    // desktop; getUserMedia skipped until in-page focus/zoom is needed.
    if (capture) el.setAttribute('capture', 'environment');
    else el.removeAttribute('capture');
    el.click();
  }

  // ponytail: fixed 1600px long side + JPEG 0.85; add a quality/size knob if
  // the payload ever exceeds the reducer message limit.
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

  async function onFile(e: React.ChangeEvent<HTMLInputElement>) {
    const file = e.target.files?.[0];
    if (!file) return;
    try {
      setDataUrl(await fileToDataUrl(file));
      setStatus('Ready — hit "Submit receipt" to queue it.');
    } catch (err) {
      setError((n) => n + 1);
      setStatus('Could not read that file: ' + (err as Error).message);
    }
  }

  async function onSubmit() {
    if (!dataUrl || submitting) return;
    setSubmitting(true);
    setStatus('Submitting…');
    try {
      await submitIngest({ kind: 'receipt_photo', payload: dataUrl });
      setStatus('Queued ✓ — items appear here once the agent processes the receipt.');
      setTimeout(close, 4000);
    } catch (err) {
      setError((n) => n + 1);
      setStatus('Submit failed: ' + (err as Error).message);
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <div className="ingest-panel">
      {dataUrl && <img className="ingest-preview" alt="Receipt preview" src={dataUrl} />}
      <input ref={fileRef} type="file" accept="image/*" hidden onChange={onFile} />
      <button className="filter" onClick={() => pickPhoto(false)}>
        Upload photo…
      </button>
      <button className="filter" onClick={() => pickPhoto(true)}>
        Take photo
      </button>
      <button className="filter" disabled={submitting} onClick={onSubmit}>
        Submit receipt
      </button>
      <button className="filter" onClick={close}>
        Close
      </button>
      <div key={error} className={`ingest-status empty${error ? ' shake' : ''}`}>
        {status}
      </div>
    </div>
  );
}

// ── shell (inside the provider) ────────────────────────────────────────
function Shell() {
  const [filter, setFilter] = useState('all');
  const [ingestOpen, setIngestOpen] = useState(false);
  const [items, isReady] = useTable(tables.item);
  const rows = useMemo(() => asItemRows(items), [items]);

  const active = useMemo(() => rows.filter((i) => i.status !== 'Depleted'), [rows]);

  const sorted = useMemo(() => {
    const filtered = active.filter((i) => {
      if (filter === 'all') return true;
      if (filter === 'expiringsoon') return i.status === 'ExpiringSoon';
      if (filter === 'opened') return i.status === 'Opened';
      if (filter === 'unopened') return i.status === 'Unopened';
      return true;
    });
    return [...filtered].sort((a, b) => {
      const aSoon = a.status === 'ExpiringSoon' ? 0 : 1;
      const bSoon = b.status === 'ExpiringSoon' ? 0 : 1;
      if (aSoon !== bSoon) return aSoon - bSoon;
      return Number(a.estExpiryTs) - Number(b.estExpiryTs);
    });
  }, [active, filter]);

  return (
    <>
      <header>
        <div className="logo">🥬</div>
        <div className="title-block">
          <h1>Digital Pantry</h1>
          <div className="tagline">Live household food inventory</div>
        </div>
        <StatusPill />
      </header>

      <div className="toolbar">
        <span className="count">
          {/* key=count re-runs the "number pop-in" animation on every change */}
          <strong key={active.length} className="count-num">
            {active.length}
          </strong>{' '}
          active items
        </span>
        <FilterTabs active={filter} onChange={setFilter} />
        <button className="filter" title="Add a receipt photo" onClick={() => setIngestOpen(true)}>
          + Receipt
        </button>
      </div>

      {ingestOpen && <IngestPanel onClose={() => setIngestOpen(false)} />}

      <div className="items">
        {!isReady ? (
          // transitions.dev: "Skeleton loader and reveal"
          Array.from({ length: 6 }, (_, i) => <div key={i} className="skeleton" />)
        ) : sorted.length === 0 ? (
          <div className="empty">
            No active items. Add food via Discord and it will appear here in real time.
          </div>
        ) : (
          sorted.map((i) => <ItemCard key={i.name} item={i} />)
        )}
      </div>

      <footer>
        Powered by{' '}
        <a href="https://spacetimedb.com" target="_blank" rel="noreferrer">
          SpacetimeDB
        </a>{' '}
        · add receipts from the page or Discord
      </footer>
    </>
  );
}

export default function App() {
  const builder = DbConnection.builder()
    .withUri(HOST)
    .withDatabaseName(DB_NAME)
    .withToken(localStorage.getItem(TOKEN_KEY) ?? undefined);
  return (
    <SpacetimeDBProvider connectionBuilder={builder}>
      <Shell />
    </SpacetimeDBProvider>
  );
}
