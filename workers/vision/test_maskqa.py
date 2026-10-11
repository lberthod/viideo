"""Tests du nettoyage et de la détection de dérive : .venvs/vision/bin/python -m pytest workers/vision -q"""
import numpy as np

from maskqa import FrameStats, clean_mask, detect_drift, frame_stats


def disk(h, w, cx, cy, r):
    yy, xx = np.mgrid[:h, :w]
    return (xx - cx) ** 2 + (yy - cy) ** 2 <= r * r


def test_clean_removes_islands_and_fills_holes():
    m = disk(100, 100, 50, 50, 30)
    m[10:12, 90:92] = True        # îlot de 4 px
    m[48:51, 48:51] = False       # trou de 9 px au centre
    c, removed, filled, area = clean_mask(m, min_island_ratio=0.05, max_hole_ratio=0.002)
    assert removed == 1 and filled == 1 and area == 4
    assert not c[10, 90] and c[49, 49]


def test_clean_keeps_big_hole_and_border_background():
    m = disk(100, 100, 50, 50, 40)
    m[30:70, 30:70] = False       # grand trou (1600 px > 0,2 %) : un vrai vide, on le garde
    c, _, filled, _ = clean_mask(m)
    assert filled == 0 and not c[50, 50]


def test_drift_flags_empty_and_jump_but_not_smooth_motion():
    stats, prev = [], None
    for i in range(30):
        cx = 30 + i  # déplacement régulier : ne doit pas être signalé
        m = disk(100, 100, cx, 50, 15) if i != 20 else np.zeros((100, 100), bool)
        if i == 10:
            m = disk(100, 100, 85, 20, 15)  # saut brutal
        stats.append(frame_stats(i, m, prev))
        prev = m
    flagged = {f["frame"]: f for f in detect_drift(stats)}
    assert 20 in flagged and "masque vide" in flagged[20]["reasons"]
    assert 10 in flagged
    assert all(f in (10, 11, 20, 21) for f in flagged), flagged.keys()


def test_drift_on_empty_series():
    assert detect_drift([]) == []
    assert detect_drift([FrameStats(0, 10.0, 1, 1.0, 0.5, 0.5)]) == []


def test_drift_flags_heavy_cleanup():
    m = disk(100, 100, 50, 50, 20)
    stats = [frame_stats(i, m, m if i else None) for i in range(10)]
    stats[4].islands_removed, stats[4].islands_area_pct = 12, 4.0
    stats[6].islands_removed, stats[6].islands_area_pct = 12, 0.2  # poussière de mèches : pas signalé
    flagged = {f["frame"]: f for f in detect_drift(stats)}
    assert list(flagged) == [4] and flagged[4]["reasons"][0].startswith("12 îlots parasites retirés")


def test_fragmentation_weighted_by_area():
    m = disk(200, 200, 100, 100, 50)
    m[5:7, 5:7] = True  # 4 px détachés : négligeable
    assert detect_drift([frame_stats(0, m, None)]) == []
    m[150:190, 10:50] = True  # gros morceau détaché : signalé
    flagged = detect_drift([frame_stats(0, m, None)])
    assert flagged and "morceaux séparés" in flagged[0]["reasons"][0]


def test_fast_motion_with_stable_area_is_not_a_rupture():
    # Un « bras » qui change de côté d'une image à l'autre : IoU faible, aire constante.
    stats, prev = [], None
    for i in range(20):
        m = disk(120, 120, 60, 60, 20)
        if i % 2:
            m[55:65, 80:115] = True
        else:
            m[55:65, 5:40] = True
        stats.append(frame_stats(i, m, prev))
        prev = m
    assert detect_drift(stats) == []


def test_persistent_fragment_becomes_one_clip_warning():
    from maskqa import split_persistent
    stats, prev = [], None
    for i in range(20):
        m = disk(200, 200, 100, 100, 50)
        m[150:190, 10:50] = True  # objet voisin inclus sur toutes les images
        if i == 12:
            m = disk(200, 200, 100, 100, 10)  # effondrement réel
        stats.append(frame_stats(i, m, prev))
        prev = m
    flagged, warnings = split_persistent(stats, detect_drift(stats))
    assert len(warnings) == 1 and "persiste sur" in warnings[0]
    assert [f["frame"] for f in flagged] == [12, 13], flagged
