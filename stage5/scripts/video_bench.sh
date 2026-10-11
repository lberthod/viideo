#!/bin/zsh
# Étape 5 — banc des moteurs vidéo locaux sur le cas « décor animé » (image fixe -> plan de 5 s, caméra fixe).
# Usage : stage5/scripts/video_bench.sh [sulphur2|ltx25|wan22|kenburns|all]
# Les commandes reprennent celles validées dans ~/ai-video/creation_video.md.
set -u
AI=$HOME/ai-video
ROOT=${0:A:h:h:h}
OUT=$ROOT/stage5/results/clips
STILL=$ROOT/stage5/results/still_1024x576.png
PROMPT="Locked camera, no camera movement. The alpine lake surface ripples gently, a few clouds drift slowly across the sky, grass and flowers sway in a light breeze. Preserve every detail, material, and lighting exactly, motion slow and continuous."
SEED=42001
mkdir -p $OUT
which=${1:-all}

run() { # nom, commande…
  local name=$1; shift
  echo "== $name"
  /usr/bin/time -l "$@" > $OUT/$name.log 2> $OUT/$name.time
  echo "exit=$? $(grep -E 'real|peak memory footprint|maximum resident' $OUT/$name.time | tr '\n' ' ')"
}

# Sulphur-2 (LTX-2.3 distillé, q4) : 5 s à 24 i/s = 121 images (grille 8k+1).
[[ $which == all || $which == sulphur2 ]] && run sulphur2 $AI/ltx-2-mlx/.venv/bin/ltx-2-mlx generate \
  --image $STILL --prompt "$PROMPT" --distilled --model $AI/models/sulphur-2-distill-mlx-q4 \
  --frame-rate 24 -H 576 -W 1024 -f 121 --seed $SEED -o $OUT/sulphur2.mp4

# LTX-2.5 (q8), via le fork xocialize (sinon retombée silencieuse sur Gemma-3, cf. guide §9).
[[ $which == all || $which == ltx25 ]] && run ltx25 $AI/ltx-2-mlx-xocialize/.venv/bin/ltx-2-mlx generate \
  --image $STILL --prompt "$PROMPT" --distilled --model $AI/models/ltx-2.5-mlx-q8 \
  --frame-rate 24 -H 576 -W 1024 -f 121 --seed $SEED -o $OUT/ltx25.mp4

# Wan 2.2 : retiré (trop lent sur Mac, cf. workflows/video/engines.json « excluded »).
[[ $which == all || $which == wan22 ]] && run wan22 $AI/wan-venv/bin/python -m mlx_video.models.wan_2.generate \
  --model-dir $AI/models/wan22-ti2v-5b-mlx-q4 --image $STILL --prompt "$PROMPT" \
  --width 1024 --height 576 --num-frames 49 --seed $SEED --output-path $OUT/wan22.mp4

# Ken Burns : zoom lent et déterministe (aucun modèle).
[[ $which == all || $which == kenburns ]] && run kenburns ffmpeg -v error -y -loop 1 -i $STILL \
  -vf "scale=4096:2304,zoompan=z='min(zoom+0.0004,1.05)':x='iw/2-(iw/zoom/2)':y='ih/2-(ih/zoom/2)':d=120:s=1024x576:fps=24" \
  -frames:v 120 -c:v libx264 -pix_fmt yuv420p -crf 18 $OUT/kenburns.mp4
