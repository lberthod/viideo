"""Étape 0 — benchmark de segmentation vidéo SAM 2.1 sur les clips de référence.

Usage : .venvs/vision/bin/python stage0/scripts/sam2_bench.py --model tiny --device mps [--clips A_presenter ...]
        [--config stage0/clips_phase2.json] [--bf16] [--offload] [--offload-state]

Pour chaque clip : extrait un proxy JPEG (cache), initialise SAM 2 avec la boîte + points
de clips.json sur l'image 0, propage le masque, écrit les masques PNG et un JSON de mesures.
Un clip peut fixer sa propre durée ("seconds"), sinon "bench_seconds" global s'applique.
Les mesures sont aussi découpées par fenêtres de 100 images (vitesse, mémoire, stabilité du masque)
pour rendre visibles un ralentissement ou une dérive sur les clips longs.
"""
import argparse
import json
import os
import subprocess
import threading
import time
from pathlib import Path

os.environ.setdefault("PYTORCH_ENABLE_MPS_FALLBACK", "1")

import cv2
import numpy as np
import psutil
import torch

ROOT = Path(__file__).resolve().parents[2]
CACHE = ROOT / "cache" / "stage0"
RESULTS = ROOT / "stage0" / "results" / "sam2"
MODELS = {
    "tiny": ("configs/sam2.1/sam2.1_hiera_t.yaml", ROOT / "models/sam2/sam2.1_hiera_tiny.pt"),
    "small": ("configs/sam2.1/sam2.1_hiera_s.yaml", ROOT / "models/sam2/sam2.1_hiera_small.pt"),
}


class PeakMemory:
    """Échantillonne RSS du process et mémoire allouée par le driver MPS."""

    def __init__(self, interval=0.05):
        self.proc = psutil.Process()
        self.interval = interval
        self.peak_rss = 0
        self.peak_mps = 0
        self._stop = threading.Event()
        self._t = threading.Thread(target=self._run, daemon=True)

    def _run(self):
        while not self._stop.is_set():
            self.peak_rss = max(self.peak_rss, self.proc.memory_info().rss)
            if torch.backends.mps.is_available():
                self.peak_mps = max(self.peak_mps, torch.mps.driver_allocated_memory())
            time.sleep(self.interval)

    def __enter__(self):
        self._t.start()
        return self

    def __exit__(self, *exc):
        self._stop.set()
        self._t.join()


def extract_frames(clip, cfg):
    out = CACHE / clip["id"] / "frames"
    if out.exists() and any(out.iterdir()):
        return out
    out.mkdir(parents=True, exist_ok=True)
    src = ROOT / "clips" / "source" / clip["file"]
    subprocess.run(
        ["ffmpeg", "-v", "error", "-y", "-ss", str(clip["start"]), "-i", str(src),
         "-t", str(clip.get("seconds", cfg["bench_seconds"])), "-vf", f"scale=-2:{cfg['proxy_height']}",
         "-q:v", "2", "-start_number", "0", str(out / "%05d.jpg")],
        check=True,
    )
    return out


def mask_metrics(masks):
    areas = np.array([m.sum() for m in masks], dtype=np.float64)
    total = masks[0].size
    ious, comps = [], []
    for i, m in enumerate(masks):
        n, _ = cv2.connectedComponents(m.astype(np.uint8))
        comps.append(n - 1)
        if i:
            inter = np.logical_and(m, masks[i - 1]).sum()
            union = np.logical_or(m, masks[i - 1]).sum()
            ious.append(inter / union if union else 1.0)
    ious = np.array(ious)
    area_jumps = np.abs(np.diff(areas)) / np.maximum(areas[:-1], 1)
    return {
        "frames": len(masks),
        "empty_frames": int((areas == 0).sum()),
        "area_pct_mean": round(float(areas.mean() / total * 100), 2),
        "area_pct_min": round(float(areas.min() / total * 100), 2),
        "area_pct_max": round(float(areas.max() / total * 100), 2),
        "consecutive_iou_mean": round(float(ious.mean()), 4),
        "consecutive_iou_min": round(float(ious.min()), 4),
        "frames_iou_below_0.9": int((ious < 0.9).sum()),
        "area_jumps_over_10pct": int((area_jumps > 0.10).sum()),
        "components_mean": round(float(np.mean(comps)), 2),
        "components_max": int(max(comps)),
    }


WINDOW = 100


def window_stats(masks, times, mem_marks):
    """Vitesse et stabilité du masque par fenêtre de WINDOW images."""
    out = []
    total = masks[0].size
    for s in range(0, len(masks), WINDOW):
        e = min(s + WINDOW, len(masks))
        dt = times[e - 1] - (times[s - 1] if s else 0.0)
        ious = []
        for i in range(max(s, 1), e):
            u = np.logical_or(masks[i], masks[i - 1]).sum()
            ious.append(np.logical_and(masks[i], masks[i - 1]).sum() / u if u else 1.0)
        areas = [m.sum() / total * 100 for m in masks[s:e]]
        rss, mps = mem_marks[e - 1]
        out.append({"frames": f"{s}-{e - 1}", "fps": round((e - s) / dt, 2) if dt > 0 else None,
                    "consecutive_iou_mean": round(float(np.mean(ious)), 4) if ious else None,
                    "consecutive_iou_min": round(float(np.min(ious)), 4) if ious else None,
                    "area_pct_mean": round(float(np.mean(areas)), 2),
                    "empty_frames": int(sum(a == 0 for a in areas)),
                    "rss_gb_end": round(rss / 1e9, 2), "mps_driver_gb_end": round(mps / 1e9, 2)})
    return out


def run_clip(predictor, clip, cfg, model_name, device, tag="", offload_video=False, offload_state=False):
    frames_dir = extract_frames(clip, cfg)
    n_frames = len(list(frames_dir.glob("*.jpg")))
    mask_dir = CACHE / clip["id"] / f"masks_{model_name}_{device}{tag}"
    mask_dir.mkdir(parents=True, exist_ok=True)

    masks = []
    with PeakMemory() as mem:
        t0 = time.perf_counter()
        state = predictor.init_state(video_path=str(frames_dir), offload_video_to_cpu=offload_video,
                                     offload_state_to_cpu=offload_state)
        t_init = time.perf_counter() - t0

        t0 = time.perf_counter()
        pts = np.array(clip["points"], dtype=np.float32)
        predictor.add_new_points_or_box(
            inference_state=state, frame_idx=0, obj_id=1,
            points=pts, labels=np.ones(len(pts), dtype=np.int32),
            box=np.array(clip["box"], dtype=np.float32),
        )
        t_prompt = time.perf_counter() - t0

        times, mem_marks = [], []
        proc = psutil.Process()
        t0 = time.perf_counter()
        for idx, _, logits in predictor.propagate_in_video(state):
            m = (logits[0, 0] > 0).cpu().numpy()
            masks.append(m)
            cv2.imwrite(str(mask_dir / f"{idx:05d}.png"), (m * 255).astype(np.uint8))
            times.append(time.perf_counter() - t0)
            if (len(masks) % WINDOW == 0) or len(masks) == n_frames:
                mps_now = torch.mps.driver_allocated_memory() if device == "mps" else 0
                mem_marks.append((proc.memory_info().rss, mps_now))
            else:
                mem_marks.append(mem_marks[-1] if mem_marks else (0, 0))
        t_prop = time.perf_counter() - t0
        predictor.reset_state(state)

    h, w = masks[0].shape
    res = {
        "clip": clip["id"], "case": clip["case"], "model": f"sam2.1_hiera_{model_name}",
        "device": device, "variant": tag.lstrip("_") or "fp32",
        "offload_video_to_cpu": offload_video, "offload_state_to_cpu": offload_state, "resolution": f"{w}x{h}", "frames": n_frames,
        "time_init_s": round(t_init, 2), "time_prompt_s": round(t_prompt, 2),
        "time_propagate_s": round(t_prop, 2),
        "fps_propagate": round(len(masks) / t_prop, 2),
        "peak_rss_gb": round(mem.peak_rss / 1e9, 2),
        "peak_mps_driver_gb": round(mem.peak_mps / 1e9, 2),
        "mask_dir": str(mask_dir.relative_to(ROOT)),
        **mask_metrics(masks),
        "windows": window_stats(masks, times, mem_marks),
    }
    return res


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--model", choices=MODELS, default="tiny")
    ap.add_argument("--device", choices=["mps", "cpu"], default="mps")
    ap.add_argument("--clips", nargs="*")
    ap.add_argument("--bf16", action="store_true", help="autocast bfloat16 (MPS)")
    ap.add_argument("--config", default="stage0/clips.json", help="fichier de clips (relatif à la racine)")
    ap.add_argument("--offload", action="store_true", help="init_state(offload_video_to_cpu=True)")
    ap.add_argument("--offload-state", action="store_true", help="init_state(offload_state_to_cpu=True)")
    args = ap.parse_args()

    cfg = json.loads((ROOT / args.config).read_text())
    clips = [c for c in cfg["clips"] if not args.clips or c["id"] in args.clips]

    from sam2.build_sam import build_sam2_video_predictor

    config, ckpt = MODELS[args.model]
    t0 = time.perf_counter()
    predictor = build_sam2_video_predictor(config, str(ckpt), device=torch.device(args.device))
    t_load = time.perf_counter() - t0
    print(f"model {args.model} on {args.device}: loaded in {t_load:.1f}s", flush=True)

    RESULTS.mkdir(parents=True, exist_ok=True)
    for clip in clips:
        try:
            tag = ("_bf16" if args.bf16 else "") + ("_offload" if args.offload else "") \
                + ("_offstate" if args.offload_state else "")
            with torch.autocast(args.device, dtype=torch.bfloat16, enabled=args.bf16):
                res = run_clip(predictor, clip, cfg, args.model, args.device, tag,
                               offload_video=args.offload, offload_state=args.offload_state)
            res["time_model_load_s"] = round(t_load, 2)
        except Exception as e:  # consigner l'échec plutôt que d'arrêter le benchmark
            res = {"clip": clip["id"], "model": args.model, "device": args.device, "error": repr(e)}
        suffix = ("__bf16" if args.bf16 else "") + ("__offload" if args.offload else "") \
            + ("__offstate" if args.offload_state else "")
        out = RESULTS / f"{clip['id']}__{args.model}__{args.device}{suffix}.json"
        out.write_text(json.dumps(res, indent=2))
        print(json.dumps(res), flush=True)


if __name__ == "__main__":
    main()
