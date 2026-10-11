"""Étape 0, compléments — probe des clips de clips_phase2.json et mesures de décodage/encodage 4K.

Usage : python3 stage0/scripts/media_phase2.py   (stdlib uniquement, appelle ffmpeg/ffprobe)

1. Probe (codec, résolution, i/s, rotation, audio) via media_test.probe.
2. Extraction du proxy 720p (10 s) : décodage logiciel vs -hwaccel videotoolbox.
3. Export 5 s en résolution native : libx264 vs h264_videotoolbox vs hevc_videotoolbox.
"""
import json
import sys
import time
import subprocess
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from media_test import probe  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
SRC = ROOT / "clips" / "source"
WORK = ROOT / "cache" / "stage0" / "media_phase2"
RESULTS = ROOT / "stage0" / "results"


def timed(cmd):
    t0 = time.perf_counter()
    p = subprocess.run(cmd, capture_output=True, text=True)
    return round(time.perf_counter() - t0, 2), p.returncode == 0, p.stderr[-300:]


def main():
    cfg = json.loads((ROOT / "stage0" / "clips_phase2.json").read_text())
    WORK.mkdir(parents=True, exist_ok=True)
    out = {"note": "Mesures indicatives : un build Rust/Tauri pouvait tourner en parallèle.", "clips": []}
    for clip in cfg["clips"]:
        src = SRC / clip["file"]
        info = probe(src)
        info["id"] = clip["id"]
        info["has_audio"] = any(s["type"] == "audio" for s in info["streams"])

        proxy = {}
        for name, pre in [("software", []), ("videotoolbox", ["-hwaccel", "videotoolbox"])]:
            dt, ok, err = timed(["ffmpeg", "-v", "error", "-y", *pre, "-i", str(src), "-t", "10",
                                 "-vf", "scale=-2:720", "-q:v", "2", "-f", "null", "-"])
            proxy[name] = {"seconds_for_10s": dt, "ok": ok, **({} if ok else {"err": err})}
        info["proxy_720p_decode"] = proxy

        enc = {}
        for name, args in [("libx264_crf18_medium", ["-c:v", "libx264", "-crf", "18", "-preset", "medium"]),
                           ("h264_videotoolbox_q65", ["-c:v", "h264_videotoolbox", "-q:v", "65"]),
                           ("hevc_videotoolbox_q65", ["-c:v", "hevc_videotoolbox", "-q:v", "65", "-tag:v", "hvc1"])]:
            dst = WORK / f"{clip['id']}__{name}.mp4"
            dt, ok, err = timed(["ffmpeg", "-v", "error", "-y", "-i", str(src), "-t", "5", "-an", *args, str(dst)])
            enc[name] = {"seconds_for_5s": dt, "ok": ok,
                         "size_mb": round(dst.stat().st_size / 1e6, 2) if ok else None,
                         **({} if ok else {"err": err})}
        info["export_native_5s"] = enc
        out["clips"].append(info)
        print(json.dumps(info), flush=True)
    (RESULTS / "media_phase2.json").write_text(json.dumps(out, indent=2, ensure_ascii=False))


if __name__ == "__main__":
    main()
