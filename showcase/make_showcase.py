"""Vidéo de démonstration : source | premier rendu | rendu corrigé, légendes AU-DESSUS de l'image.

Usage : .venvs/vision/bin/python showcase/make_showcase.py
Les rendus sont ceux exportés par l'app (cache/<projet>/exports) ; rien n'est retouché ici.
"""
import json
import os
import subprocess
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "showcase"
FONT = next(p for p in ["/System/Library/Fonts/Supplemental/Arial Bold.ttf", "/System/Library/Fonts/Helvetica.ttc"] if os.path.exists(p))
FONT_R = next(p for p in ["/System/Library/Fonts/Supplemental/Arial.ttf", "/System/Library/Fonts/Helvetica.ttc"] if os.path.exists(p))
W, H, BAR = 480, 270, 64

EXAMPLES = [
    ("Présentatrice, terrasse alpine", "dev_presenter", "v2_presenter_alps",
     "Pupitre exclu + comptoir en bois généré devant (appui des mains, coupure cachée), light wrap, harmonisation"),
    ("Présentatrice, studio TV (Assistant)", "dev_assistant", "v2_presenter_tv",
     "Pupitre exclu + comptoir TV généré devant, décor généré, light wrap, harmonisation"),
    ("Randonneur, lac alpin animé", "dev_anim", "v2_hiker",
     "Cinémagraphe (lac + ciel animés), ombre de contact sous les pieds, light wrap, lumière dorée"),
]


def latest_export(project):
    d = ROOT / "cache" / project / "exports"
    return max(d.glob("*.mp4"), key=lambda p: p.stat().st_mtime)


def clip_of(project):
    m = json.loads((ROOT / "cache" / project / "project.json").read_text())
    return m["source"]["path"], m["clip"]["start"], m["clip"]["duration"]


def bar(title, detail, path):
    im = Image.new("RGB", (3 * W, BAR), (18, 20, 23))
    d = ImageDraw.Draw(im)
    d.text((14, 8), title, font=ImageFont.truetype(FONT, 22), fill=(255, 255, 255))
    d.text((14, 38), detail, font=ImageFont.truetype(FONT_R, 15), fill=(190, 196, 204))
    im.save(path)


def labels(path):
    im = Image.new("RGBA", (3 * W, H), (0, 0, 0, 0))
    d = ImageDraw.Draw(im)
    f = ImageFont.truetype(FONT, 17)
    for i, t in enumerate(["SOURCE", "PREMIER RENDU", "RENDU CORRIGÉ"]):
        x = i * W + 10
        tw = d.textlength(t, font=f)
        d.rounded_rectangle((x, 8, x + tw + 16, 34), 5, fill=(0, 0, 0, 165))
        d.text((x + 8, 11), t, font=f, fill=(255, 255, 255, 255) if i < 2 else (255, 214, 102, 255))
    im.save(path)


def main():
    parts = []
    lab = OUT / "_labels.png"
    labels(lab)
    for i, (title, old, new, detail) in enumerate(EXAMPLES):
        src, start, dur = clip_of(new)
        b = OUT / f"_bar{i}.png"
        bar(title, detail, b)
        part = OUT / f"_part{i}.mp4"
        sc = f"fps=24,scale={W}:{H}:force_original_aspect_ratio=decrease,pad={W}:{H}:(ow-iw)/2:(oh-ih)/2,setsar=1"
        subprocess.run(["ffmpeg", "-v", "error", "-y", "-ss", str(start), "-t", str(dur), "-i", src,
                        "-i", str(latest_export(old)), "-i", str(latest_export(new)), "-loop", "1", "-i", str(b),
                        "-loop", "1", "-i", str(lab), "-filter_complex",
                        f"[0:v]{sc}[a];[1:v]{sc}[b];[2:v]{sc}[c];[a][b][c]hstack=3[row];[row][4:v]overlay=0:0[rowl];"
                        f"[3:v][rowl]vstack,format=yuv420p[v]",
                        "-map", "[v]", "-t", str(dur), "-an", "-c:v", "libx264", "-crf", "19", "-r", "24", str(part)], check=True)
        parts.append(part)
    lst = OUT / "_list.txt"
    lst.write_text("".join(f"file '{p}'\n" for p in parts))
    dest = OUT / "videeo_avant_apres_v2.mp4"
    subprocess.run(["ffmpeg", "-v", "error", "-y", "-f", "concat", "-safe", "0", "-i", str(lst), "-c:v", "libx264", "-crf", "19",
                    "-pix_fmt", "yuv420p", "-movflags", "+faststart", str(dest)], check=True)
    for p in [*parts, lst, lab, *OUT.glob("_bar*.png")]:
        p.unlink()
    print(dest)


if __name__ == "__main__":
    main()
