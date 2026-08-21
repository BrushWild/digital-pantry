# Digital Pantry — SpacetimeDB Schema

> **Status:** Draft v1 (compiles clean for `wasm32-unknown-unknown`; `spacetimedb = "2.1.0"` pin resolves to 2.8.2)
> **Source of truth:** `server/digital-pantry/spacetimedb/src/lib.rs`
> **Last updated:** 2026-08-21 — expiry sweep wired (scheduled `sweep_expiring_items`, 30-min loop)

This document explains *what* the schema contains and *why* each design choice
was made. The Rust module is the authoritative definition — if they disagree,
fix the doc.

---

## 1. Architecture recap

The pantry sits on SpacetimeDB as a **single shared module** ("household DB").
Every household member connects with their own auth token; SpacetimeDB's
`Identity` is their natural per-user key. The web UI (WASM client) and the
Hermes gateway both subscribe to the same public tables, so a change made in
either surface propagates to the other instantly — no sync layer needed.

```
┌─────────────┐   subscribe   ┌──────────────────────────┐
│  Web UI     │◄─────────────►│                          │
│ (WASM)      │   reducers    │   SpacetimeDB module     │
└─────────────┘               │   digital_pantry          │
┌─────────────┐   subscribe   │  ┌────────────────────┐  │
│  Hermes     │◄─────────────►│  │ 10 public tables   │  │
│  gateway    │   reducers    │  └────────────────────┘  │
│ (Discord /  │               │   19 reducers            │
│  Telegram)  │               └──────────────────────────┘
└─────────────┘
```

## 2. Tables (10)

| Table | PK | Role |
|---|---|---|
| `User` | `identity` | Household members; auto-created on first connect. |
| `Item` | `item_id` (auto) | One trackable unit of food. The core entity. |
| `Receipt` | `receipt_id` (auto) | A parsed grocery receipt. |
| `ReceiptItem` | `receipt_item_id` (auto) | Line items on a receipt. |
| `Recipe` | `recipe_id` (auto) | Stored recipe. |
| `RecipeIngredient` | `recipe_ingredient_id` (auto) | Ingredients per recipe (drives depletion + substitution). |
| `ShoppingListItem` | `shopping_item_id` (auto) | Reverse shopping list. |
| `DigestSubscription` | `subscription_id` (auto) | Per-endpoint weekly-digest subscriptions. |
| `PantryEvent` | `event_id` (auto) | Append-only audit trail + analytics source. |
| `ExpirySweepSchedule` | `scheduled_id` (auto) | SpacetimeDB timer table that drives the "expiring soon" sweep. |

### 2.1 `Item` — the core entity

One row = one **trackable physical unit**, not one SKU. Consequences:

- A 24-pack of eggs is **one row** with `quantity = 24`.
- A partially-used loaf is one row whose `quantity` is decremented as it's
  eaten. We deliberately do *not* split a loaf into rows — that would explode
  row counts for no benefit, and the UI can show "half a loaf" from the
  fraction.
- Buying more of an existing product **merges** into the existing row (see
  `add_item`): quantities add, earliest expiry wins. This keeps "milk" as one
  row instead of accumulating one row per shopping trip.

Fields grouped by concern:

- **Identity:** `name` (normalised lowercase, for matching, indexed),
  `display_name` (as entered), `barcode` (Open Food Facts code, indexed,
  empty = unknown).
- **Quantity:** `quantity: f64` + `unit: String`. `0 = depleted`. `f64`
  because we track partial quantities (0.5 L of milk).
- **Storage:** `location` (enum), `status` (enum).
- **Expiration:** `est_expiry_ts: i64` (Unix seconds, indexed, `0` = unknown),
  `unopened_days`, `opened_days` (i32, from the shelf-life skill cache).
- **Financial:** `price: f64`, `currency: String` (empty = household default).
- **Provenance:** `source_receipt_id` (0 = manual), `added_by: Identity`,
  `created_at` / `updated_at: Timestamp`.

**Why `i64` Unix seconds for `est_expiry_ts` instead of `Timestamp`?**
Expiry is *estimated* data (often "0 = unknown"), and we need arithmetic
(comparing, subtracting days) in reducers. `Timestamp` is a SpacetimeDB
type with a specific epoch semantics that doesn't want to hold "unknown".
`i64` with a documented sentinel (0) is simpler and portable to the WASM
client. `created_at`/`updated_at` use real `Timestamp` because they're
always set by the server clock.

### 2.2 `Receipt` / `ReceiptItem`

`Receipt` stores the store name, purchase time, totals, and — critically —
the **raw OCR text** and image URL. Keeping the raw text means we can
re-parse a receipt later when our NLP improves, without re-uploading.

`ReceiptItem` has `matched_item_id` (0 = not yet matched). The ingestion
waterfall is: OCR produces `ReceiptItem` rows → the agent fuzzy-matches each
against existing `Item`s → `match_receipt_item` links them → unmatched lines
become new `Item`s via `add_item`.

### 2.3 `Recipe` / `RecipeIngredient`

`accept_recipe` deducts every **non-optional** ingredient from the pantry.
Optional ingredients (`is_optional: true`) are skipped — they're garnish-ish
and shouldn't block a recipe. `substitute` holds a suggested alternative
name for the substitution engine.

### 2.4 `DigestSubscription` — multi-endpoint by design

A user can have **multiple rows**, one per endpoint (Discord thread +
Telegram, say). Primary key is an auto-increment `subscription_id`, *not*
`identity` — keying on identity alone would cap a user to one channel and
defeat the multi-endpoint requirement. Delivery is addressed by
`(channel, handle)`; `identity` records who created it.

`subscribe_digest` dedups on `(identity, channel, handle)`: re-subscribing
refreshes the existing row instead of creating a duplicate. This is the
"one copy per channel, never duplicates" guardrail from the design.

The weekly digest job reads **all** `is_active = true` rows and fans out.
Adding a new delivery channel is a data change (insert a row), not a deploy.

### 2.5 `PantryEvent`

Append-only log. Every meaningful mutation writes an event with an
`event_type` (indexed), optional `item_id`, a human-readable `description`,
the actor, and a timestamp. This powers the audit trail *and* is the
analytics source (waste rate, spending, what's actually getting eaten).
`log_event` is an internal helper (not a reducer) so reducers stay small.

### 2.6 `ExpirySweepSchedule` — the timer table

SpacetimeDB schedules reducers by watching a dedicated table. Each row is a
timer: when `scheduled_at` fires, the runtime calls `sweep_expiring_items`,
passing the row as its argument.

- `scheduled_id` — auto-increment id. The id **0** is the convention for a
  *repeating interval* row; a non-zero id is a one-shot that the runtime
  deletes after it fires.
- `scheduled_at` — a `ScheduleAt`. Built from a `std::time::Duration` for a
  repeating interval (`ScheduleAt::Interval`), or from a `Timestamp` for a
  one-shot (`ScheduleAt::Time`).

`init` inserts exactly one repeating row (every 30 minutes), guarded so a
re-run of `init` doesn't double-arm the loop. This is the whole "scheduler":
no external cron required for the sweep itself.

## 3. Enums

- **`Location`**: `Fridge`, `Freezer`, `Pantry`, `Counter`, `Other`.
  `Default = Pantry`. (Custom locations can be added by appending a variant —
  a breaking schema change, so keep it stable.)
- **`ItemStatus`**: `Unopened`, `Opened`, `ExpiringSoon`, `Depleted`.
  Derives `Ord` so "is this status more urgent than that one?" is a simple
  comparison (used by the digest to rank items). `ExpiringSoon` is set by the
  `sweep_expiring_items` scheduled reducer (§2.6), not by ingestion.

## 4. Reducers (19)

| Area | Reducer | Notes |
|---|---|---|
| init | `init` | Logs startup; arms the 30-min `ExpirySweepSchedule` loop (idempotent). |
| user | `client_connected` (init hook) | Auto-creates/activates `User` on connect. |
| | `set_user_name` | |
| items | `add_item` | Fuzzy-merge or create. The workhorse. |
| | `remove_item` | Hard delete + event. |
| | `deplete_item` | Sets qty 0, status Depleted. |
| | `update_item_quantity` | Auto-flips status to Depleted at 0 / Opened on first use. |
| | `update_item_location` | |
| receipts | `add_receipt` | |
| | `add_receipt_item` | Validates receipt exists. |
| | `match_receipt_item` | Links a line to an `Item`. |
| recipes | `add_recipe` | |
| | `add_recipe_ingredient` | |
| | `accept_recipe` | Deducts non-optional ingredients; logs missing ones. |
| shopping | `add_shopping_item` | |
| | `remove_shopping_item` | |
| digest | `subscribe_digest` | Upsert on (identity, channel, handle). |
| | `unsubscribe_digest` | Soft-disable by `subscription_id`. |
| expiry | `sweep_expiring_items` | **Scheduled.** 30-min loop; promotes items within the warn window to `ExpiringSoon` + logs. Also callable on demand. |

**Reducer return types:** In SpacetimeDB 2.x a reducer must return `()` or
`Result<(), E: Display>`. We use `Result<(), String>` everywhere for
readable error surfacing. Clients that need the created ID (e.g. after
`add_receipt`) read it from the public table subscription rather than the
return value — the table row is the source of truth and it's what other
clients already have via pub/sub.

## 5. Key design decisions

1. **Household = one DB, users = Identities.** Not one DB per household.
   Simplest mental model, and SpacetimeDB's auth gives us per-user identity
   for free. (Multi-*household* tenancy can be added later by scoping with a
   `household_id` column if needed.)
2. **Merge on add, not duplicate.** `add_item` finds an active row with the
   same normalised name and merges quantity + earliest expiry. Keeps the
   item list stable across many shopping trips.
3. **Expiry stored, not computed.** `est_expiry_ts` is written at ingestion
   (purchase date + shelf life). The `sweep_expiring_items` scheduled
   reducer (30-min loop, §2.6) promotes items to `ExpiringSoon` and the
   digest reads the column directly — no recompute per read.
4. **Digest subscriptions are data, not code.** Adding an endpoint = a row.
   The job fans out to every active `(channel, handle)`.
5. **Everything is public.** All tables are `public` (readable by any
   connected member of the household). Write access is gated by auth
   (any authenticated member can call reducers). A private household DB
   doesn't need per-row ACLs at v1.

## 6. Known limitations / next steps

- **No FK enforcement.** `matched_item_id`, `source_receipt_id`,
  `recipe_id` are plain `u64` columns. SpacetimeDB doesn't enforce
  foreign keys, so referential integrity is our responsibility in reducers
  (we validate existence where it matters).
- **`accept_recipe` is not atomic across a "partial success".** It deducts
  what it can and reports the rest. A reducer either commits fully or not
  at all, so "some ingredients deducted, some missing" is the committed
  state — acceptable, and the event log records what was missing.
- **Fuzzy matching is exact-match on `name` today.** True fuzzy matching
  (Levenshtein / token overlap) is the next upgrade to `add_item` and
  `accept_recipe` — the hook is there.
- **No soft-delete / history on `Item`.** Depletion sets status; hard
  delete is `remove_item`. A full item history can be derived from
  `PantryEvent` if needed.
- **Expiry sweep is wired, but notifications aren't yet.** The
  `sweep_expiring_items` scheduled reducer (30-min loop) promotes items to
  `ExpiringSoon` and logs a `PantryEvent`. The *next* glue is the digest
  job that reads those flagged items + active `DigestSubscription`s and
  actually fans out a message to each Discord/Telegram handle — the sweep
  marks the flag, the digest delivers it.

## 7. Build & verify

```bash
cd server/digital-pantry/spacetimedb
cargo build --target wasm32-unknown-unknown --release
# → target/wasm32-unknown-unknown/release/digital_pantry.wasm  (~370 KB)
```

Deploy (once a maincloud project exists):

```bash
spacetime module build
spacetime deploy --project digital-pantry
```

See the `spacetimedb-cli-deploy` skill for the full CLI flow.
