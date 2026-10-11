"""Étape 0 — compositing FFmpeg : masque SAM 2 + décor fixe -> MP4, plus vue de diagnostic 2x2.

Usage : python3 stage0/scripts/composite_test.py --model tiny --background stage0/results/backgrounds/x.png
        [--config stage0/clips_phase2.json] [--variant bf16]
La sortie a la taille des images proxy (1280x720 en paysage, ~406x720 en vertical).
Diagnostic : source | masque / sujet isolé sur damier | composite.
"""
import argparse
import json
import subprocess
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
CACHE = ROOT / "cache" / "stage0"
OUT = ROOT / "stage0" / "results" / "composites"


def fps_of(path):
    return subprocess.run(["ffprobe", "-v", "error", "-select_streams", "v:0", "-show_entries",
                           "stream=r_frame_rate", "-of", "csv=p=0", str(path)],
                          check=True, capture_output=True, text=True).stdout.strip()


def frame_size(path):
    w, h = subprocess.run(["ffprobe", "-v", "error", "-select_streams", "v:0", "-show_entries",
                           "stream=width,height", "-of", "csv=p=0", str(path)],
                          check=True, capture_output=True, text=True).stdout.strip().split(",")[:2]
    return int(w), int(h)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--model", default="tiny")
    ap.add_argument("--device", default="mps")
    ap.add_argument("--background", required=True)
    ap.add_argument("--feather", type=float, default=1.5, help="rayon de flou du masque (px)")
    ap.add_argument("--config", default="stage0/clips.json", help="fichier de clips (relatif à la racine)")
    ap.add_argument("--variant", default="", help="suffixe du dossier de masques, ex. bf16 -> masks_tiny_mps_bf16")
    args = ap.parse_args()

    cfg = json.loads((ROOT / args.config).read_text())
    vsuf = f"_{args.variant}" if args.variant else ""
    OUT.mkdir(parents=True, exist_ok=True)
    results = []
    for clip in cfg["clips"]:
        frames = CACHE / clip["id"] / "frames"
        masks = CACHE / clip["id"] / f"masks_{args.model}_{args.device}{vsuf}"
        if not masks.exists():
            continue
        fps = fps_of(ROOT / "clips" / "source" / clip["file"])
        W, H = frame_size(frames / "00000.jpg")
        comp = OUT / f"{clip['id']}__{args.model}{vsuf}.mp4"
        diag = OUT / f"{clip['id']}__{args.model}{vsuf}__diag.mp4"
        inputs = ["-framerate", fps, "-i", str(frames / "%05d.jpg"),
                  "-framerate", fps, "-i", str(masks / "%05d.png"),
                  "-loop", "1", "-framerate", fps, "-i", str(ROOT / args.background)]
        base = (f"[1:v]format=gray,boxblur={args.feather}[m];"
                f"[0:v]format=rgba[src];[src][m]alphamerge[fg];"
                f"[2:v]scale={W}:{H}:force_original_aspect_ratio=increase,crop={W}:{H},format=rgba[bg];"
                f"[bg][fg]overlay=shortest=1,format=yuv420p")
        t0 = time.perf_counter()
        p = subprocess.run(["ffmpeg", "-v", "error", "-y", *inputs, "-filter_complex", base + "[out]",
                            "-map", "[out]", "-c:v", "libx264", "-crf", "18", "-preset", "medium",
                            "-movflags", "+faststart", str(comp)], capture_output=True, text=True)
        t_comp = time.perf_counter() - t0

        diag_graph = (f"[1:v]format=gray,split=3[m1][m2][m3];"
                      f"[m1]boxblur={args.feather}[mb];"
                      f"[0:v]format=rgba,split=3[s1][s2][s3];"
                      f"[s2][mb]alphamerge[fg];[s3]format=rgba[s3a];"
                      f"color=c=0x777777:s={W}x{H}:r={fps}[c1];color=c=0xbbbbbb:s={W}x{H}:r={fps}[c2];"
                      f"[c1][c2]blend=all_expr='if(eq(mod(floor(X/40)+floor(Y/40),2),0),A,B)'[chk];"
                      f"[chk][fg]overlay=shortest=1[iso];"
                      f"[2:v]scale={W}:{H}:force_original_aspect_ratio=increase,crop={W}:{H},format=rgba[bg];"
                      f"[m3]boxblur={args.feather}[mc];[s3a][mc]alphamerge[fg2];[bg][fg2]overlay=shortest=1[cmp];"
                      f"[m2]format=rgba[mv];"
                      f"[s1][mv][iso][cmp]xstack=inputs=4:layout=0_0|w0_0|0_h0|w0_h0,scale={min(2 * W, 1280)}:-2,format=yuv420p[out]")
        p2 = subprocess.run(["ffmpeg", "-v", "error", "-y", *inputs, "-filter_complex", diag_graph,
                             "-map", "[out]", "-c:v", "libx264", "-crf", "20", "-preset", "veryfast", str(diag)],
                            capture_output=True, text=True)
        r = {"clip": clip["id"], "model": args.model, "composite_s": round(t_comp, 2),
             "ok": p.returncode == 0, "diag_ok": p2.returncode == 0,
             "composite": str(comp.relative_to(ROOT)), "diag": str(diag.relative_to(ROOT)),
             "errors": (p.stderr + p2.stderr)[-400:]}
        results.append(r)
        print(json.dumps(r), flush=True)
    cfg_tag = "" if Path(args.config).name == "clips.json" else "_" + Path(args.config).stem
    (ROOT / "stage0" / "results" / f"composite_{args.model}{vsuf}{cfg_tag}.json").write_text(
        json.dumps(results, indent=2))


if __name__ == "__main__":
    main()
