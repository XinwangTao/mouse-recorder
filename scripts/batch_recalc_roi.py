#!/usr/bin/env python3
"""Batch-recompute element ROI from template size + center (adaptive pad).

First principles (template match / sliding convolution):
  placements ≈ (ROI_W - W + 1) * (ROI_H - H + 1)
  ROI == image  → 1 placement → zero position tolerance
  pad must cover expected jitter δ, but stay below false-peak / cost blow-up

  pad = clamp(max(12, round(0.12 * min(W,H))), 12, 40)
"""
from __future__ import annotations

import argparse
import sqlite3
from pathlib import Path

try:
    from PIL import Image
except ImportError as e:
    raise SystemExit("pip install pillow") from e


FLOOR = 12
CAP = 40
REL = 0.12


def adaptive_roi_pad(iw: int, ih: int) -> int:
    m = max(1, min(iw, ih))
    rel = int(round(m * REL))
    return max(FLOOR, min(CAP, rel))


def image_rect_anchored(cx, cy, iw, ih, sw, sh):
    iw = max(1, min(iw, max(sw, 1)))
    ih = max(1, min(ih, max(sh, 1)))
    ix = max(0, min(cx - iw // 2, max(sw - iw, 0)))
    iy = max(0, min(cy - ih // 2, max(sh - ih, 0)))
    return ix, iy, iw, ih


def pad_roi(ix, iy, iw, ih, sw, sh, pad):
    bx = max(0, ix - pad)
    by = max(0, iy - pad)
    x2 = min(max(sw, ix + iw), ix + iw + pad)
    y2 = min(max(sh, iy + ih), iy + ih + pad)
    bw = max(iw, x2 - bx, 4)
    bh = max(ih, y2 - by, 4)
    return bx, by, bw, bh


def resolve_png(raw: str, search_dirs: list[Path]) -> Path | None:
    if not raw:
        return None
    p = Path(raw)
    if p.exists():
        return p
    name = p.name
    for d in search_dirs:
        cand = d / name
        if cand.exists():
            return cand
    return None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument(
        "--db",
        default=str(Path(__file__).resolve().parents[1] / "data" / "mouse_recorder.db"),
    )
    ap.add_argument("--dry-run", action="store_true")
    args = ap.parse_args()

    db = Path(args.db)
    if not db.exists():
        raise SystemExit(f"DB not found: {db}")

    search_dirs = [
        db.parent,
        Path(__file__).resolve().parents[1] / "data",
    ]

    conn = sqlite3.connect(str(db))
    rows = conn.execute(
        """
        SELECT e.id, e.name, e.center_x, e.center_y, e.bbox_x, e.bbox_y,
               e.bbox_width, e.bbox_height, e.screen_width, e.screen_height,
               (SELECT image_path FROM element_states
                WHERE element_id=e.id ORDER BY is_primary DESC, id LIMIT 1)
        FROM elements e ORDER BY e.id
        """
    ).fetchall()

    updated = 0
    skipped = 0
    print(f"DB={db}  elements={len(rows)}  dry_run={args.dry_run}")
    print(f"pad = clamp(max({FLOOR}, 0.12*min(W,H)), {FLOOR}, {CAP})")
    print("-" * 88)

    for r in rows:
        eid, name, cx, cy, obx, oby, obw, obh, sw, sh, img = r
        path = resolve_png(img or "", search_dirs)
        if not path:
            print(f"#{eid:2d} {name:20s} SKIP missing image: {img}")
            skipped += 1
            continue
        with Image.open(path) as im:
            iw, ih = im.size
        pad = adaptive_roi_pad(iw, ih)
        ix, iy, iw, ih = image_rect_anchored(cx, cy, iw, ih, sw, sh)
        bx, by, bw, bh = pad_roi(ix, iy, iw, ih, sw, sh, pad)
        placements = max(1, bw - iw + 1) * max(1, bh - ih + 1)
        print(
            f"#{eid:2d} {name:20s} img={iw:4d}x{ih:<4d} pad={pad:2d}  "
            f"roi {obw:4d}x{obh:<4d} -> {bw:4d}x{bh:<4d}  "
            f"search≈{placements}  @({bx},{by})"
        )
        if not args.dry_run:
            conn.execute(
                """UPDATE elements SET bbox_x=?, bbox_y=?, bbox_width=?, bbox_height=?
                   WHERE id=?""",
                (bx, by, bw, bh, eid),
            )
        updated += 1

    if not args.dry_run:
        conn.commit()
    conn.close()
    print("-" * 88)
    print(f"done: updated={updated} skipped={skipped}")


if __name__ == "__main__":
    main()
