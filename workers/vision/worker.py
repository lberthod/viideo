"""Worker vision videeo : segmentation vidéo SAM 2.1 pilotée en JSON Lines sur stdin/stdout.

Protocole (schema_version 1) — une ligne JSON par message :
  requête   {"id": "...", "op": "...", "params": {...}}
  réponse   {"id": "...", "ok": true, "result": {...}}  ou  {"id": "...", "ok": false, "error": "..."}
  événement {"id": "...", "event": "progress", "done": n, "total": m}

Opérations : ping, load_model, init_video, preview, propagate, cancel, reset, unload, shutdown.
Le worker n'exécute aucune commande shell et n'écrit que sous le répertoire racine fourni (--root).
stdout est réservé au protocole : toute sortie des bibliothèques est redirigée vers stderr.
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
sys.stdout = sys.stderr  # tqdm, warnings et prints de bibliothèques ne polluent pas le protocole

os.environ.setdefault("PYTORCH_ENABLE_MPS_FALLBACK", "1")
os.environ.setdefault("TQDM_DISABLE", "1")

_write_lock = threading.Lock()


def send(msg):
    with _write_lock:
        PROTO_OUT.write(json.dumps(msg) + "\n")
        PROTO_OUT.flush()


class Cancelled(Exception):
    pass


class VisionWorker:
    MODELS = {
        "tiny": ("configs/sam2.1/sam2.1_hiera_t.yaml", "sam2.1_hiera_tiny.pt"),
        "small": ("configs/sam2.1/sam2.1_hiera_s.yaml", "sam2.1_hiera_small.pt"),
    }

    def __init__(self, models_dir: Path, root: Path):
        self.models_dir = models_dir
        self.root = root.resolve()
        self.predictor = None
        self.model_name = None
        self.device = None
        self.dtype = None
        self.state = None
        self.frames_dir = None
        self.cancel = threading.Event()

    # --- utilitaires -------------------------------------------------------
    def _path(self, p, must_exist=False):
        path = Path(p).resolve()
        if path != self.root and self.root not in path.parents:
            raise ValueError(f"chemin hors du répertoire autorisé : {path}")
        if must_exist and not path.exists():
            raise FileNotFoundError(str(path))
        return path

    def _autocast(self):
        import torch
        enabled = self.dtype == "bf16" and self.device == "mps"
        return torch.autocast(self.device, dtype=torch.bfloat16, enabled=enabled)

    def _require_state(self):
        if self.predictor is None:
            raise RuntimeError("aucun modèle chargé (load_model)")
        if self.state is None:
            raise RuntimeError("aucune vidéo initialisée (init_video)")

    @staticmethod
    def _write_mask(path, mask):
        import cv2
        import numpy as np
        tmp = path.with_suffix(".tmp.png")
        cv2.imwrite(str(tmp), (mask.astype(np.uint8) * 255))
        tmp.replace(path)

    def _add_prompts(self, prompts):
        """Ajoute toutes les invites (une par image clé) ; renvoie les logits de la dernière."""
        import numpy as np
        last = None
        for pr in prompts:
            pts = pr.get("points") or []
            box = pr.get("box")
            kwargs = {"inference_state": self.state, "frame_idx": int(pr["frame"]), "obj_id": 1}
            if pts:
                kwargs["points"] = np.array([[p["x"], p["y"]] for p in pts], dtype=np.float32)
                kwargs["labels"] = np.array([1 if p.get("positive", True) else 0 for p in pts], dtype=np.int32)
            if box:
                kwargs["box"] = np.array(box, dtype=np.float32)
            if not pts and not box:
                continue
            last = (int(pr["frame"]), self.predictor.add_new_points_or_box(**kwargs))
        return last

    # --- opérations --------------------------------------------------------
    def op_ping(self, _):
        import torch
        return {"schema_version": SCHEMA_VERSION, "pid": os.getpid(), "torch": torch.__version__,
                "mps": torch.backends.mps.is_available(), "model": self.model_name}

    def op_load_model(self, p):
        import torch
        from sam2.build_sam import build_sam2_video_predictor
        name = p.get("model", "tiny")
        device = p.get("device", "mps" if torch.backends.mps.is_available() else "cpu")
        dtype = p.get("dtype", "bf16")
        if name not in self.MODELS:
            raise ValueError(f"modèle inconnu : {name}")
        if self.predictor is not None and (name, device, dtype) == (self.model_name, self.device, self.dtype):
            return {"model": name, "device": device, "dtype": dtype, "cached": True}
        self.op_unload({})
        cfg, ckpt = self.MODELS[name]
        t0 = time.perf_counter()
        self.predictor = build_sam2_video_predictor(cfg, str(self.models_dir / ckpt), device=torch.device(device))
        self.model_name, self.device, self.dtype = name, device, dtype
        return {"model": name, "device": device, "dtype": dtype, "load_s": round(time.perf_counter() - t0, 2)}

    def op_init_video(self, p):
        if self.predictor is None:
            raise RuntimeError("aucun modèle chargé (load_model)")
        frames_dir = self._path(p["frames_dir"], must_exist=True)
        n = len(list(frames_dir.glob("*.jpg")))
        if n == 0:
            raise ValueError("aucune image JPEG dans frames_dir")
        offload = bool(p.get("offload", n > 300))  # au-delà de ~300 images, garder les images en RAM CPU
        t0 = time.perf_counter()
        with self._autocast():
            self.state = self.predictor.init_state(video_path=str(frames_dir), offload_video_to_cpu=offload)
        self.frames_dir = frames_dir
        return {"frames": n, "offload": offload, "init_s": round(time.perf_counter() - t0, 2),
                "width": self.state["video_width"], "height": self.state["video_height"]}

    def op_preview(self, p):
        """Applique les invites et écrit le masque de l'image demandée (sans propager)."""
        self._require_state()
        out = self._path(p["out_png"])
        out.parent.mkdir(parents=True, exist_ok=True)
        with self._autocast():
            self.predictor.reset_state(self.state)
            last = self._add_prompts(p["prompts"])
        if last is None:
            raise ValueError("aucune invite valide")
        frame, (_, _, logits) = last
        mask = (logits[0, 0] > 0).float().cpu().numpy() > 0
        self._write_mask(out, mask)
        return {"frame": frame, "out_png": str(out), "area_pct": round(float(mask.mean() * 100), 2)}

    def op_propagate(self, p, req_id):
        """Rejoue toutes les invites, propage dans toute la vidéo et écrit un PNG par image."""
        self._require_state()
        out_dir = self._path(p["out_dir"])
        out_dir.mkdir(parents=True, exist_ok=True)
        total = self.state["num_frames"]
        self.cancel.clear()
        t0 = time.perf_counter()
        done, areas = 0, []
        with self._autocast():
            self.predictor.reset_state(self.state)
            if self._add_prompts(p["prompts"]) is None:
                raise ValueError("aucune invite valide")
            first = min(int(pr["frame"]) for pr in p["prompts"])
            # Propagation avant depuis la première image clé, puis arrière si elle n'est pas l'image 0.
            passes = [False] + ([True] if first > 0 else [])
            for reverse in passes:
                for idx, _, logits in self.predictor.propagate_in_video(self.state, start_frame_idx=first,
                                                                        reverse=reverse):
                    if self.cancel.is_set():
                        raise Cancelled()
                    mask = (logits[0, 0] > 0).float().cpu().numpy() > 0
                    self._write_mask(out_dir / f"{idx:05d}.png", mask)
                    areas.append(float(mask.mean()))
                    done += 1
                    if done % 5 == 0 or done == total:
                        send({"id": req_id, "event": "progress", "done": min(done, total), "total": total})
        dt = time.perf_counter() - t0
        empty = sum(1 for a in areas if a == 0)
        return {"frames": total, "written": done, "seconds": round(dt, 2), "fps": round(done / dt, 2),
                "empty_frames": empty, "out_dir": str(out_dir)}

    def op_reset(self, _):
        if self.predictor is not None and self.state is not None:
            self.predictor.reset_state(self.state)
        self.state = None
        self.frames_dir = None
        self._empty_cache()
        return {}

    def op_unload(self, _):
        self.state = None
        self.predictor = None
        self.model_name = None
        self._empty_cache()
        return {}

    @staticmethod
    def _empty_cache():
        import gc
        gc.collect()
        try:
            import torch
            if torch.backends.mps.is_available():
                torch.mps.empty_cache()
        except Exception:
            pass


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--models-dir", required=True)
    ap.add_argument("--root", required=True, help="seul répertoire où le worker peut lire/écrire")
    args = ap.parse_args()
    worker = VisionWorker(Path(args.models_dir), Path(args.root))

    # Lecture de stdin dans un thread : « cancel » doit pouvoir arriver pendant une propagation.
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

    ops = {"ping": worker.op_ping, "load_model": worker.op_load_model, "init_video": worker.op_init_video,
           "preview": worker.op_preview, "reset": worker.op_reset, "unload": worker.op_unload}
    while True:
        msg = inbox.get()
        if msg is None or msg.get("op") == "shutdown":
            break
        rid, op, params = msg.get("id"), msg.get("op"), msg.get("params") or {}
        try:
            if op == "propagate":
                result = worker.op_propagate(params, rid)
            elif op in ops:
                result = ops[op](params)
            else:
                raise ValueError(f"opération inconnue : {op}")
            send({"id": rid, "ok": True, "result": result})
        except Cancelled:
            send({"id": rid, "ok": False, "error": "Propagation annulée.", "cancelled": True})
        except Exception as e:
            traceback.print_exc(file=sys.stderr)
            send({"id": rid, "ok": False, "error": f"{type(e).__name__}: {e}"})


if __name__ == "__main__":
    main()
