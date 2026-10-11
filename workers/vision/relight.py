"""Normales approchées du sujet, à partir du seul masque, pour le relighting directionnel.

Modèle de forme volontairement simple et stable d'une image à l'autre (il ne dépend que du masque,
déjà nettoyé et contrôlé) :
- chaque segment horizontal du masque (tronc, bras, jambe…) est vu comme un cylindre vertical :
  nx = u, nz = √(1 − u²), u allant de −1 (bord gauche) à +1 (bord droit) ;
- la composante verticale vient de la pente d'un masque très adouci (le haut de la tête et des
  épaules regarde vers le haut, le dessous du menton vers le bas) ;
- le tout est lissé puis normalisé.

Encodage PNG RGB (comme une « normal map ») : c = (N + 1) / 2 × 255, axe y vers le bas.
L'app en déduit l'ombrage N·L par une combinaison linéaire des canaux, identique dans l'aperçu
(compositor.ts) et l'export (colorchannelmixer, media.rs).
"""
import numpy as np


def normals_from_mask(mask: np.ndarray) -> np.ndarray:
    """mask : booléen (h, w). Renvoie (h, w, 3) float32, normales unitaires, (0, 0, 1) hors du sujet."""
    import cv2

    h, w = mask.shape
    m = mask.astype(np.uint8)
    # Segments horizontaux, vectorisés : pour chaque pixel, début (a) et fin (b, exclue) de son
    # segment, par cumul des positions de début vers la droite et de fin vers la gauche.
    xs = np.broadcast_to(np.arange(w, dtype=np.int32), (h, w))
    left_edge = np.zeros((h, w), bool)
    left_edge[:, 0] = mask[:, 0]
    left_edge[:, 1:] = mask[:, 1:] & ~mask[:, :-1]
    right_edge = np.zeros((h, w), bool)
    right_edge[:, -1] = mask[:, -1]
    right_edge[:, :-1] = mask[:, :-1] & ~mask[:, 1:]
    a = np.maximum.accumulate(np.where(left_edge, xs, 0), axis=1)
    b = np.minimum.accumulate(np.where(right_edge, xs, w)[:, ::-1], axis=1)[:, ::-1] + 1
    half = (b - a) / 2.0
    nx = np.where(mask & (b - a >= 3), (xs - (a + b - 1) / 2.0) / np.maximum(half, 1), 0).astype(np.float32)
    # Lissage vertical : supprime le crénelage ligne à ligne des contours.
    sigma = max(2.0, 0.006 * h)
    nx = cv2.GaussianBlur(nx, (0, 0), sigmaX=sigma * 0.5, sigmaY=sigma, borderType=cv2.BORDER_REPLICATE)
    # Bord répliqué : un sujet coupé par le cadre (bas du corps hors champ) ne doit pas « tourner »
    # vers le bas au bord de l'image.
    soft = cv2.GaussianBlur(m.astype(np.float32), (0, 0), sigmaX=0.02 * h, borderType=cv2.BORDER_REPLICATE)
    gy = cv2.Sobel(soft, cv2.CV_32F, 0, 1, ksize=3)
    # Sobel 3×3 = 8 × la dérivée ; pente maximale d'un masque adouci ≈ 1 / (σ·√(2π)) ≈ 0,4 / σ :
    # ramenée vers [-1, 1]. Le masque croît vers le bas au sommet du sujet : normale vers le haut (−y).
    ny = -np.clip(gy / 8 * (0.02 * h) * 2.5, -1, 1) * 0.7
    nx = np.clip(nx, -0.97, 0.97)
    nz = np.sqrt(np.clip(1 - nx ** 2 - ny ** 2, 0.05, 1))
    n = np.stack([nx, ny, nz], axis=-1)
    n /= np.linalg.norm(n, axis=-1, keepdims=True)
    n[~mask] = (0, 0, 1)
    return n.astype(np.float32)


def encode(n: np.ndarray) -> np.ndarray:
    """Normales → image RGB uint8 (ordre R, G, B = x, y, z)."""
    return np.clip(np.round((n + 1) * 127.5), 0, 255).astype(np.uint8)
