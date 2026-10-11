"""Tests des normales approchées : .venvs/vision/bin/python -m pytest workers/vision/test_relight.py"""
import numpy as np

from relight import encode, normals_from_mask


def disc_mask(h=200, w=300, cut_bottom=False):
    yy, xx = np.mgrid[:h, :w]
    m = ((xx - 150) / 60.0) ** 2 + ((yy - 100) / 80.0) ** 2 < 1
    if cut_bottom:
        m[100:] = np.abs(xx[100:] - 150) < 60
    return m


def test_unit_length_and_background():
    m = disc_mask()
    n = normals_from_mask(m)
    assert np.allclose(np.linalg.norm(n, axis=-1), 1, atol=1e-4)
    assert np.allclose(n[~m], (0, 0, 1))


def test_cylinder_orientation():
    n = normals_from_mask(disc_mask())
    # Côté gauche orienté vers la gauche, côté droit vers la droite, centre face caméra.
    assert n[100, 100, 0] < -0.5 and n[100, 200, 0] > 0.5
    assert abs(n[100, 150, 0]) < 0.05 and n[100, 150, 2] > 0.9
    # Haut du sujet orienté vers le haut (y vers le bas : composante négative).
    assert n[25, 150, 1] < -0.2


def test_cut_by_frame_does_not_turn_down():
    n = normals_from_mask(disc_mask(cut_bottom=True))
    # Au bord bas de l'image, le sujet continue hors champ : pas de normale vers le bas.
    assert abs(n[199, 150, 1]) < 0.05


def test_encode_range():
    e = encode(np.array([[[-1, 0, 1]]], np.float32))
    assert e.dtype == np.uint8 and tuple(e[0, 0]) == (0, 128, 255)
