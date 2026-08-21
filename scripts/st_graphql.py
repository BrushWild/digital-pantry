#!/usr/bin/env python3
"""
st_graphql.py — Minimal SpacetimeDB GraphQL client for the Digital Pantry.

The drain poller (digest_poller.py) is a request/response loop, not a
long-lived subscription, so the plain GraphQL HTTP endpoint is the lightest
possible transport — no generated client bindings, no WebSocket, no
autogen package. `requests` is already in the project venv.

Instance URL forms (either works):
    maincloud:   https://spacetimedb.com/@<owner>/<db>
    local:       http://localhost:3000
The GraphQL endpoint is <instance>/graphql.

Auth:
    SpacetimeDB accepts an `Authorization: Bearer <token>` header. Anonymous
    (no token) works for public tables and, by default, for reducers on a
    fresh maincloud db. Set SPACETIMEDB_AUTH_TOKEN if the db is locked down.

Field naming:
    SpacetimeDB's GraphQL layer uses snake_case field names matching the
    module's Rust field names, and table/reducer names are snake_case too.
    This helper deliberately does NOT camel-case — the live schema is the
    source of truth and we test against a real db in CI/e2e.
"""
from __future__ import annotations
import json
import os
from typing import Any, Dict, Iterable, List, Optional

import requests

DEFAULT_TIMEOUT_S = 15


def instance_url() -> str:
    """Base URL of the SpacetimeDB instance (no trailing slash, no /graphql)."""
    url = os.environ.get("SPACETIMEDB_INSTANCE", "").strip()
    if not url:
        raise SystemExit(
            "SPACETIMEDB_INSTANCE env var not set "
            "(e.g. https://spacetimedb.com/@brushwild/digital-pantry)"
        )
    return url.rstrip("/")


def graphql(
    query: str,
    variables: Optional[Dict[str, Any]] = None,
    *,
    instance: Optional[str] = None,
    token: Optional[str] = None,
    timeout: int = DEFAULT_TIMEOUT_S,
) -> Dict[str, Any]:
    """POST a GraphQL query. Returns the parsed JSON body (dict).

    Raises SystemExit on HTTP-level failure (non-2xx) or on a GraphQL
    `errors` array in the response — both are worth aborting the drain cycle
    rather than silently skipping a row.
    """
    base = instance or instance_url()
    endpoint = f"{base}/graphql"
    headers = {"Content-Type": "application/json"}
    tok = token if token is not None else os.environ.get("SPACETIMEDB_AUTH_TOKEN", "").strip()
    if tok:
        headers["Authorization"] = f"Bearer {tok}"
    r = requests.post(
        endpoint,
        headers=headers,
        data=json.dumps({"query": query, "variables": variables or {}}),
        timeout=timeout,
    )
    r.raise_for_status()
    body = r.json()
    if isinstance(body, dict) and body.get("errors"):
        raise SystemExit(f"GraphQL errors: {body['errors']}")
    return body


def query_table(table: str, columns: Iterable[str],
                where: Optional[Dict[str, Any]] = None,
                order_by: Optional[Dict[str, str]] = None,
                limit: Optional[int] = None,
                **kw: Any) -> List[Dict[str, Any]]:
    """Query a table's rows, returning a list of row dicts.

    Example:
        query_table("digest_outbox",
                    ["outbox_id", "subscription_id", "channel", "handle",
                     "message", "item_count", "is_delivered", "created_at"],
                    where={"is_delivered": {"eq": False}},
                    order_by={"outbox_id": "asc"})
    """
    cols = ",\n".join(columns)
    q = "query($where: RowFilter, $order: [OrderField!], $limit: Int) {\n"
    q += f"  {table}(where: $where, orderBy: $order, limit: $limit) "
    q += "{\n" + " " + f"{cols}\n  }}\n}}"
    variables: Dict[str, Any] = {
        "where": where,
        "order": [order_by] if isinstance(order_by, dict) else order_by,
        "limit": limit,
    }
    body = graphql(q, variables, **kw)
    data = body.get("data") or {}
    rows = data.get(table)
    return rows or []


def call_reducer(reducer: str, args: Dict[str, Any], **kw: Any) -> Dict[str, Any]:
    """Call a reducer by name and return the response payload.

    SpacetimeDB 1.x/2.x GraphQL exposes reducers as a top-level mutation
    with a `Reducer` input:
        mutation { <reducer>(args: {..}) { ok status } }
    """
    q = f'mutation($args: {reducer}Args) {{ {reducer}(args: $args) {{ ok status }} }}'
    body = graphql(q, {"args": args}, **kw)
    return body.get("data", {}).get(reducer, {})


if __name__ == "__main__":
    # Tiny self-test: connect + list tables via introspection.
    body = graphql('query { __schema { queryType { fields { name } } } }')
    fields = [f["name"] for f in body["data"]["__schema"]["queryType"]["fields"]]
    print(json.dumps({"instance": instance_url(),
                      "available_queries": fields}, indent=2))
