# videeo — studio local de transformation vidéo par IA

Application macOS locale (Apple Silicon) : détourage vidéo, remplacement de décor, compositing et export,
sans coût d'API par défaut. Spécification complète : [docs/brief.md](docs/brief.md).

## État

**Étape 0 — preuve technique sur le Mac.** Rapport : [stage0/REPORT.md](stage0/REPORT.md) (version web : [stage0/web/index.html](stage0/web/index.html)).

## Reproduire l'étape 0

```bash
# Environnement vision (PyTorch + SAM 2.1)
git clone --depth 1 https://github.com/facebookresearch/sam2.git vendor/sam2
uv venv --python 3.12 .venvs/vision
SAM2_BUILD_CUDA=0 uv pip install --python .venvs/vision/bin/python torch torchvision -e vendor/sam2 psutil opencv-python-headless
mkdir -p models/sam2 && cd models/sam2 && for m in tiny small; do curl -LO https://dl.fbaipublicfiles.com/segment_anything_2/092824/sam2.1_hiera_$m.pt; done; cd -

# Environnement image (diffusers, modèle déjà présent dans le cache Hugging Face)
uv venv --python 3.12 .venvs/image
uv pip install --python .venvs/image/bin/python torch diffusers transformers accelerate psutil

# Clips de test : voir stage0/clips.json (URLs Mixkit) -> clips/source/mixkit-<id>.mp4

python3 stage0/scripts/media_test.py
.venvs/vision/bin/python stage0/scripts/sam2_bench.py --model tiny --device mps
.venvs/vision/bin/python stage0/scripts/sam2_bench.py --model small --device mps
.venvs/image/bin/python stage0/scripts/image_bench.py
python3 stage0/scripts/composite_test.py --model small --background stage0/results/backgrounds/studio_loft_1280x720.png
```
