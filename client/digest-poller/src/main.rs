//! Digital Pantry — digest drain poller.
//!
//! A long-lived SpacetimeDB *client*. It connects to the live `digital-pantry`
//! database over WebSocket via the official `spacetimedb-sdk`, subscribes to
//! `digest_outbox`, delivers each *pending* outbox row to its channel
//! (Discord/Telegram/WhatsApp webhook), then acks by invoking the
//! `mark_outbox_delivered` reducer.
//!
//! Delivery is idempotent per `outbox_id`: we only deliver rows that are not
//! yet `is_delivered`, and we ack exactly once per id.
//!
//! ## Run
//! ```sh
//! SPACETIMEDB_HOST=wss://maincloud.spacetimedb.com \
//! SPACETIMEDB_DB_NAME=digital-pantry \
//! DIGEST_POLLER_MODE=dry-run \
//! cargo run --release
//! ```

mod module_bindings;

use module_bindings::*;
use spacetimedb_sdk::table::WithInsert;
use spacetimedb_sdk::DbContext;
use std::env;
use std::sync::{Arc, Mutex};

/// outbox_ids we have already acked, so a re-delivered insert/update does not
/// double-post to a real channel within this process lifetime.
type Delivered = Mutex<Vec<u64>>;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let host = env::var("SPACETIMEDB_HOST").unwrap_or_else(|_| "wss://maincloud.spacetimedb.com".into());
    let db_name = env::var("SPACETIMEDB_DB_NAME").unwrap_or_else(|_| "digital-pantry".into());
    let live = env::var("DIGEST_POLLER_MODE").unwrap_or_else(|_| "dry-run".into());

    // Shared so the on_insert callback (which runs on the connection thread)
    // can track what we've already delivered.
    let delivered: Arc<Delivered> = Arc::new(Mutex::new(Vec::new()));

    let conn = DbConnection::builder()
        .with_database_name(db_name.clone())
        .with_uri(host.clone())
        .on_connect(|_ctx, _id, _db| {
            println!("[poller] connected");
        })
        .on_connect_error(|_ctx, e| {
            eprintln!("[poller] connection error: {e:?}");
            std::process::exit(1);
        })
        .build()
        .expect("failed to build connection");

    // Pump WebSocket messages on a background thread.
    conn.run_threaded();

    // Activate the subscription to digest_outbox. `on_insert` fires after this
    // is applied; without it the table callback is registered but never
    // triggered (this is the step the domino-vision reference uses).
    conn.subscription_builder()
        .on_applied(|_ctx| println!("[poller] digest_outbox subscription applied"))
        .on_error(|_ctx, e| eprintln!("[poller] digest_outbox subscription error: {e}"))
        .add_query(|q| q.from.digest_outbox())
        .subscribe();

    // Owned clones for the `move` closure (which runs on the connection thread
    // and must outlive `main`'s stack frame for the callback's lifetime).
    let live_owned = live.clone();
    let deliv = Arc::clone(&delivered);
    conn.db()
        .digest_outbox()
        .on_insert(move |ctx, row: &DigestOutbox| {
            if row.is_delivered {
                return; // already delivered by someone else / a prior run
            }

            // De-dupe within this process lifetime (only ids we already
            // delivered successfully are tracked, so failed rows can be
            // retried).
            {
                let g = deliv.lock().unwrap();
                if g.contains(&row.outbox_id) {
                    return;
                }
            }

            println!(
                "[poller] outbox_id={} channel={} handle={} items={} :: {}",
                row.outbox_id, row.channel, row.handle, row.item_count, row.message
            );

            let delivered = match live_owned.as_str() {
                "live" => match deliver(&row.channel, &row.handle, &row.message, row.item_count) {
                    Ok(()) => {
                        // Confirmed success — track it so a re-delivered insert
                        // does not double-post within this process lifetime.
                        deliv.lock().unwrap().push(row.outbox_id);
                        println!("[poller]   -> delivered to {} ({})", row.channel, row.handle);
                        true
                    }
                    Err(e) => {
                        // Do NOT ack on failure: the row stays pending and is
                        // re-delivered on the next subscription apply (WS
                        // reconnect / process restart).
                        eprintln!("[poller]   -> delivery failed ({e}); not acking");
                        false
                    }
                },
                _ => {
                    println!("[poller]   -> (dry-run) would deliver to {}", row.channel);
                    true
                }
            };

            // Ack: mark delivered on the server, log the completion. In live
            // mode this runs only after a confirmed delivery.
            if delivered {
                let ack_id = row.outbox_id;
                if let Err(e) = ctx.reducers().mark_outbox_delivered_then(ack_id, move |_rctx, res| {
                    match res {
                        Ok(Ok(())) => println!("[poller]   -> acked mark_outbox_delivered({ack_id})"),
                        Ok(Err(msg)) => eprintln!("[poller]   -> ack failed for {ack_id}: {msg}"),
                        Err(int_err) => eprintln!("[poller]   -> ack internal error for {ack_id}: {int_err:?}"),
                    }
                }) {
                    eprintln!("[poller]   -> failed to send ack for {ack_id}: {e:?}");
                }
            }
        });

    println!("[poller] mode={live}; listening for pending digest_outbox rows…");
    loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

/// Post the digest to the channel's webhook. Discord accepts `{"content":..}`;
/// Telegram/WhatsApp webhooks vary — generic JSON body for now, expanded per
/// channel as each is wired.
fn deliver(channel: &str, handle: &str, message: &str, item_count: u32) -> Result<(), Box<dyn std::error::Error>> {
    match channel {
        // Discord: the handle is either a webhook URL or a channel/thread ID.
        // When it's a bare ID (digits only), post via the bot token instead —
        // this avoids needing a pre-created webhook (the bot's guild role may
        // lack MANAGE_WEBHOOKS). Requires DISCORD_BOT_TOKEN in the env.
        "discord" => {
            deliver_discord(handle, message, item_count)?;
        }
        "telegram" => {
            let client = reqwest::blocking::Client::new();
            client
                .post(handle)
                .json(&serde_json::json!({ "text": format!("{message} ({item_count} items)") }))
                .send()
                .map_err(|e| Box::new(e))?;
        }
        "whatsapp" => {
            let client = reqwest::blocking::Client::new();
            client
                .post(handle)
                .json(&serde_json::json!({ "body": format!("{message} ({item_count} items)") }))
                .send()
                .map_err(|e| Box::new(e))?;
        }
        other => {
            eprintln!("[poller]   -> unknown channel '{other}'; skipping webhook");
        }
    }
    Ok(())
}

/// Discord delivery: webhook URL if the handle looks like a URL, else the bot
/// token posting to the channel/thread whose ID is the handle.
///
/// Returns `Err` on any non-2xx response — the caller must NOT ack a row whose
/// delivery failed (the outbox row stays pending for a retry).
fn deliver_discord(handle: &str, message: &str, item_count: u32) -> Result<(), Box<dyn std::error::Error>> {
    let body: serde_json::Value =
        serde_json::json!({ "content": format!("{message} ({item_count} items)") });
    let client = reqwest::blocking::Client::new();
    if let Ok(token) = env::var("DISCORD_BOT_TOKEN") {
        if let Ok(channel_id) = handle.parse::<u64>() {
            let url = format!("https://discord.com/api/v10/channels/{channel_id}/messages");
            let resp = client
                .post(&url)
                .header("Authorization", format!("Bot {token}"))
                .json(&body)
                .send()
                .map_err(|e| Box::new(e))?;
            let status = resp.status();
            if status.is_success() {
                Ok(())
            } else {
                let text = resp.text().unwrap_or_default();
                Err(Box::new(DeliverError(format!(
                    "discord bot-token post to {channel_id} returned {status}: {text}"
                ))))
            }
        } else {
            // Not a bare channel ID — treat as an explicit webhook URL.
            post_webhook(&client, handle, &body)
        }
    } else if let Ok(channel_id) = handle.parse::<u64>() {
        // Bare channel ID but no bot token in the env — cannot post.
        Err(Box::new(DeliverError(format!(
            "DISCORD_BOT_TOKEN not set; cannot post to channel {channel_id}"
        ))))
    } else {
        // Explicit webhook URL, no bot token needed.
        post_webhook(&client, handle, &body)
    }
}

fn post_webhook(
    client: &reqwest::blocking::Client,
    url: &str,
    body: &serde_json::Value,
) -> Result<(), Box<dyn std::error::Error>> {
    let resp = client.post(url).json(body).send().map_err(|e| Box::new(e))?;
    let status = resp.status();
    if status.is_success() {
        Ok(())
    } else {
        let text = resp.text().unwrap_or_default();
        Err(Box::new(DeliverError(format!(
            "webhook post to {url} returned {status}: {text}"
        ))))
    }
}

/// A delivery error carrying a human-readable description.
#[derive(Debug)]
struct DeliverError(String);

impl std::fmt::Display for DeliverError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for DeliverError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedup_tracks_ids() {
        let d: Arc<Delivered> = Arc::new(Mutex::new(Vec::new()));
        {
            let mut g = d.lock().unwrap();
            assert!(!g.contains(&42));
            g.push(42);
        }
        {
            let g = d.lock().unwrap();
            assert!(g.contains(&42));
        }
    }

    #[test]
    fn dry_run_mode_defaults() {
        // DIGEST_POLLER_MODE unset -> dry-run (no webhook side effects).
        let v = env::var("DIGEST_POLLER_MODE").unwrap_or_else(|_| "dry-run".into());
        assert_eq!(v, "dry-run");
    }
}
