"""Nettoyage et contrôle qualité des masques de segmentation (fonctions pures, numpy/OpenCV).

- `clean_mask` retire les îlots parasites et bouche les petits trous intérieurs.
- `FrameStats` résume une image ; `detect_drift` repère les images suspectes à partir de la série,
  avec des seuils relatifs au clip (un clip très animé n'est pas pénalisé pour son mouvement normal).
"""
from dataclasses import asdict, dataclass

import cv2
import numpy as np


def clean_mask(mask: np.ndarray, min_island_ratio: float = 0.05, max_hole_ratio: float = 0.0002):
    """Renvoie (masque nettoyé, nb d'îlots retirés, nb de trous bouchés, aire des îlots retirés en px).

    - îlot : composante connexe plus petite que `min_island_ratio` × la plus grande composante ;
    - trou : composante de fond qui ne touche pas le bord de l'image et dont l'aire est inférieure
      à `max_hole_ratio` × l'aire de l'image.

    Le seuil des trous est volontairement très bas (0,02 % : ~180 px en 720p). Une « poche » de
    décor fermée par le sujet (entre une bretelle de sac, un bras et une jambe) est un vrai vide : la
    boucher y collerait le paysage de la vidéo source. Mesuré sur le randonneur : poches de 800 à
    2 700 px, défauts réels de SAM de quelques dizaines de px (médiane 71).
    """
    m = mask.astype(np.uint8)
    removed = filled = removed_area = 0
    n, labels, stats, _ = cv2.connectedComponentsWithStats(m, connectivity=8)
    if n > 2:
        areas = stats[1:, cv2.CC_STAT_AREA]
        keep = areas >= areas.max() * min_island_ratio
        removed = int((~keep).sum())
        removed_area = int(areas[~keep].sum())
        if removed:
            lut = np.zeros(n, np.uint8)
            lut[1:][keep] = 1
            m = lut[labels]
    if max_hole_ratio > 0:
        inv = (1 - m).astype(np.uint8)
        n, labels, stats, _ = cv2.connectedComponentsWithStats(inv, connectivity=4)
        h, w = m.shape
        limit = max_hole_ratio * h * w
        for i in range(1, n):
            x, y, bw, bh, area = stats[i]
            touches = x == 0 or y == 0 or x + bw == w or y + bh == h
            if not touches and area <= limit:
                m[labels == i] = 1
                filled += 1
    return m.astype(bool), removed, filled, removed_area


@dataclass
class FrameStats:
    frame: int
    area_pct: float
    components: int
    iou_prev: float
    cx: float
    cy: float
    # Part de la surface hors de la plus grande composante (fragmentation), en % du masque.
    fragment_pct: float = 0.0
    islands_removed: int = 0
    holes_filled: int = 0
    # Aire des îlots retirés par le nettoyage, en % du masque.
    islands_area_pct: float = 0.0


def frame_stats(idx: int, mask: np.ndarray, prev: np.ndarray | None) -> FrameStats:
    h, w = mask.shape
    area = float(mask.mean())
    n, _, cc, _ = cv2.connectedComponentsWithStats(mask.astype(np.uint8), connectivity=8)
    comp_areas = cc[1:, cv2.CC_STAT_AREA]
    total = int(comp_areas.sum())
    fragment = float((total - comp_areas.max()) / total * 100) if total else 0.0
    if prev is not None:
        union = np.logical_or(mask, prev).sum()
        iou = float(np.logical_and(mask, prev).sum() / union) if union else 1.0
    else:
        iou = 1.0
    ys, xs = np.nonzero(mask)
    cx = float(xs.mean() / w) if len(xs) else -1.0
    cy = float(ys.mean() / h) if len(ys) else -1.0
    return FrameStats(idx, round(area * 100, 3), n - 1, round(iou, 4), round(cx, 4), round(cy, 4), round(fragment, 3))


def detect_drift(stats: list[FrameStats], window: int = 7):
    """Repère les images suspectes. Renvoie [{frame, score, reasons}] trié par image.

    Critères (relatifs au clip) :
    - masque vide ;
    - aire hors de [0,6 ; 1,6] × la médiane glissante des images voisines ;
    - rupture : IoU avec l'image précédente très inférieur à la normale du clip (médiane − 4 MAD,
      et < 0,85) ET aire qui change de plus de 20 % : un mouvement rapide (danse, bras) fait chuter
      l'IoU mais garde l'aire, alors qu'une perte ou un changement de sujet modifie l'aire ;
    - saut du centre de masse : plus de 8 % de l'image ET plus de 0,6 × la taille du sujet
      (racine de son aire) ; un grand sujet qui bouge les bras déplace légitimement son centre ;
    - fragmentation significative : morceaux secondaires (ou îlots retirés par le nettoyage) pesant
      au moins 1 % du masque. Quelques pixels détachés (bouts de mèches) ne sont pas signalés ;
      le nettoyage améliore le rendu mais ne doit pas cacher un masque réellement instable.
    """
    if not stats:
        return []
    s = sorted(stats, key=lambda x: x.frame)
    areas = np.array([x.area_pct for x in s])
    ious = np.array([x.iou_prev for x in s[1:]]) if len(s) > 1 else np.array([1.0])
    med_iou = float(np.median(ious))
    mad = float(np.median(np.abs(ious - med_iou))) or 0.005
    iou_floor = min(0.85, med_iou - 4 * mad)
    out = []
    for i, x in enumerate(s):
        reasons = []
        score = 0.0
        prev_area = s[i - 1].area_pct if i > 0 else x.area_pct
        area_change = abs(x.area_pct - prev_area) / max(prev_area, 1e-6)
        if x.area_pct == 0:
            reasons.append("masque vide")
            score += 3
        else:
            lo, hi = max(0, i - window), min(len(s), i + window + 1)
            neighbours = np.delete(areas[lo:hi], i - lo)
            ref = float(np.median(neighbours)) if len(neighbours) else x.area_pct
            if ref > 0:
                ratio = x.area_pct / ref
                if ratio < 0.6 or ratio > 1.6:
                    reasons.append(f"aire ×{ratio:.2f} par rapport aux voisines")
                    score += min(abs(np.log(ratio)) * 3, 3)
        if i > 0 and x.iou_prev < iou_floor and area_change > 0.2:
            reasons.append(f"rupture avec l'image précédente (IoU {x.iou_prev:.2f})")
            score += (iou_floor - x.iou_prev) * 10
        if i > 0 and x.cx >= 0 and s[i - 1].cx >= 0:
            jump = float(np.hypot(x.cx - s[i - 1].cx, x.cy - s[i - 1].cy))
            size = float(np.sqrt(max(min(x.area_pct, s[i - 1].area_pct), 1e-6) / 100))
            if jump > 0.08 and jump / size > 0.6:
                reasons.append(f"saut de position ({jump * 100:.0f} % de l'image)")
                score += jump * 10
        if x.components > 1 and x.fragment_pct >= 1.0:
            reasons.append(f"{x.components} morceaux séparés ({x.fragment_pct:.1f} % du masque hors du sujet principal)")
            score += min(x.fragment_pct / 2, 2)
        if x.islands_area_pct >= 1.0:
            reasons.append(f"{x.islands_removed} îlots parasites retirés ({x.islands_area_pct:.1f} % du masque)")
            score += min(x.islands_area_pct / 2, 2)
        if reasons:
            out.append({"frame": x.frame, "score": round(score, 2), "reasons": reasons})
    return out


FRAGMENT_PREFIXES = ("morceaux séparés", "îlots parasites retirés")


def split_persistent(stats: list[FrameStats], flagged: list[dict], share: float = 0.5):
    """Une fragmentation présente sur plus de `share` des images est un défaut du clip (souvent un objet
    voisin inclus dans la sélection), pas une suite d'accidents : on la remonte en un seul avertissement
    et on ne garde par image que les autres raisons. Renvoie (flagged filtré, avertissements du clip)."""
    def is_frag(r):
        return any(p in r for p in FRAGMENT_PREFIXES)
    frag_frames = [f for f in flagged if any(is_frag(r) for r in f["reasons"])]
    if not stats or len(frag_frames) <= share * len(stats):
        return flagged, []
    mean_pct = float(np.mean([x.fragment_pct or x.islands_area_pct for x in stats]))
    warning = (f"Un morceau séparé du sujet principal persiste sur {len(frag_frames)} images sur {len(stats)} "
               f"(≈ {mean_pct:.1f} % du masque) : un objet voisin est probablement inclus. "
               "Ajoutez un point « Exclure » dessus puis propagez à nouveau.")
    kept = []
    for f in flagged:
        reasons = [r for r in f["reasons"] if not is_frag(r)]
        if reasons:
            kept.append({**f, "reasons": reasons})
    return kept, [warning]


def summarize(stats: list[FrameStats], flagged: list[dict]):
    ious = [x.iou_prev for x in stats[1:]] or [1.0]
    return {
        "frames": len(stats),
        "empty_frames": sum(1 for x in stats if x.area_pct == 0),
        "iou_mean": round(float(np.mean(ious)), 4),
        "iou_min": round(float(np.min(ious)), 4),
        "flagged": len(flagged),
        "islands_removed": sum(x.islands_removed for x in stats),
        "holes_filled": sum(x.holes_filled for x in stats),
    }


def stats_to_json(stats: list[FrameStats]):
    return [asdict(x) for x in sorted(stats, key=lambda x: x.frame)]
