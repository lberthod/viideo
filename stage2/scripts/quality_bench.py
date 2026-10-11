"""Étape 2 — banc qualité : nettoyage du masque et détection de dérive sur les clips de référence.

Usage : .venvs/vision/bin/python stage2/scripts/quality_bench.py [--clips A_presenter ...]

Pilote le worker de l'app (workers/vision/worker.py) via son protocole JSON Lines, sur les proxies
de l'étape 0 (cache/stage0/<clip>/frames), avec les invites de stage0/clips.json :
  - propagation brute puis avec nettoyage ;
  - résumé qualité, images signalées et planche de contrôle des images signalées.
"""
import argparse
import json
import shutil
import subprocess
import sys
import time
from pathlib import Path

import cv2
import numpy as np

ROOT = Path(__file__).resolve().parents[2]
WORK = ROOT / "cache" / "stage2"
OUT = ROOT / "stage2" / "results"


class Worker:
    def __init__(self, root: Path):
        self.p = subprocess.Popen(
            [str(ROOT / ".venvs/vision/bin/python"), str(ROOT / "workers/vision/worker.py"),
             "--models-dir", str(ROOT / "models/sam2"), "--root", str(root)],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, bufsize=1)
        assert json.loads(self.p.stdout.readline())["event"] == "ready"
        self.n = 0

    def req(self, op, params=None):
        self.n += 1
        rid = f"q{self.n}"
        self.p.stdin.write(json.dumps({"id": rid, "op": op, "params": params or {}}) + "\n")
        self.p.stdin.flush()
        for line in self.p.stdout:
            m = json.loads(line)
            if m.get("id") == rid and "event" not in m:
                if not m["ok"]:
                    raise RuntimeError(m["error"])
                return m["result"]
        raise RuntimeError("worker arrêté")

    def close(self):
        self.p.stdin.write('{"id":"x","op":"shutdown"}\n')
        self.p.stdin.flush()
        self.p.wait(timeout=20)


def prompts_for(clip):
    return [{"frame": 0, "box": clip["box"], "points": [{"x": x, "y": y, "positive": True} for x, y in clip["points"]]}]


def sheet(frames_dir: Path, masks_dir: Path, flagged, dest: Path, title: str):
    """Planche : jusqu'à 8 images signalées (les plus graves), masque en rouge, avec l'image précédente."""
    top = sorted(flagged, key=lambda f: -f["score"])[:4]
    tiles = []
    for f in top:
        for idx in (max(f["frame"] - 1, 0), f["frame"]):
            img = cv2.imread(str(frames_dir / f"{idx:05d}.jpg"))
            m = cv2.imread(str(masks_dir / f"{idx:05d}.png"), 0)
            ov = img.copy()
            ov[m > 0] = (0.5 * ov[m > 0] + [0, 0, 127]).astype(np.uint8)
            cnts, _ = cv2.findContours(m, cv2.RETR_EXTERNAL, cv2.CHAIN_APPROX_SIMPLE)
            cv2.drawContours(ov, cnts, -1, (0, 255, 0), 2)
            label = f"#{idx}" + (" " + f["reasons"][0][:40] if idx == f["frame"] else " (avant)")
            cv2.putText(ov, label, (12, 40), 0, 1.0, (0, 255, 255), 3)
            h = 300
            tiles.append(cv2.resize(ov, (int(ov.shape[1] * h / ov.shape[0]), h)))
    if not tiles:
        return None
    w = max(t.shape[1] for t in tiles)
    tiles = [cv2.copyMakeBorder(t, 0, 0, 0, w - t.shape[1], cv2.BORDER_CONSTANT) for t in tiles]
    rows = [np.hstack(tiles[i:i + 2]) if i + 1 < len(tiles) else np.hstack([tiles[i], np.zeros_like(tiles[i])])
            for i in range(0, len(tiles), 2)]
    img = np.vstack(rows)
    cv2.putText(img, title, (12, img.shape[0] - 14), 0, 0.9, (255, 255, 255), 2)
    cv2.imwrite(str(dest), img, [cv2.IMWRITE_JPEG_QUALITY, 80])
    return dest


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--clips", nargs="*")
    ap.add_argument("--config", default="stage0/clips.json")
    args = ap.parse_args()
    cfg = json.loads((ROOT / args.config).read_text())
    clips = [c for c in cfg["clips"] if not args.clips or c["id"] in args.clips]
    OUT.mkdir(parents=True, exist_ok=True)
    WORK.mkdir(parents=True, exist_ok=True)
    w = Worker(WORK)
    w.req("load_model", {"model": "tiny", "dtype": "bf16"})
    results = []
    for clip in clips:
        src = ROOT / "cache" / "stage0" / clip["id"] / "frames"
        if not src.exists():
            print("proxy absent, ignoré :", clip["id"])
            continue
        frames = WORK / clip["id"] / "frames"
        if not frames.exists():
            shutil.copytree(src, frames)
        row = {"clip": clip["id"], "case": clip.get("case"), "frames": len(list(frames.glob("*.jpg")))}
        for variant, cleanup in (("raw", None), ("clean", {"min_island_ratio": 0.05, "max_hole_ratio": 0.002})):
            out = WORK / clip["id"] / f"masks_{variant}"
            t0 = time.perf_counter()
            r = w.req("propagate", {"frames_dir": str(frames), "out_dir": str(out), "prompts": prompts_for(clip),
                                    "cleanup": cleanup, "resume": False})
            q = json.loads((out / "quality.json").read_text())
            row[variant] = {"seconds": round(time.perf_counter() - t0, 1), "fps": r["fps"], **q["summary"],
                            "warnings": q.get("warnings", []),
                            "flagged_frames": [{"frame": f["frame"], "reasons": f["reasons"]} for f in q["flagged"]]}
            if variant == "clean" and q["flagged"]:
                dest = sheet(frames, out, q["flagged"], OUT / f"flagged_{clip['id']}.jpg", f"{clip['id']} : images signalées (nettoyage actif)")
                row["sheet"] = str(dest.relative_to(ROOT)) if dest else None
        results.append(row)
        c, r_ = row["clean"], row["raw"]
        print(f"{clip['id']:22} {row['frames']:4} img | brut: {r_['flagged']:3} signalées, iou {r_['iou_mean']:.3f}/{r_['iou_min']:.2f}"
              f" | nettoyé: {c['flagged']:3} signalées, {len(c['warnings'])} avert., {c['islands_removed']} îlots, {c['holes_filled']} trous, {c['fps']} i/s", flush=True)
    w.close()
    (OUT / "quality_bench.json").write_text(json.dumps(results, indent=2, ensure_ascii=False))


if __name__ == "__main__":
    sys.exit(main())
