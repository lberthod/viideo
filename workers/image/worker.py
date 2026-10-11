"""Worker image videeo : génération de décors fixes, en JSON Lines sur stdin/stdout.

Même protocole que le worker vision (schema_version 1) :
  requête {"id", "op", "params"} ; réponse {"id", "ok", "result" | "error"} ;
  événement {"id", "event": "progress", "done", "total"}.

Opérations : ping, load_model, generate, cancel, unload, shutdown.
Modèle : FLUX.2 [klein] 4B (Apache-2.0) depuis le cache Hugging Face, hors ligne (aucun téléchargement).
Le worker n'écrit que sous --root et n'exécute aucune commande shell.
"""
import argparse
import json
import os
import queue
import sys
import threading
import time
import traceback
from pathlib import Path

SCHEMA_VERSION = 1
PROTO_OUT = sys.stdout
sys.stdout = sys.stderr

os.environ["HF_HUB_OFFLINE"] = "1"
os.environ.setdefault("PYTORCH_ENABLE_MPS_FALLBACK", "1")

MODELS = {
    "flux2-klein-4b": {"repo": "black-forest-labs/FLUX.2-klein-4B", "license": "Apache-2.0", "default_steps": 4},
}

_lock = threading.Lock()


def send(msg):
    with _lock:
        PROTO_OUT.write(json.dumps(msg) + "\n")
        PROTO_OUT.flush()


class Cancelled(Exception):
    pass


class ImageWorker:
    def __init__(self, root: Path):
        self.root = root.resolve()
        self.pipe = None
        self.model = None
        self.cancel = threading.Event()

    def _path(self, p):
        path = Path(p).resolve()
        if self.root not in path.parents:
            raise ValueError(f"chemin hors du répertoire autorisé : {path}")
        return path

    def op_ping(self, _):
        import torch
        return {"schema_version": SCHEMA_VERSION, "pid": os.getpid(), "torch": torch.__version__,
                "mps": torch.backends.mps.is_available(), "model": self.model, "models": MODELS}

    def op_load_model(self, p):
        import torch
        from diffusers import Flux2KleinPipeline
        name = p.get("model", "flux2-klein-4b")
        if name not in MODELS:
            raise ValueError(f"modèle inconnu : {name}")
        if self.model == name:
            return {"model": name, "cached": True}
        self.op_unload({})
        t0 = time.perf_counter()
        try:
            self.pipe = Flux2KleinPipeline.from_pretrained(MODELS[name]["repo"], torch_dtype=torch.bfloat16).to("mps")
        except OSError as e:
            raise RuntimeError(f"Modèle {MODELS[name]['repo']} absent ou incomplet dans le cache Hugging Face : {e}")
        self.model = name
        return {"model": name, "load_s": round(time.perf_counter() - t0, 1), "license": MODELS[name]["license"]}

    def op_generate(self, p, req_id):
        """Génère une image et l'écrit en PNG (+ métadonnées JSON à côté, pour la reproductibilité)."""
        # Validation avant tout chargement : inutile de charger 24 Go pour une requête invalide.
        prompt = str(p["prompt"]).strip()
        if not prompt or len(prompt) > 2000:
            raise ValueError("prompt vide ou trop long")
        w, h = int(p["width"]), int(p["height"])
        if not (256 <= w <= 2048 and 256 <= h <= 2048 and w % 16 == 0 and h % 16 == 0):
            raise ValueError("dimensions invalides (256–2048, multiples de 16)")
        if not 1 <= int(p.get("steps", 4)) <= 50:
            raise ValueError("nombre d'étapes invalide")
        seed = int(p.get("seed", 0)) & 0xFFFFFFFF
        out = self._path(p["out_png"])
        import torch
        if self.pipe is None:
            self.op_load_model({"model": p.get("model", "flux2-klein-4b")})
        steps = int(p.get("steps", MODELS[self.model]["default_steps"]))
        out.parent.mkdir(parents=True, exist_ok=True)
        self.cancel.clear()

        def on_step(pipe, i, t, kwargs):
            if self.cancel.is_set():
                raise Cancelled()  # interrompt la boucle de débruitage sans finir les étapes restantes
            send({"id": req_id, "event": "progress", "done": i + 1, "total": steps})
            return kwargs

        t0 = time.perf_counter()
        result = self.pipe(prompt=prompt, width=w, height=h, num_inference_steps=steps,
                           generator=torch.Generator("mps").manual_seed(seed), callback_on_step_end=on_step)
        if self.cancel.is_set():
            raise Cancelled()
        img = result.images[0]
        tmp = out.with_suffix(".tmp.png")
        img.save(tmp)
        tmp.replace(out)
        meta = {"backend": "builtin", "model": self.model, "repo": MODELS[self.model]["repo"],
                "license": MODELS[self.model]["license"], "prompt": prompt, "width": w, "height": h,
                "steps": steps, "seed": seed, "seconds": round(time.perf_counter() - t0, 1)}
        out.with_suffix(".json").write_text(json.dumps(meta, ensure_ascii=False, indent=2))
        return {"out_png": str(out), **meta}

    def op_unload(self, _):
        self.pipe = None
        self.model = None
        import gc
        gc.collect()
        try:
            import torch
            torch.mps.empty_cache()
        except Exception:
            pass
        return {}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--root", required=True)
    args = ap.parse_args()
    worker = ImageWorker(Path(args.root))
    inbox = queue.Queue()

    def reader():
        for line in sys.stdin:
            line = line.strip()
            if not line:
                continue
            try:
                msg = json.loads(line)
            except json.JSONDecodeError as e:
                send({"id": None, "ok": False, "error": f"JSON invalide : {e}"})
                continue
            if msg.get("op") == "cancel":
                worker.cancel.set()
                send({"id": msg.get("id"), "ok": True, "result": {}})
                continue
            inbox.put(msg)
        inbox.put(None)

    threading.Thread(target=reader, daemon=True).start()
    send({"id": None, "event": "ready", "schema_version": SCHEMA_VERSION})
    ops = {"ping": worker.op_ping, "load_model": worker.op_load_model, "unload": worker.op_unload}
    while True:
        msg = inbox.get()
        if msg is None or msg.get("op") == "shutdown":
            break
        rid, op, params = msg.get("id"), msg.get("op"), msg.get("params") or {}
        try:
            if op == "generate":
                result = worker.op_generate(params, rid)
            elif op in ops:
                result = ops[op](params)
            else:
                raise ValueError(f"opération inconnue : {op}")
            send({"id": rid, "ok": True, "result": result})
        except Cancelled:
            send({"id": rid, "ok": False, "error": "Génération annulée.", "cancelled": True})
        except Exception as e:
            traceback.print_exc(file=sys.stderr)
            send({"id": rid, "ok": False, "error": f"{type(e).__name__}: {e}"})


if __name__ == "__main__":
    main()
