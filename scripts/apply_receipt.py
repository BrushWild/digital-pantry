#!/usr/bin/env python3
"""
apply_receipt.py — receipt image -> item rows (the OCR -> add_item leg).

Runs OCR (reusing ocr_receipt.py), parses the lines into item rows, and
emits JSON the ingest-poller feeds into `add_item` one row at a time.

Usage:
    apply_receipt.py IMAGE [--url URL] [--receipt-id N] [--no-preprocess]

    IMAGE   local path or data: URL (same contract as ocr_receipt.py)
    --receipt-id  ingest_inbox.ingest_id, echoed back as source_receipt_id

Output JSON:
    {
      "items": [
        {"name": "...", "display_name": "...", "quantity": 1.0, "unit": "",
         "location": "Fridge", "est_expiry_ts": 1760000000,
         "unopened_days": 14, "opened_days": 7, "price": 0.0,
         "currency": "", "barcode": "", "source_receipt_id": 9}
      ],
      "skipped": ["...lines that were not items..."],
      "raw_text": "..."
    }

Shelf life: cache-only lookup via shelf_life.read_cache (offline, no web —
web research is the LLM's job per shelf_life.py's own contract). Cache miss
=> est_expiry_ts 0 (the reducer treats 0 as "no expiry").
"""
import argparse
import importlib.util
import json
import re
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent


def _load(name: str):
    spec = importlib.util.spec_from_file_location(name, HERE / f"{name}.py")
    assert spec is not None and spec.loader is not None
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


ocr = _load("ocr_receipt")
shelf_life = _load("shelf_life")

# ponytail: store/brand headers and footer lines are heuristic (all-caps,
# keyword list, no price); real receipts from a specific store may need a
# per-store rule. Add when a household store proves out.
_SKIP_KEYWORDS = re.compile(
    r"(?i)^\s*(?:"
    r"subtotal|total|tax|balance|change|cash\b|card\b|paid|amount|due|"
    r"thank|customer|cashier|receipt|store|addr|phone|tel:|www\.|http|"
    r"no\.? sale|sale no|date:|time:|balance due|tip|discount"
    r")\b"
)
_PRICE_TAIL = re.compile(r"\s+[-$€£]?\s?[\d.,]+\s*$")
_DATELIKE = re.compile(r"^\s*\d{1,2}[\/\-]\d{1,2}[\/\-]\d{2,4}\s*$")
_TIME = re.compile(r"^\s*\d{1,2}:\d{2}[:\d]*\s*$")


def is_item_line(text: str) -> bool:
    t = text.strip()
    if not 2 <= len(t) <= 60 or not re.search(r"[A-Za-z]", t):
        return False
    if _SKIP_KEYWORDS.match(t) or _DATELIKE.match(t) or _TIME.match(t):
        return False
    if re.fullmatch(r"[\d\s\.,$€£\-:/%()#]*", t):  # pure numbers/prices
        return False
    # store headers: all-caps with a space ("GREEN GROCER"); "COKE" (1 word)
    # and mixed-case product names survive
    if re.search(r"[A-Z]", t) and not re.search(r"[a-z]", t) and " " in t:
        return False
    # address/phone line: a US phone number in any common shape
    if re.search(r"(\(\d{3}\)|\d{3}[-. ])\s?\d{3}[-. ]\d{4}", t):
        return False
    return True


def parse_items(ocr_out: dict, receipt_id: int) -> dict:
    items, skipped = [], []
    for ln in ocr_out.get("lines", []):
        text = (ln.get("text") or "").strip()
        if not text or not is_item_line(text):
            if text:
                skipped.append(text)
            continue
        m = _PRICE_TAIL.search(text)
        name = _PRICE_TAIL.sub("", text).strip() or text.strip()
        name = re.sub(r"\s{2,}", " ", name)
        if len(name) < 2:
            skipped.append(text)
            continue

        slug = shelf_life.slugify(name)
        cache = shelf_life.read_cache(slug)
        unopened = opened = 0
        location = "Other"
        if cache:
            d = cache["data"]
            unopened = int(d.get("unopened_days", 0) or 0)
            opened = int(d.get("opened_days", 0) or unopened)
            storage = (d.get("storage") or "").lower()
            loc = {"fridge": "Fridge", "freezer": "Freezer",
                   "pantry": "Pantry", "counter": "Counter"}.get(storage)
            if loc:
                location = loc
        est_expiry = int(time.time()) + unopened * 86400 if unopened > 0 else 0
        items.append({
            "name": name.lower(),
            "display_name": name,
            "quantity": 1.0,
            "unit": "",
            "location": location,
            "est_expiry_ts": est_expiry,
            "unopened_days": unopened,
            "opened_days": opened,
            "price": 0.0,
            "currency": "",
            "barcode": "",
            "source_receipt_id": receipt_id,
        })
    return {"items": items, "skipped": skipped, "raw_text": ocr_out.get("raw_text", "")}


def _selftest():
    # seed a deterministic cache entry under the EXACT slug parse_items will
    # compute (slugify of the display name), so the expiry assert is hermetic.
    seed_slug = shelf_life.slugify("Whole Milk 1L")
    shelf_life.write_cache(seed_slug, "Whole Milk 1L",
                          {"unopened_days": 14, "opened_days": 7,
                           "storage": "fridge"})
    fake = {"lines": [
        {"text": "GREEN GROCER"},              # all-caps header
        {"text": "123 Main St  (555) 010-2030"},  # address/phone line
        {"text": "Whole Milk 1L", "conf": 0.9},
        {"text": "Sourdough Bread 22oz", "conf": 0.9},
        {"text": "$4.99", "conf": 0.9},         # price-only
        {"text": "08/24/2026", "conf": 0.9},   # date
        {"text": "SUBTOTAL $12.40", "conf": 0.9},
        {"text": "TOTAL $12.40", "conf": 0.9},
        {"text": "THANK YOU", "conf": 0.9},
    ], "raw_text": ""}
    out = parse_items(fake, receipt_id=9)
    names = [i["name"] for i in out["items"]]
    assert names == ["whole milk 1l", "sourdough bread 22oz"], names
    assert all(i["source_receipt_id"] == 9 for i in out["items"])
    assert all(i["quantity"] == 1.0 for i in out["items"])
    # shelf-life cache hit (whole milk seeded above) => expiry + location set
    milk = next(i for i in out["items"] if i["name"] == "whole milk 1l")
    assert milk["est_expiry_ts"] > 0 and milk["unopened_days"] == 14, milk
    assert milk["location"] == "Fridge", milk
    # headers/footers/price/date lines must not become items
    assert len(out["skipped"]) >= 4, out["skipped"]
    print("selftest OK:", json.dumps(names))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("image", nargs="?", default=None)
    ap.add_argument("--url", default=None)
    ap.add_argument("--receipt-id", type=int, default=0)
    ap.add_argument("--no-preprocess", action="store_true")
    ap.add_argument("--selftest", action="store_true")
    a = ap.parse_args()

    if a.selftest:
        _selftest()
        return
    if not a.url and not a.image:
        ap.error("provide IMAGE or --url")

    img, ocr_path = ocr.load_image(a.image or "", a.url, a.no_preprocess)
    from rapidocr_onnxruntime import RapidOCR
    t0 = time.time()
    result, _ = RapidOCR()(ocr_path)
    dt = time.time() - t0
    lines = [{"text": t, "conf": round(float(c), 3)} for _b, t, c in (result or [])]
    ocr_out = {"image": a.image or a.url, "size": list(img.size),
               "ocr_seconds": round(dt, 2), "lines": lines,
               "raw_text": "\n".join(l["text"] for l in lines)}
    out = parse_items(ocr_out, a.receipt_id)
    out["ocr_seconds"] = round(dt, 2)
    print(json.dumps(out, ensure_ascii=False))


if __name__ == "__main__":
    try:
        main()
    except Exception as e:
        print(json.dumps({"error": str(e)}), file=sys.stderr)
        sys.exit(1)
