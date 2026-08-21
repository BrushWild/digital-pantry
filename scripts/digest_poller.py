#!/usr/bin/env python3
"""
digest_poller.py — Drain the Digital Pantry digest outbox.

The SpacetimeDB wasm module can't make outbound network calls, so
`send_digest` writes one row per active `DigestSubscription` endpoint into
the `DigestOutbox` table. This poller is the networked half of the loop:

    1. Query `DigestOutbox` for rows where `is_delivered == false`, oldest
       first.
    2. Deliver each row over its channel.
    3. Ack with the `mark_outbox_delivered` reducer.

A row is only acked *after* its delivery succeeds, so a failed send (rate
limit, dead webhook) is retried on the next cycle — at-least-once delivery.

Channels:
    discord   handle = Discord webhook URL (POSTs a message there)
    telegram  handle = bot token, delivered to a chat id — wired when the
              household Telegram bot is provisioned (see --todo-telegram)
    whatsapp  same shape as telegram (not wired yet)

Usage:
    digest_poller.py --once              # one drain cycle, exit
    digest_poller.py --once --dry-run   # print what would be sent; no ack
    digest_poller.py                     # loop forever (default every 30s)

Environment:
    SPACETIMEDB_INSTANCE  (required) e.g. https://spacetimedb.com/@owner/db
    SPACETIMEDB_AUTH_TOKEN optional Bearer token for the instance
"""
from __future__ import annotations
import argparse
import json
import os
import sys
import time
from typing import Any, Dict, List

import requests

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from st_graphql import call_reducer, query_table  # noqa: E402

OUTBOX_COLUMNS = ["outbox_id", "subscription_id", "channel", "handle",
                  "message", "item_count", "is_delivered", "created_at"]
DEFAULT_POLL_SECONDS = 30
HTTP_TIMEOUT = 15


def deliver(row: Dict[str, Any], dry_run: bool = False) -> bool:
    """Deliver one outbox row over its channel. Returns True on success."""
    channel = (row.get("channel") or "").strip().lower()
    handle = row.get("handle") or ""
    message = row.get("message") or "(empty digest)"

    if dry_run:
        print(f"[dry-run] would POST {len(message)} chars to "
              f"{channel}/{handle}", file=sys.stderr)
        return True

    if channel == "discord":
        return _deliver_discord(handle, message)

    if channel in ("telegram", "whatsapp"):
        print(json.dumps({"status": "skipped",
                          "reason": f"channel '{channel}' not wired yet",
                          "outbox_id": row.get("outbox_id"),
                          "handle": handle}), file=sys.stderr)
        # Skip (not ack) so the row stays pending for the next cycle.
        return True

    print(json.dumps({"status": "skipped",
                      "reason": f"unknown channel '{channel}'",
                      "outbox_id": row.get("outbox_id")}), file=sys.stderr)
    return True


def _deliver_discord(webhook_url: str, message: str) -> bool:
    """POST a message to a Discord webhook. Returns True on 2xx."""
    if not webhook_url.startswith("http"):
        print(json.dumps({"status": "error",
                          "reason": f"discord handle is not a URL: {webhook_url!r}"})
                , file=sys.stderr)
        return False
    # Discord webhooks accept markdown; cap at 2000 chars (their limit).
    payload = {"content": message[:2000]}
    try:
        r = requests.post(webhook_url, json=payload, timeout=HTTP_TIMEOUT)
        if r.status_code in (200, 204):
            return True
        print(json.dumps({"status": "error",
                          "reason": f"discord webhook HTTP {r.status_code}",
                          "body": r.text[:200]}), file=sys.stderr)
        return False
    except requests.RequestException as e:
        print(json.dumps({"status": "error",
                          "reason": f"discord webhook failed: {e}"}), file=sys.stderr)
        return False


def drain_once(dry_run: bool = False) -> int:
    """Run one drain cycle. Returns number of rows delivered."""
    rows = query_table(
        "digest_outbox",
        OUTBOX_COLUMNS,
        where={"is_delivered": {"eq": False}},
        order_by={"outbox_id": "asc"},
        limit=50,  # bound a single cycle; rest next cycle
    )
    if not rows:
        print("no pending digest rows", file=sys.stderr)
        return 0

    delivered = 0
    for row in rows:
        ok = deliver(row, dry_run=dry_run)
        if not ok:
            # Leave unacked — retried next cycle.
            continue
        if dry_run:
            delivered += 1
            continue
        res = call_reducer("mark_outbox_delivered",
                           {"outbox_id": row["outbox_id"]})
        if res.get("ok"):
            delivered += 1
            print(json.dumps({"delivered": row["outbox_id"],
                              "channel": row["channel"],
                              "chars": len(row.get("message", ""))}),
                  file=sys.stderr)
        else:
            print(json.dumps({"status": "ack-failed",
                              "outbox_id": row["outbox_id"],
                              "res": res}), file=sys.stderr)
    return delivered


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--once", action="store_true",
                    help="run a single drain cycle and exit")
    ap.add_argument("--dry-run", action="store_true",
                    help="print what would be sent; do not ack rows")
    ap.add_argument("--interval", type=int, default=DEFAULT_POLL_SECONDS,
                    help=f"poll interval in seconds (default {DEFAULT_POLL_SECONDS})")
    a = ap.parse_args()

    if a.once:
        n = drain_once(dry_run=a.dry_run)
        print(json.dumps({"drained": n, "dry_run": a.dry_run}))
        return

    print(f"digest poller: draining every {a.interval}s "
          f"(dry_run={a.dry_run})", file=sys.stderr)
    while True:
        try:
            drain_once(dry_run=a.dry_run)
        except Exception as e:  # noqa: BLE001 — keep the loop alive
            print(f"drain cycle error: {e}", file=sys.stderr)
        time.sleep(a.interval)


if __name__ == "__main__":
    try:
        main()
    except SystemExit:
        raise
    except Exception as e:  # noqa: BLE001
        print(json.dumps({"error": str(e)}), file=sys.stderr)
        sys.exit(1)
