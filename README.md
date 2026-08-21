# digital-pantry

A custom single household digital pantry that leverages a local instance of the
Hermes agent, SpacetimeDB, GitHub pages/WASM interface, and multiple Hermes
gateway forms of communication (Telegram, Discord, etc.).

See `Foundation.md` in the Obsidian vault (`Digital Pantry` folder) for the
high-level architecture, and `Decisions & Research.md` for the running log of
decisions and research findings.

## Repo layout

```
digital-pantry/
├── requirements.txt        # pinned Python deps for scripts/
├── .venv/                  # local virtualenv (not committed)
└── scripts/
    ├── ocr_receipt.py      # receipt photo → structured text (RapidOCR/ONNX)
    ├── shelf_life.py       # shelf-life lookup + Obsidian cache
    └── barcode_lookup.py   # Open Food Facts barcode → product record
```

The Hermes skills that drive these scripts live in the Hermes skills directory
(`receipt-ocr`, `shelf-life-lookup`) and reference the absolute paths here.

## Quick start (scripts)

```bash
cd /opt/data/digital-pantry
python3 -m venv .venv
. .venv/bin/activate
pip install -r requirements.txt
```

### Receipt OCR

```bash
python scripts/ocr_receipt.py /path/to/receipt.jpg
python scripts/ocr_receipt.py dummy --url "https://example.com/receipt.jpg" --out /tmp/r.json
```

Outputs JSON with `lines[]` (text + confidence) and `raw_text`. The LLM "brain"
then does the semantic parse (items, quantities, prices, total).

### Shelf life

```bash
python scripts/shelf_life.py "whole milk"                       # cache lookup
python scripts/shelf_life.py "whole milk" --days 14 --opened-days 7 \
    --storage fridge --source "https://..." --confidence medium  # write cache
python scripts/shelf_life.py --list                             # list cached
```

Cache files are human-readable Obsidian notes under
`/opt/vault/Digital Pantry/ShelfLife/<slug>.md` and can be hand-edited.

### Barcode lookup

```bash
python scripts/barcode_lookup.py 3017620422003
python scripts/barcode_lookup.py 3017620422003 --nutriments
```

Uses Open Food Facts API v2. Unknown codes return `found: false` with a
suggestion to fall back to the NLP/manual entry path.

## Roadmap (current focus)

- [x] Receipt OCR engine validated (RapidOCR, ~3s/receipt on CPU)
- [x] Shelf-life lookup skill with Obsidian caching
- [x] Barcode lookup via Open Food Facts
- [ ] SpacetimeDB schema (Item, Receipt, Recipe, ShoppingList, User,
      DigestSubscription)
- [ ] SpacetimeDB deployment (maincloud free tier)
- [ ] WASM web client on GitHub Pages
- [ ] Discord/Telegram gateway wiring + weekly digest cron
- [ ] NLP ingestion skill (primary reliable path)
