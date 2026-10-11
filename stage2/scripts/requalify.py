"""Recalcule le contrôle qualité (workers/vision/maskqa.py) sur des dossiers de masques existants,
sans relancer SAM 2 — pour régler les seuils de détection.

Usage : .venvs/vision/bin/python stage2/scripts/requalify.py <dossier_masques> [...]
"""
import json
import sys
from pathlib import Path

import cv2

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "workers" / "vision"))
import maskqa  # noqa: E402


def requalify(d: Path):
    files = sorted(d.glob("*.png"), key=lambda f: int(f.stem))
    ledger = json.loads((d / "chunks.json").read_text()) if (d / "chunks.json").exists() else {}
    info = ledger.get("cleanup", {})
    stats, prev = [], None
    for f in files:
        m = cv2.imread(str(f), cv2.IMREAD_GRAYSCALE) > 127
        st = maskqa.frame_stats(int(f.stem), m, prev)
        removed, filled, area = info.get(f.stem.lstrip("0") or "0", [0, 0, 0])
        st.islands_removed, st.holes_filled = removed, filled
        st.islands_area_pct = round(area / max(int(m.sum()), 1) * 100, 3)
        stats.append(st)
        prev = m
    flagged, warnings = maskqa.split_persistent(stats, maskqa.detect_drift(stats))
    return maskqa.summarize(stats, flagged), flagged, warnings


if __name__ == "__main__":
    for arg in sys.argv[1:]:
        summary, flagged, warnings = requalify(Path(arg))
        for w in warnings:
            print(f"{arg}\n  AVERTISSEMENT CLIP : {w}")
        print(f"{arg}\n  {summary['frames']} images, {len(flagged)} signalées : "
              + ", ".join(f"{f['frame']} ({f['reasons'][0]})" for f in flagged[:6]) + (" …" if len(flagged) > 6 else ""))
