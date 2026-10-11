"""Étape 0 — compare deux séries de masques (ex. bf16 vs fp32, tiny vs small) par IoU image à image.

Usage : .venvs/vision/bin/python stage0/scripts/compare_masks.py masks_tiny_mps masks_tiny_mps_bf16
"""
import json
import sys
from pathlib import Path

import cv2
import numpy as np

ROOT = Path(__file__).resolve().parents[2]
CACHE = ROOT / "cache" / "stage0"


def main(ref, other):
    out = []
    for clip_dir in sorted(CACHE.iterdir()):
        a, b = clip_dir / ref, clip_dir / other
        if not (a.exists() and b.exists()):
            continue
        ious = []
        for fa in sorted(a.glob("*.png")):
            ma = cv2.imread(str(fa), 0) > 0
            mb = cv2.imread(str(b / fa.name), 0) > 0
            union = np.logical_or(ma, mb).sum()
            ious.append(np.logical_and(ma, mb).sum() / union if union else 1.0)
        ious = np.array(ious)
        out.append({"clip": clip_dir.name, "ref": ref, "other": other,
                    "iou_mean": round(float(ious.mean()), 4), "iou_min": round(float(ious.min()), 4),
                    "frames_below_0.95": int((ious < 0.95).sum())})
        print(json.dumps(out[-1]))
    dest = ROOT / "stage0" / "results" / f"compare__{ref}__vs__{other}.json"
    dest.write_text(json.dumps(out, indent=2))


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
