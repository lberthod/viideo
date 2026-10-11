"""Étape 0 — compositing FFmpeg : masque SAM 2 + décor fixe -> MP4, plus vue de diagnostic 2x2.

Usage : python3 stage0/scripts/composite_test.py --model tiny --background stage0/results/backgrounds/x.png
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


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--model", default="tiny")
    ap.add_argument("--device", default="mps")
    ap.add_argument("--background", required=True)
    ap.add_argument("--feather", type=float, default=1.5, help="rayon de flou du masque (px)")
    args = ap.parse_args()

    cfg = json.loads((ROOT / "stage0" / "clips.json").read_text())
    OUT.mkdir(parents=True, exist_ok=True)
    results = []
    for clip in cfg["clips"]:
        frames = CACHE / clip["id"] / "frames"
        masks = CACHE / clip["id"] / f"masks_{args.model}_{args.device}"
        if not masks.exists():
            continue
        fps = fps_of(ROOT / "clips" / "source" / clip["file"])
        comp = OUT / f"{clip['id']}__{args.model}.mp4"
        diag = OUT / f"{clip['id']}__{args.model}__diag.mp4"
        inputs = ["-framerate", fps, "-i", str(frames / "%05d.jpg"),
                  "-framerate", fps, "-i", str(masks / "%05d.png"),
                  "-loop", "1", "-i", str(ROOT / args.background)]
        base = (f"[1:v]format=gray,boxblur={args.feather}[m];"
                f"[0:v]format=rgba[src];[src][m]alphamerge[fg];"
                f"[2:v]scale=1280:720:force_original_aspect_ratio=increase,crop=1280:720,format=rgba[bg];"
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
                      f"color=c=0x777777:s=1280x720:r={fps}[c1];color=c=0xbbbbbb:s=1280x720:r={fps}[c2];"
                      f"[c1][c2]blend=all_expr='if(eq(mod(floor(X/40)+floor(Y/40),2),0),A,B)'[chk];"
                      f"[chk][fg]overlay=shortest=1[iso];"
                      f"[2:v]scale=1280:720:force_original_aspect_ratio=increase,crop=1280:720,format=rgba[bg];"
                      f"[m3]boxblur={args.feather}[mc];[s3a][mc]alphamerge[fg2];[bg][fg2]overlay=shortest=1[cmp];"
                      f"[m2]format=rgba[mv];"
                      f"[s1][mv][iso][cmp]xstack=inputs=4:layout=0_0|w0_0|0_h0|w0_h0,scale=1280:-2,format=yuv420p[out]")
        p2 = subprocess.run(["ffmpeg", "-v", "error", "-y", *inputs, "-filter_complex", diag_graph,
                             "-map", "[out]", "-c:v", "libx264", "-crf", "20", "-preset", "veryfast", str(diag)],
                            capture_output=True, text=True)
        r = {"clip": clip["id"], "model": args.model, "composite_s": round(t_comp, 2),
             "ok": p.returncode == 0, "diag_ok": p2.returncode == 0,
             "composite": str(comp.relative_to(ROOT)), "diag": str(diag.relative_to(ROOT)),
             "errors": (p.stderr + p2.stderr)[-400:]}
        results.append(r)
        print(json.dumps(r), flush=True)
    (ROOT / "stage0" / "results" / f"composite_{args.model}.json").write_text(json.dumps(results, indent=2))


if __name__ == "__main__":
    main()
