#!/usr/bin/env python3
"""
ocr_receipt.py — Receipt OCR for the Digital Pantry.

Runs RapidOCR (ONNX) on a receipt photo and emits clean, structured text.
The *semantic* parsing (which lines are items, prices, totals) is done by the
Hermes "brain" (LLM) from this output — this script's job is robust text
extraction from a hard, noisy image.

Usage:
    ocr_receipt.py IMAGE [--url URL] [--out FILE] [--no-preprocess]

    IMAGE   local path to the receipt image (jpg/png)
    --url   if set, download from URL first and ignore IMAGE
    --out   write JSON to FILE instead of stdout
    --no-preprocess  skip light preprocessing (upscale/contrast)

Output JSON:
    {
      "image": "...",
      "size": [w, h],
      "ocr_seconds": 0.8,
      "lines": [ {"text": "...", "conf": 0.98}, ... ],   # reading order
      "raw_text": "line1\nline2\n..."
    }

Preprocessing (default on): images with the long side < 1200px are upscaled
2x; auto-contrast is applied. This meaningfully helps small/blurry photos.
"""
import argparse
import io
import json
import sys
import time
from pathlib import Path


def load_image(path: str, url: str | None, no_preprocess: bool):
    from PIL import Image, ImageOps, ImageEnhance

    if url:
        import requests
        r = requests.get(url, timeout=60, headers={"User-Agent": "Mozilla/5.0"})
        r.raise_for_status()
        img = Image.open(io.BytesIO(r.content))
    else:
        if not Path(path).exists():
            raise FileNotFoundError(path)
        img = Image.open(path)

    img = img.convert("RGB")
    w, h = img.size

    if not no_preprocess:
        long_side = max(w, h)
        if long_side < 1200:
            scale = max(1.0, 1200.0 / long_side)
            img = img.resize((int(w * scale), int(h * scale)), Image.LANCZOS)
        img = ImageOps.autocontrast(img)
        img = ImageEnhance.Contrast(img).enhance(1.15)
        img = ImageEnhance.Sharpness(img).enhance(1.4)
        # keep a temp path for OCR (rapidocr takes a path)
        tmp = Path("/tmp/_receipt_ocr_pre.jpg")
        img.save(tmp, quality=95)
        return img, str(tmp)

    img.save("/tmp/_receipt_ocr_raw.jpg", quality=95)
    return img, "/tmp/_receipt_ocr_raw.jpg"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("image", nargs="?", default=None)
    ap.add_argument("--url", default=None)
    ap.add_argument("--out", default=None)
    ap.add_argument("--no-preprocess", action="store_true")
    a = ap.parse_args()

    if not a.url and not a.image:
        ap.error("provide IMAGE or --url")

    t0 = time.time()
    img, ocr_path = load_image(a.image or "", a.url, a.no_preprocess)
    from rapidocr_onnxruntime import RapidOCR
    ocr = RapidOCR()
    result, _elapse = ocr(ocr_path)
    dt = time.time() - t0

    lines = []
    raw = []
    if result:
        # result: list of [bbox, text, confidence]
        for box, text, conf in result:
            lines.append({"text": text, "conf": round(float(conf), 3)})
            raw.append(text)
    out = {
        "image": a.image or a.url,
        "size": list(img.size),
        "ocr_seconds": round(dt, 2),
        "lines": lines,
        "raw_text": "\n".join(raw),
    }
    payload = json.dumps(out, ensure_ascii=False, indent=2)
    if a.out:
        Path(a.out).write_text(payload)
        print(f"wrote {a.out} ({len(lines)} lines, {dt:.1f}s)")
    else:
        print(payload)


if __name__ == "__main__":
    try:
        main()
    except Exception as e:
        print(json.dumps({"error": str(e)}), file=sys.stderr)
        sys.exit(1)
