"""Étape 0 — test de génération d'image locale avec un modèle déjà présent dans le cache HF.

Usage : .venvs/image/bin/python stage0/scripts/image_bench.py
Hors ligne (HF_HUB_OFFLINE=1) : aucun téléchargement. Modèle : FLUX.2 [klein] 4B via diffusers, MPS.
"""
import json
import os
import threading
import time
from pathlib import Path

os.environ["HF_HUB_OFFLINE"] = "1"
os.environ.setdefault("PYTORCH_ENABLE_MPS_FALLBACK", "1")

import psutil
import torch
from diffusers import Flux2KleinPipeline

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "stage0" / "results" / "backgrounds"
MODEL = "black-forest-labs/FLUX.2-klein-4B"
PROMPTS = {
    "studio_loft": "wide photo of a modern bright loft interior, large windows, soft daylight, "
                   "empty space in the center, shallow depth of field, realistic, 35mm",
    "mountain_terrace": "wide photo of a wooden terrace overlooking snowy alpine mountains at golden hour, "
                        "empty foreground, realistic photography",
}


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    proc = psutil.Process()
    peak = {"rss": 0, "mps": 0}
    stop = threading.Event()

    def sample():
        while not stop.is_set():
            peak["rss"] = max(peak["rss"], proc.memory_info().rss)
            peak["mps"] = max(peak["mps"], torch.mps.driver_allocated_memory())
            time.sleep(0.1)

    threading.Thread(target=sample, daemon=True).start()
    t0 = time.perf_counter()
    pipe = Flux2KleinPipeline.from_pretrained(MODEL, torch_dtype=torch.bfloat16).to("mps")
    res = {"model": MODEL, "device": "mps", "dtype": "bfloat16", "load_s": round(time.perf_counter() - t0, 1),
           "images": []}
    print("loaded", res["load_s"], flush=True)

    for i, (name, prompt) in enumerate(PROMPTS.items()):
        for w, h, steps in ((1280, 720, 4),) if i else ((768, 432, 4), (1280, 720, 4)):
            t0 = time.perf_counter()
            img = pipe(prompt=prompt, width=w, height=h, num_inference_steps=steps,
                       generator=torch.Generator("mps").manual_seed(42)).images[0]
            dt = time.perf_counter() - t0
            path = OUT / f"{name}_{w}x{h}.png"
            img.save(path)
            res["images"].append({"name": name, "size": f"{w}x{h}", "steps": steps, "seconds": round(dt, 1),
                                  "file": str(path.relative_to(ROOT))})
            print(res["images"][-1], flush=True)

    stop.set()
    res["peak_rss_gb"] = round(peak["rss"] / 1e9, 1)
    res["peak_mps_driver_gb"] = round(peak["mps"] / 1e9, 1)
    (ROOT / "stage0" / "results" / "image_bench.json").write_text(json.dumps(res, indent=2))
    print(json.dumps(res, indent=2))


if __name__ == "__main__":
    main()
