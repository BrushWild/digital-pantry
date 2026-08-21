#!/usr/bin/env python3
"""
barcode_lookup.py — Open Food Facts product lookup for the Digital Pantry.

Usage:
    barcode_lookup.py 3017620422003
    barcode_lookup.py 3017620422003 --fields code,product_name,quantity,image_url
    barcode_lookup.py 3017620422003 --nutriments

Output JSON: normalized product record ready to seed a Digital Pantry item.

Returns:
    { "found": true, "code": "...", "name": "...", "quantity": "...",
      "image_url": "...", "nutriments": {...}, "raw": {...} }
    { "found": false, "code": "...", "reason": "product not found",
      "suggestion": "fall back to NLP/manual entry" }
"""
import argparse
import json
import sys

API = "https://world.openfoodfacts.org/api/v2/product/{code}.json"

DEFAULT_FIELDS = ("code,product_name,product_name_en,quantity,brand,"
                  "image_url,expiration_date,packaging")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("code")
    ap.add_argument("--fields", default=DEFAULT_FIELDS)
    ap.add_argument("--nutriments", action="store_true")
    a = ap.parse_args()

    import requests
    url = API.format(code=a.code)
    r = requests.get(url, params={"fields": a.fields}, timeout=30,
                     headers={"User-Agent": "digital-pantry/0.1 (household food inventory)"})
    r.raise_for_status()
    j = r.json()

    if j.get("status") != 1:
        print(json.dumps({
            "found": False,
            "code": a.code,
            "reason": j.get("status_verbose", "not found"),
            "suggestion": "fall back to NLP/manual entry (waterfall tertiary path)",
        }, indent=2))
        return

    p = j["product"]
    out = {
        "found": True,
        "code": p.get("code"),
        "name": p.get("product_name_en") or p.get("product_name"),
        "brand": p.get("brand"),
        "quantity": p.get("quantity"),
        "image_url": p.get("image_url"),
        "expiration_date": p.get("expiration_date"),
        "categories": p.get("categories"),
    }
    if a.nutriments and "nutriments" in p:
        # keep only the most useful nutrition fields
        keep = {k: v for k, v in p["nutriments"].items()
                if isinstance(v, (int, float)) and k in (
                    "energy-kcal_100g", "proteins_100g", "carbohydrates_100g",
                    "fat_100g", "saturated-fat_100g", "sugars_100g",
                    "fiber_100g", "salt_100g", "sodium_100g")}
        out["nutriments_per_100g"] = keep
    print(json.dumps(out, indent=2, ensure_ascii=False))


if __name__ == "__main__":
    try:
        main()
    except Exception as e:
        print(json.dumps({"error": str(e)}), file=sys.stderr)
        sys.exit(1)
