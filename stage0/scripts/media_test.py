"""Étape 0 — test média : probe, clip « type téléphone » (rotation + audio), export MP4.

Usage : python3 stage0/scripts/media_test.py   (stdlib uniquement, appelle ffmpeg/ffprobe)

Vérifie que l'export conserve la durée, l'audio, l'orientation d'affichage et la synchro
audio/vidéo, et compare l'encodeur logiciel (libx264) à VideoToolbox (h264_videotoolbox).
"""
import json
import subprocess
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SRC = ROOT / "clips" / "source"
WORK = ROOT / "cache" / "stage0" / "media"
RESULTS = ROOT / "stage0" / "results"


def probe(path):
    out = subprocess.run(
        ["ffprobe", "-v", "error", "-print_format", "json", "-show_format", "-show_streams", str(path)],
        check=True, capture_output=True, text=True,
    ).stdout
    d = json.loads(out)
    info = {"file": path.name, "duration": float(d["format"]["duration"]),
            "size_mb": round(int(d["format"]["size"]) / 1e6, 2), "streams": []}
    for s in d["streams"]:
        st = {"type": s["codec_type"], "codec": s["codec_name"], "duration": float(s.get("duration", 0))}
        if s["codec_type"] == "video":
            rot = 0
            for sd in s.get("side_data_list", []):
                if "rotation" in sd:
                    rot = int(sd["rotation"])
            st.update(width=s["width"], height=s["height"], fps=s["r_frame_rate"],
                      pix_fmt=s.get("pix_fmt"), rotation=rot,
                      display=f"{s['height']}x{s['width']}" if rot % 180 else f"{s['width']}x{s['height']}")
        if s["codec_type"] == "audio":
            st.update(sample_rate=s["sample_rate"], channels=s["channels"])
        info["streams"].append(st)
    return info


def run(cmd):
    t0 = time.perf_counter()
    p = subprocess.run(cmd, capture_output=True, text=True)
    return time.perf_counter() - t0, p


def main():
    WORK.mkdir(parents=True, exist_ok=True)
    report = {"sources": [probe(p) for p in sorted(SRC.glob("*.mp4"))]}

    # Clip simulant un iPhone : HEVC paysage + matrice d'affichage 90° (portrait), audio AAC 48 kHz.
    # Encodage puis remux en copie : seul le remux sans réencodage conserve la matrice de rotation.
    phone = WORK / "phone_like.mov"
    landscape = WORK / "phone_like_landscape.mov"
    dt, p = run(["ffmpeg", "-v", "error", "-y", "-i", str(SRC / "mixkit-16189.mp4"),
                 "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000",
                 "-t", "8", "-map", "0:v", "-map", "1:a",
                 "-c:v", "hevc_videotoolbox", "-q:v", "60", "-tag:v", "hvc1",
                 "-c:a", "aac", "-b:a", "128k", "-shortest", str(landscape)])
    if p.returncode == 0:
        dt2, p = run(["ffmpeg", "-v", "error", "-y", "-display_rotation:v:0", "90", "-i", str(landscape),
                      "-c", "copy", str(phone)])
        dt += dt2
    report["phone_like_create"] = {"seconds": round(dt, 2), "ok": p.returncode == 0, "stderr": p.stderr[-500:]}
    src_info = probe(phone)
    report["phone_like_probe"] = src_info

    exports = {
        "libx264_crf18": ["-c:v", "libx264", "-preset", "medium", "-crf", "18"],
        "libx264_veryfast": ["-c:v", "libx264", "-preset", "veryfast", "-crf", "20"],
        "videotoolbox_h264": ["-c:v", "h264_videotoolbox", "-q:v", "65"],
    }
    report["exports"] = {}
    src_v = next(s for s in src_info["streams"] if s["type"] == "video")
    for name, vargs in exports.items():
        out = WORK / f"export_{name}.mp4"
        tmp = out.with_suffix(".part.mp4")
        dt, p = run(["ffmpeg", "-v", "error", "-y", "-i", str(phone), *vargs, "-pix_fmt", "yuv420p",
                     "-c:a", "aac", "-b:a", "160k", "-movflags", "+faststart", str(tmp)])
        res = {"seconds": round(dt, 2), "ok": p.returncode == 0, "stderr": p.stderr[-300:]}
        if p.returncode == 0:
            tmp.replace(out)  # export atomique : on ne publie le fichier qu'une fois complet
            info = probe(out)
            v = next(s for s in info["streams"] if s["type"] == "video")
            a = [s for s in info["streams"] if s["type"] == "audio"]
            res.update(
                size_mb=info["size_mb"], display=v["display"], rotation_tag=v["rotation"],
                orientation_preserved=v["display"] == src_v["display"],
                audio_present=bool(a),
                duration_delta_s=round(abs(info["duration"] - src_info["duration"]), 3),
                av_delta_s=round(abs(v["duration"] - a[0]["duration"]), 3) if a else None,
            )
            # Décodage complet pour détecter une corruption éventuelle
            _, chk = run(["ffmpeg", "-v", "error", "-i", str(out), "-f", "null", "-"])
            res["decode_errors"] = chk.stderr.strip()[:300]
        report["exports"][name] = res

    _, p = run(["ffmpeg", "-hide_banner", "-filters"])
    report["ffmpeg_filters_missing"] = [f for f in ("drawtext", "zscale", "libplacebo", "alphamerge", "overlay")
                                        if f" {f} " not in p.stdout]

    RESULTS.mkdir(parents=True, exist_ok=True)
    (RESULTS / "media_test.json").write_text(json.dumps(report, indent=2, ensure_ascii=False))
    print(json.dumps(report["exports"], indent=2))
    print("phone_like:", json.dumps(report["phone_like_probe"]["streams"]))
    print("missing filters:", report["ffmpeg_filters_missing"])


if __name__ == "__main__":
    main()
