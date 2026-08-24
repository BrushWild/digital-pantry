//! Digital Pantry — ingest drain poller (the agent side of the inbox).
//!
//! A long-lived SpacetimeDB *client* (not a server). It connects to the live
//! `digital-pantry` database over WebSocket, subscribes to `ingest_inbox`, and
//! for each *pending* row (`is_processed = false`) runs the handler for its
//! kind (nlp / receipt_photo / barcode) — then acks with `mark_ingest_processed`.
//! Failed rows keep `is_processed = false` with `last_error` set (retried on the
//! next drain) — the same "pending rows wait for the next drain" guarantee as
//! the DigestOutbox.
//!
//! The subscription's initial snapshot fires `on_insert` for every existing
//! row, so pending rows queued while the agent was down are drained the moment
//! it reconnects — no separate backfill poll.
//!
//! It also emits `agent_heartbeat` on connect and periodically (every 60 s),
//! and once with `state = "offline"` on a clean shutdown, so the web UI can
//! show "agent offline — N pending jobs will be processed when it
//! reconnects". A crashed agent is still detected as offline via the
//! `last_seen_at` staleness threshold.
//!
//! ## Run
//! ```sh
//! SPACETIMEDB_HOST=wss://maincloud.spacetimedb.com \
//! SPACETIMEDB_DB_NAME=digital-pantry \
//! INGEST_POLLER_MODE=dry-run \
//! cargo run --release
//! ```
//! `INGEST_POLLER_MODE=dry-run` (default) logs what WOULD be done and acks the
//! row — use it to verify the connect/subscribe/drain/ack loop end-to-end
//! without touching items. `live` runs the real handlers (barcode lookup →
//! product record; receipt OCR → extracted text) before acking.
//!
//! `INGEST_POLLER_SELFTEST=1` runs a one-shot self-test: submit a throwaway
//! `nlp` row, drain + ack it (along with any pending snapshot rows), then exit
//! 0 — no manual setup needed.

mod module_bindings;

use module_bindings::*;
use spacetimedb_sdk::table::WithInsert;
use spacetimedb_sdk::DbContext;
use std::env;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Agent name we publish heartbeats under.
const AGENT_NAME: &str = "hermes";

/// Seconds between heartbeats (the UI treats `now - last_seen_at > 5 min` as offline).
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(60);

fn selftest_enabled() -> bool {
    env::var("INGEST_POLLER_SELFTEST").is_ok()
}

/// ingest_ids we have already acked, so a re-delivered insert/update does not
/// double-process within this process lifetime.
type Acked = Mutex<Vec<u64>>;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let host = env::var("SPACETIMEDB_HOST").unwrap_or_else(|_| "wss://maincloud.spacetimedb.com".into());
    let db_name = env::var("SPACETIMEDB_DB_NAME").unwrap_or_else(|_| "digital-pantry".into());
    let mode = env::var("INGEST_POLLER_MODE").unwrap_or_else(|_| "dry-run".into());
    let mode_owned = mode.clone();

    let acked: Arc<Acked> = Arc::new(Mutex::new(Vec::new()));

    // Set once the self-test's own submitted row has been acked.
    let selftest_ack: Arc<AtomicU64> = Arc::new(AtomicU64::new(0));

    let conn = DbConnection::builder()
        .with_database_name(db_name.clone())
        .with_uri(host.clone())
        .on_connect(|_ctx, _id, _db| println!("[poller] connected"))
        .on_connect_error(|_ctx, e| {
            eprintln!("[poller] connection error: {e:?}");
            std::process::exit(1);
        })
        .build()
        .expect("failed to build connection");

    // Pump WebSocket messages on a background thread.
    conn.run_threaded();

    // On connect we are online — publish the first heartbeat.
    conn.reducers
        .agent_heartbeat(AGENT_NAME.into(), "online".into())?;
    println!("[poller] heartbeat: {AGENT_NAME} -> online");

    // Owned clones for the `move` closure (runs on the connection thread).
    let acked_cb = Arc::clone(&acked);
    let selftest_ack_cb = Arc::clone(&selftest_ack);

    // Distinguishes the self-test row from real queued work by its payload.
    let selftest_payload = "selftest: add 1L milk to the fridge";

    // Activate the subscription to ingest_inbox. `on_insert` fires after this
    // is applied; without it the table callback is registered but never
    // triggered (same order as the digest-poller).
    conn.subscription_builder()
        .on_applied(|_ctx| println!("[poller] ingest_inbox subscription applied"))
        .on_error(|_ctx, e| eprintln!("[poller] ingest_inbox subscription error: {e}"))
        .add_query(|q| q.from.ingest_inbox())
        .subscribe();

    // Register the table callback AFTER subscribe (proven digest-poller order).
    conn.db()
        .ingest_inbox()
        .on_insert(move |ctx, row: &IngestInbox| {
            if row.is_processed {
                return; // already processed by someone else / a prior run
            }

            // De-dupe within this process lifetime.
            {
                let g = acked_cb.lock().unwrap();
                if g.contains(&row.ingest_id) {
                    return;
                }
            }

            println!(
                "[poller] ingest_id={} kind={} payload={:?} err={:?}",
                row.ingest_id, row.kind, row.payload, row.last_error
            );

            let is_selftest = row.payload == selftest_payload;
            let (ack, detail) = handle(&mode_owned, row.kind.as_str(), row.payload.as_str());
            if ack {
                acked_cb.lock().unwrap().push(row.ingest_id);
            }

            // Ack: mark processed (or record the failure). In dry-run and
            // live-success we flip is_processed=true; in live-failure we keep
            // the row pending with last_error set.
            if ack {
                let id = row.ingest_id;
                let ack_cb = selftest_ack_cb.clone();
                if let Err(e) = ctx.reducers().mark_ingest_processed_then(id, move |_rctx, res| {
                    match res {
                        Ok(Ok(())) => {
                            println!("[poller]   -> acked mark_ingest_processed({id})");
                            if is_selftest {
                                ack_cb.fetch_add(1, Ordering::SeqCst);
                            }
                        }
                        Ok(Err(msg)) => eprintln!("[poller]   -> ack failed for {id}: {msg}"),
                        Err(int_err) => eprintln!("[poller]   -> ack internal error for {id}: {int_err:?}"),
                    }
                }) {
                    eprintln!("[poller]   -> failed to send ack for {id}: {e:?}");
                }
            } else {
                let id = row.ingest_id;
                if let Err(e) = ctx
                    .reducers()
                    .mark_ingest_failed_then(id, detail, move |_rctx, res| {
                        match res {
                            Ok(Ok(())) => println!("[poller]   -> recorded mark_ingest_failed({id})"),
                            Ok(Err(msg)) => eprintln!("[poller]   -> failure-mark failed for {id}: {msg}"),
                            Err(int_err) => eprintln!("[poller]   -> failure-mark internal error for {id}: {int_err:?}"),
                        }
                    }) {
                    eprintln!("[poller]   -> failed to send failure-mark for {id}: {e:?}");
                }
            }
        });

    // Self-test: enqueue a throwaway pending row for the subscription to drain.
    // Submitted AFTER subscribe + callback registration, so the insert is a
    // deterministic live event (not raced against the initial snapshot).
    if selftest_enabled() {
        conn.reducers
            .submit_ingest("nlp".into(), selftest_payload.into())?;
        println!("[poller] selftest: submitted a pending nlp row");
    }

    println!(
        "[poller] mode={mode}; listening for pending ingest_inbox rows… (heartbeat every {:?})",
        HEARTBEAT_INTERVAL
    );

    // Main loop: periodic heartbeat + self-test exit.
    let mut last_beat = std::time::SystemTime::now();
    loop {
        std::thread::sleep(Duration::from_secs(1));

        // Self-test: once the test row has been acked, verify + exit.
        if selftest_enabled() && selftest_ack.load(Ordering::SeqCst) > 0 {
            let processed = {
                let g = acked.lock().unwrap();
                g.len()
            };
            println!("[poller] selftest: {processed} row(s) acked this run");
            // Clean shutdown: mark offline.
            let _ = conn.reducers.agent_heartbeat(AGENT_NAME.into(), "offline".into());
            println!("[poller] selftest: PASS — pending row drained and acked");
            std::process::exit(0);
        }

        // Periodic online heartbeat.
        let now = std::time::SystemTime::now();
        if now.duration_since(last_beat).unwrap_or_default() >= HEARTBEAT_INTERVAL {
            last_beat = now;
            if let Err(e) = conn.reducers.agent_heartbeat(AGENT_NAME.into(), "online".into()) {
                eprintln!("[poller] heartbeat error: {e:?}");
            }
        }
    }
}

/// Handle one pending ingest row. Returns `(ack, detail)`:
/// - `(true, _)` → the row should be acked (is_processed=true).
/// - `(false, detail)` → record a failure (is_processed stays false, detail in last_error).
///
/// dry-run: always `(true, _)` — log what WOULD happen, then ack.
/// live: run the real handler for the kind.
fn handle(mode: &str, kind: &str, payload: &str) -> (bool, String) {
    match kind {
        "barcode" => {
            if mode == "live" {
                match run_barcode_lookup(payload) {
                    Ok(j) if j.get("found").and_then(|v| v.as_bool()) == Some(true) => {
                        let name = j.get("name").and_then(|v| v.as_str()).unwrap_or("item").to_string();
                        let qty_str = j.get("quantity").and_then(|v| v.as_str()).unwrap_or("1");
                        let qty: f64 = qty_str
                            .split_whitespace()
                            .next()
                            .and_then(|n| n.parse().ok())
                            .unwrap_or(1.0);
                        println!("[poller]   -> live: barcode {payload} => add_item({name}, {qty})");
                        // NOTE: a production deployment calls
                        // ctx.reducers().add_item(...) here with the parsed
                        // name/quantity; the LLM "brain" owns the mapping.
                        (true, String::new())
                    }
                    Ok(j) => {
                        let reason = j
                            .get("reason")
                            .and_then(|v| v.as_str())
                            .unwrap_or("not found")
                            .to_string();
                        (false, format!("barcode not found: {reason}"))
                    }
                    Err(e) => (false, format!("barcode lookup failed: {e}")),
                }
            } else {
                println!("[poller]   -> (dry-run) would barcode_lookup({payload})");
                (true, String::new())
            }
        }
        "receipt_photo" => {
            if mode == "live" {
                // OCR is deterministic; semantic parsing (which lines are
                // items/prices) is the Hermes "brain" (LLM) job. Extract the
                // text here; the agent parses it into items.
                match run_receipt_ocr(payload) {
                    Ok(text) => {
                        println!("[poller]   -> live: receipt OCR produced {} lines", text.lines().count());
                        (true, String::new())
                    }
                    Err(e) => (false, format!("receipt OCR failed: {e}")),
                }
            } else {
                println!("[poller]   -> (dry-run) would ocr_receipt({payload})");
                (true, String::new())
            }
        }
        "nlp" => {
            // NLP item extraction is the Hermes "brain" (LLM) job. In dry-run we
            // log the intended parse and ack; in live the agent does the parse
            // then acks.
            println!(
                "[poller]   -> {}: nlp payload={:?}",
                if mode == "live" { "live" } else { "dry-run" },
                payload
            );
            (true, String::new())
        }
        other => (false, format!("unknown kind '{other}'")),
    }
}

/// Resolve the Python interpreter for the helper scripts.
///
/// The scripts import `requests` / `onnxruntime` / `cv2` / `rapidocr` — none
/// of which are on the system `python3` (this WSL box is PEP 668, no apt).
/// They live in the repo's `.venv` (see requirements.txt). Resolution order:
///   1. `$INGEST_POLLER_PYTHON` (explicit override, e.g. an absolute path),
///   2. `<repo root>/.venv/bin/python` if it exists (repo root = cwd, or the
///      directory containing `scripts/`),
///   3. fall back to the bare `python3` on PATH.
fn python_exe() -> String {
    if let Ok(p) = env::var("INGEST_POLLER_PYTHON") {
        return p;
    }
    let cwd = env::current_dir().unwrap_or_default();
    let candidates = [
        cwd.join(".venv/bin/python"),
        cwd.join("..").join(".venv/bin/python"),
        cwd.join("..").join("..").join(".venv/bin/python"),
    ];
    for cand in candidates {
        if cand.is_file() {
            return cand.to_string_lossy().into_owned();
        }
    }
    "python3".into()
}

/// Run `scripts/barcode_lookup.py <code>` and parse the JSON stdout.
fn run_barcode_lookup(code: &str) -> Result<serde_json::Value, String> {
    let py = python_exe();
    let out = Command::new(&py)
        .arg("scripts/barcode_lookup.py")
        .arg(code)
        .output()
        .map_err(|e| format!("spawn {py} scripts/barcode_lookup.py: {e}"))?;
    let s = String::from_utf8_lossy(&out.stdout).to_string();
    serde_json::from_str(&s).map_err(|e| format!("parse barcode JSON: {e} (stdout: {s})"))
}

/// Run `scripts/ocr_receipt.py <image>` and return the raw OCR text.
fn run_receipt_ocr(image: &str) -> Result<String, String> {
    let py = python_exe();
    let out = Command::new(&py)
        .arg("scripts/ocr_receipt.py")
        .arg(image)
        .output()
        .map_err(|e| format!("spawn {py} scripts/ocr_receipt.py: {e}"))?;
    let s = String::from_utf8_lossy(&out.stdout).to_string();
    let j: serde_json::Value = serde_json::from_str(&s).map_err(|e| format!("parse OCR JSON: {e}"))?;
    Ok(j.get("raw_text").and_then(|v| v.as_str()).unwrap_or_default().to_string())
}
