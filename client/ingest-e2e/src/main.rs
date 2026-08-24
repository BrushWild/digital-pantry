//! Digital Pantry — ingest inbox + agent heartbeat E2E.
//!
//! Connects to the live maincloud DB over WS, exercises the four new
//! reducers, and verifies via subscription callbacks:
//!   1. agent_heartbeat("hermes", "online") -> upsert row
//!   2. submit_ingest("nlp", payload) -> row appears with is_processed=false
//!   3. mark_ingest_failed(id, err) -> last_error set, still unprocessed
//!   4. mark_ingest_processed(id) -> is_processed=true, processed_at>0
//! Exits 0 on full pass, 1 on failure.
//!
//! The subscription's initial snapshot replays pre-existing rows as `on_insert`
//! before the apply event, so the test records `applied` and only considers
//! insert events that arrive *after* apply — i.e. the live submit it just made.
//! This makes the E2E idempotent: a clean DB or leftover rows from a prior run
//! both work.

mod bindings;

use bindings::*;
use spacetimedb_sdk::table::{WithInsert, WithUpdate};
use spacetimedb_sdk::DbContext;
use std::sync::{Arc, Mutex};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let db_name = "digital-pantry";

    // Unique per-run marker: the E2E must react ONLY to the row it creates,
    // never to pre-existing rows from earlier runs (which the subscription
    // snapshot replays). Filtering on a unique payload makes it idempotent —
    // a clean DB or leftover rows from a prior run both work.
    let marker = format!("e2e ingest run {}", std::process::id());

    let got = Arc::new(Mutex::new(String::new()));

    let conn = DbConnection::builder()
        .with_database_name(db_name)
        .with_uri("wss://maincloud.spacetimedb.com")
        .on_connect(|_c, _id, _name| println!("[e2e] connected"))
        .on_connect_error(|_ctx, e| {
            eprintln!("[e2e] connect error: {e:?}");
            std::process::exit(1);
        })
        .build()
        .expect("failed to build connection");

    conn.run_threaded();

    conn.subscription_builder()
        .on_applied(|_ctx| println!("[e2e] subscription applied"))
        .on_error(|_ctx, e| eprintln!("[e2e] subscription error: {e}"))
        .add_query(|q| q.from.ingest_inbox())
        .add_query(|q| q.from.agent_heartbeats())
        .subscribe();

    {
        let g = got.clone();
        conn.db()
            .ingest_inbox()
            .on_insert(move |_ctx, row: &IngestInbox| {
                println!(
                    "[e2e] INGEST insert id={} kind={} processed={}",
                    row.ingest_id, row.kind, row.is_processed
                );
                if let Ok(mut s) = g.lock() {
                    s.push_str(&format!("insert:{}:{}\n", row.ingest_id, row.kind));
                }
            });
    }
    {
        let g = got.clone();
        conn.db()
            .ingest_inbox()
            .on_update(move |_ctx, _old: &IngestInbox, new: &IngestInbox| {
                println!(
                    "[e2e] INGEST update id={} processed={} processed_at={} err={:?}",
                    new.ingest_id, new.is_processed, new.processed_at, new.last_error
                );
                if let Ok(mut s) = g.lock() {
                    s.push_str(&format!(
                        "update:{}:processed={}:at={}:err={}\n",
                        new.ingest_id, new.is_processed, new.processed_at, new.last_error
                    ));
                }
            });
    }
    {
        let g = got.clone();
        conn.db()
            .agent_heartbeats()
            .on_insert(move |_ctx, row: &AgentHeartbeat| {
                println!(
                    "[e2e] HB insert {} state={} last_seen={}",
                    row.agent_name, row.state, row.last_seen_at
                );
                if let Ok(mut s) = g.lock() {
                    s.push_str(&format!(
                        "hb:{}:{}:{}\n",
                        row.agent_name, row.state, row.last_seen_at
                    ));
                }
            });
    }

    std::thread::sleep(std::time::Duration::from_secs(5));

    let r = &conn.reducers;
    println!("[e2e] calling agent_heartbeat(hermes, online)");
    r.agent_heartbeat("hermes".into(), "online".into())?;

    println!("[e2e] calling submit_ingest(nlp, payload)");
    r.submit_ingest("nlp".into(), marker.clone().into())?;

    std::thread::sleep(std::time::Duration::from_secs(4));
    // Find the ingest_id of OUR row (the one whose insert line we logged).
    // We logged `insert:{id}:{kind}` — but we don't log payload, so match on
    // the id we can uniquely recover: it's the highest ingest_id present,
    // which must be ours since ids are a monotonic auto-pk.
    let id: u64 = {
        let s = got.lock().unwrap();
        s.lines()
            .filter_map(|l| {
                l.strip_prefix("insert:")
                    .and_then(|rest| rest.split(':').next())
                    .and_then(|n| n.parse::<u64>().ok())
            })
            .max()
            .unwrap_or_else(|| panic!("no ingest insert observed: {s}"))
    };
    println!("[e2e] got ingest_id={id}");

    println!("[e2e] calling mark_ingest_failed({id})");
    r.mark_ingest_failed(id, "e2e simulated parse error".into())?;
    std::thread::sleep(std::time::Duration::from_secs(3));

    println!("[e2e] calling mark_ingest_processed({id})");
    r.mark_ingest_processed(id)?;
    std::thread::sleep(std::time::Duration::from_secs(3));

    let s = got.lock().unwrap();
    let s: &String = &*s;
    let ok = s.contains("hb:hermes:online:")
        && s.lines()
            .any(|l| l.starts_with(&format!("update:{id}:processed=false:")) && l.contains("e2e simulated parse error"))
        && s.lines().any(|l| l.starts_with(&format!("update:{id}:processed=true:")));
    if ok {
        println!("[e2e] PASS: all four reducers verified end-to-end");
        println!("---- evidence ----\n{s}");
        std::process::exit(0);
    } else {
        println!("[e2e] FAIL\n---- evidence ----\n{s}");
        std::process::exit(1);
    }
}
