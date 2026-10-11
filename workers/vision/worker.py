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

sys.path.insert(0, str(Path(__file__).resolve().parent))

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
        """Charge toute la vidéo dans un état SAM 2 (utilisé par les bancs d'essai ; l'app passe par
        `preview` et `propagate`, qui ne chargent que les images nécessaires)."""
        if self.predictor is None:
            raise RuntimeError("aucun modèle chargé (load_model)")
        frames_dir = self._path(p["frames_dir"], must_exist=True)
        n = len(list(frames_dir.glob("*.jpg")))
        if n == 0:
            raise ValueError("aucune image JPEG dans frames_dir")
        self.op_reset({})  # libère l'état précédent avant d'en créer un autre
        offload = bool(p.get("offload", n > 300))
        t0 = time.perf_counter()
        with self._autocast():
            self.state = self.predictor.init_state(video_path=str(frames_dir), offload_video_to_cpu=offload)
        self.frames_dir = frames_dir
        return {"frames": n, "offload": offload, "init_s": round(time.perf_counter() - t0, 2),
                "width": self.state["video_width"], "height": self.state["video_height"]}

    @staticmethod
    def _frame_files(frames_dir: Path):
        files = sorted(frames_dir.glob("*.jpg"), key=lambda f: int(f.stem))
        if not files:
            raise ValueError("aucune image JPEG dans frames_dir")
        return files

    def _link_frames(self, files, dest: Path):
        """Dossier de liens symboliques 00000.jpg… vers un sous-ensemble d'images (SAM 2 lit un dossier)."""
        import shutil
        shutil.rmtree(dest, ignore_errors=True)
        dest.mkdir(parents=True)
        for i, f in enumerate(files):
            (dest / f"{i:05d}.jpg").symlink_to(f)
        return dest

    def _init_subset(self, files, link_dir: Path):
        self._link_frames(files, link_dir)
        with self._autocast():
            return self.predictor.init_state(video_path=str(link_dir), offload_video_to_cpu=len(files) > 150)

    def op_preview(self, p):
        """Masque d'une seule image à partir de ses invites, sans propager.

        Nouveau format : {frames_dir, frame, prompts, out_png} — ne charge que cette image.
        Ancien format (après init_video) : {prompts, out_png}.
        """
        import shutil
        out = self._path(p["out_png"])
        out.parent.mkdir(parents=True, exist_ok=True)
        if "frames_dir" in p:
            if self.predictor is None:
                raise RuntimeError("aucun modèle chargé (load_model)")
            frame = int(p["frame"])
            files = self._frame_files(self._path(p["frames_dir"], must_exist=True))
            if not 0 <= frame < len(files):
                raise ValueError(f"image {frame} hors de l'extrait")
            link = out.parent / ".preview_frames"
            state = self._init_subset([files[frame]], link)
            prompts = [dict(pr, frame=0) for pr in p["prompts"] if int(pr["frame"]) == frame]
            saved, self.state = self.state, state
            try:
                with self._autocast():
                    last = self._add_prompts(prompts)
            finally:
                self.state = saved
                shutil.rmtree(link, ignore_errors=True)
        else:
            self._require_state()
            with self._autocast():
                self.predictor.reset_state(self.state)
                last = self._add_prompts(p["prompts"])
            frame = last[0] if last else None
        if last is None:
            raise ValueError("aucune invite valide pour cette image")
        _, (_, _, logits) = last
        mask = (logits[0, 0] > 0).float().cpu().numpy() > 0
        self._write_mask(out, mask)
        return {"frame": frame, "out_png": str(out), "area_pct": round(float(mask.mean() * 100), 2)}

    def op_propagate(self, p, req_id):
        """Propage le masque sur toute la vidéo, par blocs, et écrit un PNG par image + quality.json.

        Paramètres : frames_dir (ou la vidéo de init_video), out_dir, prompts, cleanup (ou null),
        chunk_size (défaut 400), resume (défaut true).

        Découpage : blocs [k·C, (k+1)·C] qui partagent leur image frontière. On part du bloc qui contient
        la première image clé F0 : propagation avant (et arrière jusqu'au début du bloc), puis blocs
        suivants amorcés par le masque de leur première image, puis blocs précédents en arrière, amorcés
        par le masque de leur dernière image. La mémoire est bornée par la taille d'un bloc (~12,6 Mo
        par image) et non par la durée de l'extrait. Les images clés d'un bloc s'appliquent à ce bloc
        et, via l'amorçage, à la suite ; elles n'influencent pas les blocs déjà calculés.

        Reprise : `chunks.json` dans out_dir mémorise les blocs terminés pour ces invites ; une
        propagation annulée ou interrompue repart du premier bloc non terminé.
        """
        import hashlib
        import shutil
        import cv2
        import maskqa
        if self.predictor is None:
            raise RuntimeError("aucun modèle chargé (load_model)")
        if "frames_dir" in p:
            frames_dir = self._path(p["frames_dir"], must_exist=True)
        elif self.frames_dir is not None:
            frames_dir = self.frames_dir
        else:
            raise RuntimeError("frames_dir manquant")
        files = self._frame_files(frames_dir)
        n = len(files)
        out_dir = self._path(p["out_dir"])
        out_dir.mkdir(parents=True, exist_ok=True)
        prompts = sorted(p["prompts"], key=lambda pr: int(pr["frame"]))
        if not prompts:
            raise ValueError("aucune invite valide")
        cleanup = p.get("cleanup")
        size = max(int(p.get("chunk_size", 400)), 16)
        f0 = int(prompts[0]["frame"])

        # Si une vidéo entière est chargée (init_video), on la libère : les blocs ont leur propre état.
        if self.state is not None:
            self.op_reset({})

        sig = hashlib.sha256(json.dumps([prompts, cleanup, n, size, self.model_name, self.dtype], sort_keys=True).encode()).hexdigest()
        ledger_path = out_dir / "chunks.json"
        ledger = None
        if p.get("resume", True) and ledger_path.exists():
            try:
                ledger = json.loads(ledger_path.read_text())
            except json.JSONDecodeError:
                ledger = None
        if not ledger or ledger.get("sig") != sig:
            for f in out_dir.glob("*.png"):
                f.unlink()
            ledger = {"sig": sig, "done": [], "cleanup": {}}

        def save_ledger():
            tmp = ledger_path.with_suffix(".tmp")
            tmp.write_text(json.dumps(ledger))
            tmp.replace(ledger_path)

        last_k = max((n - 2) // size, 0) if n > 1 else 0
        bounds = {k: (k * size, min((k + 1) * size, n - 1)) for k in range(last_k + 1)}
        k0 = min(f0 // size, last_k)
        order = list(range(k0, last_k + 1)) + list(range(k0 - 1, -1, -1))
        resumed = len(ledger["done"])
        written = set(int(f.stem) for f in out_dir.glob("*.png"))
        self.cancel.clear()
        t0 = time.perf_counter()
        computed = 0
        link_dir = out_dir / ".chunk_frames"

        def load_mask(idx):
            return cv2.imread(str(out_dir / f"{idx:05d}.png"), cv2.IMREAD_GRAYSCALE) > 127

        try:
            for k in order:
                if k in ledger["done"]:
                    continue
                s, e = bounds[k]
                state = self._init_subset(files[s:e + 1], link_dir)
                self.state = state
                with self._autocast():
                    local = [dict(pr, frame=int(pr["frame"]) - s) for pr in prompts if s <= int(pr["frame"]) <= e]
                    forward = k >= k0
                    if k > k0:  # amorce : masque de la première image, issu du bloc précédent
                        self.predictor.add_new_mask(state, frame_idx=0, obj_id=1, mask=load_mask(s))
                    if k < k0:  # amorce : masque de la dernière image, issu du bloc suivant
                        self.predictor.add_new_mask(state, frame_idx=e - s, obj_id=1, mask=load_mask(e))
                    self._add_prompts(local)
                    if k == k0:
                        start = f0 - s
                        passes = [(start, False)] + ([(start, True)] if start > 0 else [])
                    else:
                        passes = [(0 if forward else e - s, not forward)]
                    for start, reverse in passes:
                        for li, _, logits in self.predictor.propagate_in_video(state, start_frame_idx=start, reverse=reverse):
                            if self.cancel.is_set():
                                raise Cancelled()
                            g = s + li
                            if g in written:  # image frontière ou déjà calculée avant une reprise
                                continue
                            mask = (logits[0, 0] > 0).float().cpu().numpy() > 0
                            info = [0, 0, 0]
                            if cleanup:
                                mask, *info = maskqa.clean_mask(
                                    mask, float(cleanup.get("min_island_ratio", 0.05)), float(cleanup.get("max_hole_ratio", 0.002)))
                            self._write_mask(out_dir / f"{g:05d}.png", mask)
                            ledger["cleanup"][str(g)] = info
                            written.add(g)
                            computed += 1
                            if len(written) % 5 == 0 or len(written) == n:
                                send({"id": req_id, "event": "progress", "done": len(written), "total": n})
                self.predictor.reset_state(state)
                self.state = None
                del state
                self._empty_cache()
                ledger["done"].append(k)
                save_ledger()
        finally:
            self.state = None
            shutil.rmtree(link_dir, ignore_errors=True)
            if ledger["done"]:
                save_ledger()
        dt = time.perf_counter() - t0

        # Contrôle qualité en relisant les PNG dans l'ordre : mémoire constante quelle que soit la durée.
        stats, prev = [], None
        for idx in range(n):
            mask = load_mask(idx)
            st = maskqa.frame_stats(idx, mask, prev)
            removed, filled, area = ledger["cleanup"].get(str(idx), [0, 0, 0])
            st.islands_removed, st.holes_filled = removed, filled
            st.islands_area_pct = round(area / max(int(mask.sum()), 1) * 100, 3)
            stats.append(st)
            prev = mask
        flagged, warnings = maskqa.split_persistent(stats, maskqa.detect_drift(stats))
        summary = maskqa.summarize(stats, flagged)
        summary["warnings"] = len(warnings)
        quality = {"schema_version": 1, "cleanup": cleanup, "chunk_size": size, "chunks": len(bounds),
                   "summary": summary, "warnings": warnings, "flagged": flagged, "frames": maskqa.stats_to_json(stats)}
        tmp = out_dir / "quality.json.tmp"
        tmp.write_text(json.dumps(quality))
        tmp.replace(out_dir / "quality.json")
        return {"frames": n, "written": len(written), "computed": computed, "resumed_chunks": resumed,
                "chunks": len(bounds), "seconds": round(dt, 2), "fps": round(computed / dt, 2) if dt > 0 else 0,
                "empty_frames": summary["empty_frames"], "flagged": summary["flagged"], "summary": summary,
                "out_dir": str(out_dir)}

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
