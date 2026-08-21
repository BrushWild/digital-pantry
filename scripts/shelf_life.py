#!/usr/bin/env python3
"""
shelf_life.py — Shelf-life lookup for the Digital Pantry.

Probes the internet for an item's shelf life, then caches the result in an
Obsidian memory file so it is only looked up once per item.

Cache location: /opt/vault/Digital Pantry/ShelfLife/<slug>.md
Each cache file is a small, human-readable note the household can hand-edit:

    ---
    item: whole milk
    slug: whole-milk
    unopened_days: 14
    opened_days: 7
    storage: fridge
    source: https://www.usda.gov/...
    confidence: high|medium|low
    last_verified: 2026-08-20
    ---
    # Whole Milk
    Notes: raw/unpasteurized is shorter; store at back of fridge, not door.

Usage:
    shelf_life.py "whole milk"                 # lookup (cache or web)
    shelf_life.py "whole milk" --refresh      # force web re-lookup
    shelf_life.py "whole milk" --storage fridge
    shelf_life.py --list                       # list cached items
    shelf_life.py "whole milk" --show          # print cached note only

If --source-url is provided, the script fetches that URL and includes a text
snippet for the LLM to extract from; otherwise the LLM does the web search
first and passes --days/--opened-days explicitly.
"""
import argparse
import json
import re
import sys
from datetime import date
from pathlib import Path

VAULT = Path("/opt/vault/Digital Pantry/ShelfLife")


def slugify(s: str) -> str:
    s = s.lower().strip()
    s = re.sub(r"[^a-z0-9]+", "-", s)
    return s.strip("-")


def parse_frontmatter(text: str) -> dict:
    m = re.match(r"^---\n(.*?)\n---", text, re.DOTALL)
    if not m:
        return {}
    d = {}
    for line in m.group(1).splitlines():
        if ":" in line:
            k, v = line.split(":", 1)
            d[k.strip()] = v.strip()
    return d


def read_cache(slug: str):
    p = VAULT / f"{slug}.md"
    if not p.exists():
        return None
    text = p.read_text()
    return {"path": str(p), "data": parse_frontmatter(text), "text": text}


def write_cache(slug: str, item: str, data: dict, notes: str = ""):
    VAULT.mkdir(parents=True, exist_ok=True)
    p = VAULT / f"{slug}.md"
    fm = []
    for k in ["item", "slug", "unopened_days", "opened_days", "storage",
              "source", "confidence", "last_verified"]:
        if k in data:
            fm.append(f"{k}: {data[k]}")
    body = f"---\n" + "\n".join(fm) + "\n---\n\n"
    body += f"# {item}\n"
    if notes:
        body += f"\nNotes: {notes}\n"
    p.write_text(body)
    return str(p)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("item", nargs="?", default=None)
    ap.add_argument("--refresh", action="store_true")
    ap.add_argument("--storage", default=None)
    ap.add_argument("--days", type=int, default=None)
    ap.add_argument("--opened-days", type=int, default=None)
    ap.add_argument("--source", default=None)
    ap.add_argument("--confidence", default="medium",
                    choices=["high", "medium", "low"])
    ap.add_argument("--notes", default="")
    ap.add_argument("--show", action="store_true")
    ap.add_argument("--list", action="store_true")
    a = ap.parse_args()

    if a.list:
        items = sorted(VAULT.glob("*.md")) if VAULT.exists() else []
        print(json.dumps([{"file": p.name, **parse_frontmatter(p.read_text())}
                          for p in items], indent=2))
        return

    if not a.item:
        ap.error("item required (or use --list)")
    slug = slugify(a.item)

    # --- read from cache first ---
    if not a.refresh:
        cached = read_cache(slug)
        if cached and not a.days:
            print(json.dumps({"cache": "hit", "path": cached["path"],
                              "data": cached["data"]}, indent=2))
            return

    # --- if LLM supplied explicit values, write cache ---
    if a.days is not None:
        data = {
            "item": a.item,
            "slug": slug,
            "unopened_days": a.days,
            "opened_days": a.opened_days if a.opened_days is not None else a.days,
            "storage": a.storage or "fridge",
            "source": a.source or "llm-estimate",
            "confidence": a.confidence,
            "last_verified": date.today().isoformat(),
        }
        p = write_cache(slug, a.item, data, a.notes)
        print(json.dumps({"cache": "written", "path": p, "data": data}, indent=2))
        return

    # --- otherwise: emit a "needs lookup" payload for the LLM to research ---
    hint_sources = [
        "USDA food storage: https://www.fsis.usda.gov/food-safety/safe-food-handling/storage-temperatures",
        "Stilltasty shelf life: https://www.stilltasty.com/shelf-life",
    ]
    print(json.dumps({
        "cache": "miss" if not a.refresh else "refresh",
        "item": a.item,
        "slug": slug,
        "hint_sources": hint_sources,
        "instruction": ("Research the shelf life, then re-run with "
                        "--days <unopened> --opened-days <opened> "
                        "--storage <fridge|pantry|freezer> "
                        "--source <url> --confidence <level>"),
    }, indent=2))


if __name__ == "__main__":
    try:
        main()
    except Exception as e:
        print(json.dumps({"error": str(e)}), file=sys.stderr)
        sys.exit(1)
