"""Étape 5 — mesures objectives des plans animés de décor.

Usage : .venvs/vision/bin/python stage5/scripts/analyze_clips.py stage5/results/still_1024x576.png stage5/results/clips/*.mp4

Pour un décor placé derrière un sujet filmé caméra fixe, on veut :
  - fidélité : le plan reste le décor choisi (SSIM avec l'image d'origine, au début et à la fin) ;
  - caméra fixe : pas de déplacement global (flux optique médian ≈ 0) — sinon le décor « glisse »
    derrière un sujet immobile ;
  - du mouvement local (eau, nuages, herbe) : 95e centile du flux optique ;
  - pas de scintillement : écart-type de la luminance moyenne image par image ;
  - bouclable : SSIM entre la dernière et la première image (pour couvrir un extrait plus long).
"""
import json
import sys
from pathlib import Path

import cv2
import numpy as np


def ssim(a, b):
    a = a.astype(np.float64)
    b = b.astype(np.float64)
    c1, c2 = (0.01 * 255) ** 2, (0.03 * 255) ** 2
    mu_a, mu_b = cv2.GaussianBlur(a, (11, 11), 1.5), cv2.GaussianBlur(b, (11, 11), 1.5)
    s_a = cv2.GaussianBlur(a * a, (11, 11), 1.5) - mu_a ** 2
    s_b = cv2.GaussianBlur(b * b, (11, 11), 1.5) - mu_b ** 2
    s_ab = cv2.GaussianBlur(a * b, (11, 11), 1.5) - mu_a * mu_b
    m = ((2 * mu_a * mu_b + c1) * (2 * s_ab + c2)) / ((mu_a ** 2 + mu_b ** 2 + c1) * (s_a + s_b + c2))
    return float(m.mean())


def frames(path):
    cap = cv2.VideoCapture(str(path))
    out = []
    while True:
        ok, f = cap.read()
        if not ok:
            break
        out.append(f)
    fps = cap.get(cv2.CAP_PROP_FPS)
    cap.release()
    return out, fps


def analyze(still_path, clip):
    still = cv2.imread(str(still_path))
    fr, fps = frames(clip)
    h, w = fr[0].shape[:2]
    ref = cv2.cvtColor(cv2.resize(still, (w, h)), cv2.COLOR_BGR2GRAY)
    gray = [cv2.cvtColor(f, cv2.COLOR_BGR2GRAY) for f in fr]
    small = [cv2.resize(g, (w // 2, h // 2)) for g in gray]
    glob, local = [], []
    for a, b in zip(small, small[1:]):
        flow = cv2.calcOpticalFlowFarneback(a, b, None, 0.5, 3, 15, 3, 5, 1.2, 0)
        mag = np.hypot(flow[..., 0], flow[..., 1]) * 2  # en pixels de la pleine résolution
        glob.append(float(np.hypot(np.median(flow[..., 0]), np.median(flow[..., 1])) * 2))
        local.append(float(np.percentile(mag, 95)))
    lum = np.array([g.mean() for g in gray])
    return {
        "clip": Path(clip).stem,
        "frames": len(fr), "fps": round(fps, 2), "size": f"{w}x{h}",
        "ssim_first_vs_still": round(ssim(gray[0], ref), 3),
        "ssim_last_vs_still": round(ssim(gray[-1], ref), 3),
        "ssim_mean_vs_still": round(float(np.mean([ssim(g, ref) for g in gray[:: max(1, len(gray) // 12)]])), 3),
        "camera_drift_px_per_frame": round(float(np.median(glob)), 3),
        "camera_drift_total_px": round(float(np.sum(glob)), 1),
        "local_motion_p95_px": round(float(np.median(local)), 2),
        "flicker_luma_std": round(float(np.std(np.diff(lum))), 3),
        "loop_ssim_last_vs_first": round(ssim(gray[-1], gray[0]), 3),
    }


def contact_sheet(clips, dest):
    """Première, milieu et dernière image de chaque plan, une ligne par moteur."""
    rows = []
    for c in clips:
        fr, _ = frames(c)
        picks = [fr[0], fr[len(fr) // 2], fr[-1]]
        tiles = []
        for i, f in enumerate(picks):
            t = cv2.resize(f, (480, 270))
            cv2.putText(t, f"{Path(c).stem} {['debut', 'milieu', 'fin'][i]}", (8, 26), 0, 0.7, (0, 255, 255), 2)
            tiles.append(t)
        rows.append(np.hstack(tiles))
    cv2.imwrite(str(dest), np.vstack(rows), [cv2.IMWRITE_JPEG_QUALITY, 82])


if __name__ == "__main__":
    still, clips = sys.argv[1], sys.argv[2:]
    res = [analyze(still, c) for c in clips]
    for r in res:
        print(json.dumps(r))
    out = Path(clips[0]).parent.parent
    (out / "clip_metrics.json").write_text(json.dumps(res, indent=2))
    contact_sheet(clips, out / "clips_sheet.jpg")
