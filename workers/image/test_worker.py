"""Tests du worker image sans charger de modèle : .venvs/image/bin/python -m pytest workers/image -q"""
from pathlib import Path

import pytest

from worker import ImageWorker


@pytest.fixture
def w(tmp_path):
    return ImageWorker(tmp_path)


@pytest.mark.parametrize("params,msg", [
    ({"prompt": "", "width": 512, "height": 512}, "prompt"),
    ({"prompt": "x" * 2001, "width": 512, "height": 512}, "prompt"),
    ({"prompt": "lac", "width": 1000, "height": 512}, "dimensions"),
    ({"prompt": "lac", "width": 4096, "height": 512}, "dimensions"),
    ({"prompt": "lac", "width": 512, "height": 512, "steps": 0}, "étapes"),
])
def test_rejects_invalid_requests_before_loading(w, tmp_path, params, msg):
    with pytest.raises(ValueError, match=msg):
        w.op_generate({**params, "out_png": str(tmp_path / "a.png")}, "r1")
    assert w.pipe is None, "aucun modèle ne doit être chargé pour une requête invalide"


def test_rejects_paths_outside_root(w):
    with pytest.raises(ValueError, match="hors du répertoire"):
        w.op_generate({"prompt": "lac", "width": 512, "height": 512, "out_png": "/tmp/evil.png"}, "r1")
    assert w.pipe is None
