# Étape 0 — rapport de faisabilité

**Projet :** videeo, studio local de transformation vidéo par IA ([brief](../docs/brief.md))
**Date :** 11 octobre 2026
**Machine :** MacBook Pro, Apple M5 Pro (GPU 20 cœurs), 64 Go de mémoire unifiée, macOS 26.6.2

## Verdict

**Le MVP est faisable en local sur cette machine, sans aucune API payante.** La chaîne complète a été testée de bout en bout sur 5 clips libres de droits : probe → proxy → sélection du sujet par boîte et points → masque SAM 2.1 propagé → décor généré localement → compositing → export MP4. Elle produit des résultats exploitables sur des clips courts.

Trois points conditionnent la suite :

1. **Il faut exécuter SAM 2 en demi-précision (bf16) sur MPS.** En fp32 on obtient 1,5–2,7 images/s ; en bf16 on monte à 6,3–7,5 images/s, avec des masques identiques à plus de 99 %.
2. **Le modèle *tiny* suffit pour le MVP.** *small* n'apporte pas de gain mesurable sur ces clips et n'est pas plus lent : le goulot n'est pas l'encodeur d'image.
3. **Les limites attendues se confirment.** Il n'y a pas de transparence dans les cheveux fins, l'éclairage ne correspond pas au décor, et un objet dans la boîte de départ (un pupitre) reste collé au sujet. La correction de masque par l'utilisateur doit donc faire partie du MVP.

## 1. Environnement

| Élément | Version / état |
|---|---|
| macOS / Xcode | 26.6.2 / Xcode 27.0 |
| FFmpeg | 9.0.1 (Homebrew), encodeurs `h264_videotoolbox`, `hevc_videotoolbox` et `prores_videotoolbox`, hwaccel `videotoolbox` |
| Filtres FFmpeg absents | `drawtext` (pas de libfreetype), `zscale`, `libplacebo` |
| Rust / Node | rustc 1.98.0, cargo 1.98.0 / Node 24.20.0 (prêts pour Tauri) |
| Python | uv 0.12.6 ; venvs isolés en Python 3.12 dans `.venvs/` |
| Vision | PyTorch 2.14.1, MPS disponible ; SAM 2 au commit `2b90b9f` |
| Image | diffusers 0.41.0, transformers 5.19.0 |
| Disque libre | ~425 Go |

Détails et empreintes SHA-256 des checkpoints : [results/environment.txt](results/environment.txt).

### Modèles et outils déjà présents sur le Mac (non modifiés)

- **Génération d'image (cache Hugging Face) :** FLUX.2 [klein] 4B (Apache-2.0, complet sauf un fichier de config de 1 Ko récupéré pendant le test), Z-Image-Turbo (snapshot incomplet, sans `model_index.json`), Qwen-Image-Edit (mflux q8), FIBO.
- **Vidéo générative :** Wan2.2-TI2V-5B, LTX-2.5 (versions MLX et LoRA), SeedVR2 3B (upscale vidéo, dans ComfyUI).
- **ComfyUI** 0.34.0 dans `~/ComfyUI` (son propre `.venv`, torch nightly), Comfy Desktop, nœud `ComfyUI-AppleSilicon-FP8`, modèles partagés dans `~/ComfyUI-Shared` (16 Go : VAE Wan 2.1, LoRA Wan2.2 lightx2v 4 étapes…).
- **LLM / VLM :** Ollama (qwen3-vl:30b, qwen3:8b, qwen3.5:35b-a3b, z-image-turbo…), llama.cpp (`llama-server`), MLX (Qwen3-VL-4B-Instruct-4bit, gemma-3-12b-it-4bit).
- **Draw Things** (FLUX.1, T5-XXL).

Ces modèles couvrent déjà les étapes 3 à 5 du brief (décor, planificateur LLM, vidéo générative) **sans nouveau téléchargement lourd**.

## 2. Clips de test

Ce sont des clips libres de droits Mixkit ([Stock Video Free License](https://mixkit.co/license/#videoFree)), téléchargés dans `clips/source/` (non versionnés). Les URLs, les prompts et les décalages sont dans [clips.json](clips.json). Chaque test porte sur 5 s, en proxy 1280×720.

| Id | Cas du brief | Contenu | Source |
|---|---|---|---|
| A_presenter | 1 | Présentatrice, caméra fixe | Mixkit 23385 (720p, 25 i/s) |
| B_office_gestures | 2 | Femme assise, gestes avec téléphone et lunettes | Mixkit 45924 (720p, 25 i/s) |
| C_walking_occlusion | 2 | Homme qui marche, occulté par des herbes, retire son sac | Mixkit 16156 (720p, 24 i/s) |
| D_long_hair | 3 | Cheveux longs écartés et en mouvement | Mixkit 16189 (720p, 29,97 i/s) |
| E_selfie_handheld | 4 | Selfie, caméra à main, bras levé | Mixkit 10442 (1080p, 23,976 i/s) |

Aucun clip source n'a de piste audio. L'audio et l'orientation ont donc été testés sur un clip « type iPhone » fabriqué pour l'occasion (voir §3).

## 3. Média : probe, orientation, audio, export

Script : [scripts/media_test.py](scripts/media_test.py). Résultats : [results/media_test.json](results/media_test.json).

Le clip test simule un iPhone : vidéo HEVC stockée en 1280×720 avec une **matrice de rotation de 90°** (affichage portrait 720×1280), plus de l'audio AAC 48 kHz, sur 8 s.

| Export MP4 H.264 + AAC | Temps (8 s de vidéo) | Taille | Orientation | Audio | Écart A/V | Erreurs de décodage |
|---|---|---|---|---|---|---|
| libx264 CRF 18, preset medium | 0,53 s | 2,67 Mo | 720×1280 ✔ | ✔ | 25 ms | aucune |
| libx264 CRF 20, preset veryfast | 0,28 s | 2,01 Mo | 720×1280 ✔ | ✔ | 25 ms | aucune |
| h264_videotoolbox q 65 | 0,53 s | 1,76 Mo | 720×1280 ✔ | ✔ | 25 ms | aucune |

Constats :
- FFmpeg applique la rotation automatiquement au réencodage : la sortie est en vrai portrait, sans tag de rotation. **La rotation doit être lue au probe et inscrite dans le manifeste du projet**, car un simple `width/height` est trompeur.
- Une métadonnée de rotation n'est conservée qu'en **remux sans réencodage** : avec `-display_rotation` sur une entrée, le premier essai a été tourné physiquement.
- L'écart A/V de 25 ms correspond au padding de l'encodeur AAC, une trame audio. Il est acceptable mais doit être vérifié sur de vrais fichiers iPhone (VFR, HDR Dolby Vision).
- En 720p, libx264 est aussi rapide que VideoToolbox. **VideoToolbox ne se justifiera qu'en 4K ou en HEVC** : il faudra le mesurer sur des sources 4K avant d'en faire le défaut.
- L'export atomique (`.part.mp4` puis renommage) est en place dans le script.

## 4. Segmentation vidéo : SAM 2.1

Script : [scripts/sam2_bench.py](scripts/sam2_bench.py). Résultats par run : [results/sam2/](results/sam2/). Comparaisons : `results/compare__*.json`.

- **Installation :** clone du dépôt officiel et `pip install -e` avec `SAM2_BUILD_CUDA=0`. Ça fonctionne sur MPS. L'extension CUDA `_C` est absente, donc le post-traitement de remplissage des trous est sauté (avertissement sans effet notable sur nos clips).
- **Checkpoints :** `sam2.1_hiera_tiny.pt` (156 Mo) et `sam2.1_hiera_small.pt` (184 Mo), licence Apache-2.0.
- **Prompt :** une boîte et 2–3 points positifs sur l'image 0, en coordonnées du proxy, fixés à la main dans `clips.json`.

### Vitesse et mémoire (proxy 1280×720, 120–150 images par clip)

| Variante | Vitesse de propagation | Mémoire MPS (pic driver) | RSS process |
|---|---|---|---|
| tiny fp32 | 1,45–2,74 i/s | 4,1–7,9 Go | 2,2–2,6 Go |
| small fp32 | 1,78–1,98 i/s | 4,1–7,9 Go | 2,3–2,6 Go |
| **tiny bf16** | **6,3–7,5 i/s** | 3,0–6,8 Go | 2,4–4,3 Go |
| small bf16 | 5,9–6,2 i/s | 3,4–6,8 Go | 2,4–2,8 Go |
| tiny CPU fp32 (clip A seul) | 1,12 i/s | — | 3,3 Go |

- Chargement du modèle : 0,2–1,4 s. `init_state` : 1,4–3 s pour 125 images.
- Le profilage montre que tout le temps est dans l'exécution GPU (synchronisée au `.cpu()`). Le surcoût Python est négligeable. En fp32, le débit baisse au fil des images (3,4 → 2,3 i/s) à mesure que la mémoire de suivi grossit.
- **Extrapolation, non mesurée :** un clip de 30 s à 30 i/s (900 images) prendrait ~2–2,5 min en tiny bf16. SAM 2 charge toutes les images dans `init_state` (~12 Mo par image en 1024² fp32), soit ~11 Go pour 900 images. **Au-delà de ~20 s, il faudra `offload_video_to_cpu` ou un traitement par segments** : à tester en priorité à l'étape 2.

### Qualité des masques

Métriques automatiques (tiny bf16) : aucune image vide sur aucun clip.

| Clip | IoU entre images consécutives (moy. / min) | Images < 0,9 | Observation visuelle |
|---|---|---|---|
| A_presenter | 0,993 / 0,972 | 0 | Très stable, mais **le pied du pupitre est inclus** (il était dans la boîte de départ) |
| B_office_gestures | 0,985 / 0,946 | 0 | Mains, téléphone et lunettes suivis ; tiny et small diffèrent (IoU 0,93) sur la frontière bureau/bras |
| C_walking_occlusion | 0,929 / 0,837 | 29 | Suivi complet malgré les herbes et le sac retiré ; IoU plus bas car la marche est réelle, pas un défaut |
| D_long_hair | 0,957 / 0,799 | 18 | Cheveux écartés captés comme une masse ; **pas de transparence entre les mèches** (halo gris du fond) |
| E_selfie_handheld | 0,973 / 0,941 | 0 | Bras levé et téléphone inclus ; caméra mobile sans dérive |

**bf16 vs fp32 :** IoU moyen de 0,993 à 0,9996 selon les clips, minimum 0,954 sur une seule image. **CPU vs MPS fp32 :** identiques (IoU 1,0).

Aperçus : [masks_tiny_overview.jpg](results/masks_tiny_overview.jpg), [masks_hair_selfie_overview.jpg](results/masks_hair_selfie_overview.jpg).

## 5. Génération de décor : FLUX.2 [klein] 4B

Script : [scripts/image_bench.py](scripts/image_bench.py). Résultats : [results/image_bench.json](results/image_bench.json).

- Le modèle est déjà en cache, chargé via `diffusers.Flux2KleinPipeline` en bf16 sur MPS, en mode hors ligne. Seul `scheduler_config.json` (1 Ko) manquait et a été récupéré.
- Chargement : 6 s. **Génération en 4 étapes : 6,2 s en 768×432 et ~10,5 s en 1280×720.**
- **Pic mémoire MPS : ~24 Go** (transformer 4B et encodeur texte Qwen3 en bf16). Ce n'est pas compatible avec SAM 2 et un LLM chargés en même temps : il faudra **décharger explicitement les modèles**, comme le prévoit le brief.
- Qualité : deux décors photoréalistes et cohérents ([loft](results/backgrounds/studio_loft_1280x720.png), [terrasse alpine](results/backgrounds/mountain_terrace_1280x720.png)).

## 6. Compositing et export

Script : [scripts/composite_test.py](scripts/composite_test.py). Rendus : `results/composites/` (non versionnés). Images extraites : `results/report_assets/`.

Le graphe FFmpeg enchaîne : masque PNG, `boxblur` 1,5 px (adoucissement), `alphamerge` sur la source, `overlay` sur le décor recadré en 1280×720, puis H.264. La vue de diagnostic 2×2 montre la source, le masque, le sujet isolé sur damier et le composite.

- **Temps : 0,57–0,69 s pour 5 s de vidéo 720p**, soit beaucoup plus vite que la segmentation. Inutile d'envisager Metal pour l'instant.
- Défauts observés, conformes aux « principes de vérité » du brief :
  - **Éclairage et couleur :** le sujet garde la lumière de studio ou de bureau, ce qui se voit sur un décor au coucher de soleil. Les réglages couleur simples de l'étape 2 seront nécessaires.
  - **Échelle et position :** le sujet est posé tel quel. Le contrôle d'échelle et de position de l'utilisateur est indispensable (la présentatrice paraît géante).
  - **Contours :** halo léger. La décontamination de couleur de bord et le réglage du feather sont à prévoir.
  - **Cheveux :** le fond gris d'origine reste visible entre les mèches. Un modèle de matting serait utile, mais après le MVP.

## 7. Recommandations pour la suite

| Décision | Recommandation |
|---|---|
| Segmentation | **SAM 2.1 tiny, bf16, MPS**, en worker Python persistant (le modèle reste chargé en ~2,4 Go). Garder *small* comme option « Qualité » seulement si un clip réel le justifie. |
| Proxy | 720p pour l'aperçu et la segmentation, masque remis à l'échelle pour le rendu final. Valider sur une source 4K iPhone. |
| Clips longs | Tester `offload_video_to_cpu` et la segmentation par blocs avec ré-amorçage à partir du dernier masque, avant le MVP. |
| Correction utilisateur | Clics positifs et négatifs sur une image clé, puis re-propagation : nécessaire dès le MVP (cas du pupitre). |
| Rendu / export | FFmpeg CLI : libx264 par défaut, VideoToolbox en option après mesure en 4K. Rotation lue au probe et stockée dans le manifeste. |
| Décor | FLUX.2 klein 4B déjà présent (Apache-2.0), derrière un adaptateur. Charger à la demande et décharger avant SAM 2. ComfyUI est disponible mais non nécessaire pour un décor fixe. |
| LLM (étape 4) | Ollama ou MLX déjà installés (qwen3:8b, Qwen3-VL-4B) : aucun téléchargement à prévoir. |
| Mémoire | Un seul modèle lourd à la fois ; budget observé : SAM 2 ≤ 7 Go, FLUX.2 klein ≈ 24 Go. |

### Prochaine étape concrète (étape 1 — MVP manuel)

1. Squelette Tauri 2 + Vue 3 + noyau Rust, avec manifeste `project.json` versionné.
2. Worker vision en JSON Lines sur stdin/stdout : `load_model`, `init_video`, `add_prompt`, `propagate` (événements de progression), `unload`.
3. Écran : import, probe, choix de l'extrait, clic ou boîte sur l'image 0, aperçu du masque, puis propagation.
4. Décor importé ou généré, curseurs d'échelle et de position, compositing FFmpeg, export MP4.

### À tester avant ou pendant l'étape 1

- De vrais fichiers iPhone : HEVC 4K, VFR, HDR et audio réel. **Fournis-en 2–3 si possible.**
- Un clip de 30–60 s, pour mesurer la mémoire et la dérive dans le temps.
- La correction par points négatifs (le pupitre du clip A).

## 8. Reproduire

Voir le [README](../README.md). Les poids, les clips, les venvs et le cache sont exclus de Git (`.gitignore`). Leur reconstruction est décrite dans le README et dans `clips.json`.
