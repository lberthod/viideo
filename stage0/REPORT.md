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

## 9. Compléments : clips verticaux et clip long

Ce complément comble les deux manques listés en §7 : des images « type téléphone » (vertical 9:16, 4K, haute cadence) et un clip long pour mesurer la mémoire, la vitesse et la dérive de SAM 2 dans le temps. Toutes les mesures sont en **SAM 2.1 tiny, bf16, MPS**.

> **Mesures bruitées.** Un build Rust/Tauri et un autre processus Python de vision tournaient en parallèle (load average entre 10 et 30). Les vitesses sont donc indicatives, à ±30 % près. La mémoire et la qualité des masques ne sont pas affectées.

### Clips ajoutés

Tous les clips sont sous [Mixkit Stock Video Free License](https://mixkit.co/license/#videoFree), vérifiée sur la page de chaque clip. Ils sont téléchargés dans `clips/source/` (280 Mo). La configuration est dans [clips_phase2.json](clips_phase2.json), avec les prompts en coordonnées proxy et une durée par clip (`seconds`).

| Id | Source | Format natif | Durée testée | Proxy | Contenu |
|---|---|---|---|---|---|
| V1_vertical_4k60 | [Mixkit 28311](https://mixkit.co/free-stock-video/girl-sending-a-selfie-by-cell-phone-on-a-green-28311/) | H.264 2160×3840, 59,94 i/s | 13,8 s (825 images) | 406×720 | Femme qui manipule son téléphone, fond vert |
| V2_vertical_selfie | [Mixkit 42323](https://mixkit.co/free-stock-video/portrait-of-an-influencer-talking-to-the-camera-42323/) | H.264 1080×1920, 23,976 i/s | 16,9 s (404 images) | 406×720 | Influenceuse face caméra, cheveux bouclés, gestes, fond chargé |
| L_breakdance_40s | [Mixkit 452](https://mixkit.co/free-stock-video/man-breakdancing-452/) | H.264 3840×2160, 23,976 i/s | 40,0 s (959 images) | 1280×720 | Breakdance en plan continu : poiriers, rotations, cheveux |

Choix et limites :
- **Clip long :** les clips Mixkit de plus de 30 s montrant une personne sont presque tous sous *Restricted License*. Le seul plan continu de 30 s ou plus sous licence libre trouvé est le n° 452 (40 s, aucune coupe détectée par `select=scene>0.3`). Ce mouvement est un cas de stress, plus dur qu'un vlog face caméra.
- **Pas de vrai fichier iPhone :** les trois sources sont en H.264 SDR, sans audio ni rotation, et à cadence constante. Le HEVC, le HDR, le VFR et l'audio réel restent **à tester sur de vrais fichiers de téléphone**. Pexels refuse l'accès scripté (HTTP 403).

### Média : probe, proxy et export 4K

Script : [scripts/media_phase2.py](scripts/media_phase2.py). Résultats : [results/media_phase2.json](results/media_phase2.json).

| Clip | Proxy 720p, 10 s : logiciel / `-hwaccel videotoolbox` | Export 5 s natif : libx264 CRF 18 medium | h264_videotoolbox | hevc_videotoolbox |
|---|---|---|---|---|
| V1 (4K60 vertical) | 0,96 s / 3,09 s | 15,6 s (9,7 Mo) | 4,7 s (9,3 Mo) | 4,9 s (6,3 Mo) |
| V2 (1080p vertical) | 0,35 s / 1,13 s | 2,2 s | 0,8 s | 0,8 s |
| L (4K24) | 0,82 s / 1,94 s | 6,9 s (13,3 Mo) | 2,1 s (4,2 Mo) | 2,1 s (2,7 Mo) |

- **En 4K, VideoToolbox est 3 à 3,5 fois plus rapide que libx264**, avec un débit d'environ 1× le temps réel en 4K60. HEVC VideoToolbox produit des fichiers 35 % plus légers, à vitesse égale. Pour l'export en résolution native depuis une source 4K, il faut **VideoToolbox par défaut** (cf. §3). La qualité visuelle à q 65 reste à vérifier.
- Pour extraire le proxy, le **décodage logiciel est plus rapide** que `-hwaccel videotoolbox`, à cause du rapatriement des images avant `scale`. Il faut garder le décodage logiciel pour les proxys.
- Les dimensions des vidéos verticales sont stockées telles quelles (2160×3840, rotation 0). Le probe gère les deux cas, mais il faut quand même tester une vraie vidéo iPhone avec matrice de rotation.

### SAM 2 sur les clips verticaux

| Clip | Images | Vitesse (i/s) | Pic MPS | Pic RSS | IoU consécutif moy. / min | Images < 0,9 |
|---|---|---|---|---|---|---|
| V1_vertical_4k60 | 825 | 7,6 (run 1, charge modérée) ; 3,7 (run 2, load ~30) | 21,0 Go | 11,0 Go | 0,996 / 0,962 | 0 |
| V2_vertical_selfie | 404 | 4,6 (processus dédié) ; 3,2 (run 1, après V1) | 10,4 Go | 5,7 Go | 0,984 / 0,891 | 1 |

- Fichiers : `results/sam2/V*_bf16.json` (processus dédié) et `*__run1_same_process.json` (premier passage, trois clips dans un même processus).
- **Format vertical :** aucun problème. Le proxy 406×720 passe tel quel dans SAM 2, qui redimensionne en 1024², et les prompts se donnent en coordonnées proxy.
- **V1 :** le masque est très propre sur fond vert, sans dérive sur 825 images. Le téléphone tenu en main est en général exclu du masque. Il faudra un clic positif si on veut l'inclure.
- **V2 :** les cheveux bouclés sont suivis comme une masse, et le fond (boîtes de céréales) reste visible entre les boucles, comme pour D_long_hair. La main qui passe devant le corps est bien incluse. On observe jusqu'à 21 petites composantes sur une image : des îlots dans les cheveux.
- **4K60 → 60 i/s :** traiter toutes les images d'un clip à 60 i/s double le coût par seconde de vidéo. Pour l'aperçu, on pourrait segmenter à 30 i/s puis interpoler ou dupliquer les masques. C'est à décider à l'étape 1.

### SAM 2 sur le clip long (40 s, 959 images, 1280×720)

Le clip a été lancé dans des processus séparés pour isoler la mémoire. Résumé : [results/long_clip_summary.json](results/long_clip_summary.json).

| Variante | `init_state` | Vitesse (i/s) | Vitesse par fenêtre de 100 images | Pic MPS (driver) | MPS en régime (début → fin) | Pic RSS |
|---|---|---|---|---|---|---|
| GPU (sans offload) | 9,6 s | 5,28 | 4,7–6,0 | **24,3 Go** | 13,4 → 14,1 Go | 12,7 Go |
| `offload_video_to_cpu` | 13,2 s | 4,36 (load ~28) | 3,0–5,3 | **3,1 Go** | 1,4 → 3,1 Go | 13,7 Go |
| `offload_video` + `offload_state` | 11,9 s | 5,39 | 4,4–6,2 | **2,4 Go** | 1,3 → 2,4 Go | 14,2 Go |
| GPU, même processus après V1 et V2 | 9,8 s | 5,06 | 3,7–5,7 | 36,6 Go | 24,5 Go | 13,1 Go |

**Masques :** ils sont identiques avec et sans offload (IoU 1,0 sur les 959 images, `results/compare__masks_tiny_mps_bf16__vs__*offload*.json`). Aucune image n'est vide. L'IoU consécutif vaut 0,848 en moyenne et 0,599 au minimum, avec 676 images sous 0,9.

**Dérive : aucune dérive visible.** Les images 0, 120, 240 … 958 ont été vérifiées en superposition. Le danseur est suivi entier, y compris en poirier, en rotation, cheveux compris, et jusqu'à la dernière image, sans fuite sur le mur ni sur les ombres. L'IoU consécutif bas ne vient pas d'une dérive : il varie avec l'intensité du mouvement (0,75 à 0,95 selon les fenêtres) et ne décroît pas avec le temps. Les pires images (163, 235) sont des jambes et des bras qui se déplacent fortement entre deux images à 24 i/s, avec un masque correct. Extraits : `results/report_assets/L_breakdance_diag_{2,20,38}s.jpg`.

**Vitesse dans le temps :** il n'y a pas de ralentissement systématique en bf16 sur 959 images, et les variations suivent la charge de la machine. Sur V1 (run 1), on voit une baisse de 8,4 à 6,1 i/s sur 825 images, mais elle ne se reproduit pas proprement sous charge. À surveiller.

**Mémoire, ce qui compte vraiment :**
1. **Le coût mémoire dépend du nombre d'images, pas de la résolution du proxy.** SAM 2 stocke chaque image en 1024×1024 fp32, soit environ 12,6 Mo par image. Pour 959 images, cela fait 13,4 Go sur le GPU en régime et 24 Go en pic pendant le chargement. V1 (825 images verticales 406×720) atteint aussi 21 Go.
2. **`offload_video_to_cpu` règle le problème côté GPU** (3 Go au lieu de 24 Go), sans perte de qualité ni coût de vitesse mesurable dans le bruit. `offload_state_to_cpu` gagne encore 0,7 Go.
3. **L'offload ne règle rien côté RSS.** Le chargeur de SAM 2 décode d'abord toutes les images en fp32 sur CPU (~12,7 Go pour 40 s à 24 i/s). En mémoire unifiée, c'est la même RAM. Il faut extrapoler à **60 s à 60 i/s (3 600 images) : ~45 Go**, ce qui est intenable sur 64 Go avec le reste.
4. La mémoire de suivi croît peu : +0,7 Go sur 959 images.
5. Le cache MPS n'est pas rendu entre deux clips dans un même processus (36,6 Go de pic au 3ᵉ clip). Le worker persistant devra appeler `torch.mps.empty_cache()` après chaque `propagate` ou `reset_state`.

Incident : le premier essai `--offload --offload-state`, enchaîné dans une boucle shell, n'a pas écrit de JSON. Sa sortie avait été filtrée et la cause n'a pas été identifiée. Relancé seul, il a fonctionné. Aucun OOM n'a été observé.

### Compositing

Sorties : `results/composites/{V1,V2,L}_*__tiny_bf16*.mp4` et `results/composite_tiny_bf16_clips_phase2.json`.

Les temps sont de 1,3 s (V1, 825 images), 1,7 s (V2) et 6,6 s (L, 40 s). La sortie suit maintenant la taille du proxy : 406×720 en vertical. Le décor 16:9 est recadré au centre.

**Bug corrigé dans `composite_test.py` :** l'image de décor bouclée entrait à 25 i/s par défaut, et `overlay` imposait cette cadence. Les sorties étaient donc rééchantillonnées à 25 i/s, avec 344 images au lieu de 825 pour V1. Le décor reçoit maintenant `-framerate` = cadence de la source. **Les composites A–E du §6 ont été produits avant la correction** : ils sont à 25 i/s, avec des images dupliquées ou sautées pour C, D et E. Il faut les régénérer si on les réutilise.

### Modifications des scripts

Elles sont toutes rétrocompatibles : sans option, le comportement est celui de §4–6, sauf la correction de cadence.
- `sam2_bench.py` : options `--config` (défaut `stage0/clips.json`), `--offload` (`offload_video_to_cpu`) et `--offload-state` (`offload_state_to_cpu`). Le champ `seconds` par clip est optionnel, avec `bench_seconds` global par défaut. Le JSON contient maintenant `windows` : vitesse, IoU, aire et mémoire par fenêtre de 100 images. Suffixes de fichiers : `__offload`, `__offstate`.
- `composite_test.py` : options `--config` et `--variant` (dossier de masques `masks_<modèle>_<device>_<variant>`). La sortie prend la taille du proxy, et la cadence du décor est corrigée.
- Nouveau script `media_phase2.py` : probe et mesures de décodage et d'encodage 4K.

### Recommandation mise à jour pour les clips longs

| Durée × cadence | Stratégie |
|---|---|
| ≤ ~400 images (≤ 15 s à 24–30 i/s) | Tout en mémoire, sur GPU ou avec `offload_video_to_cpu`, ~5 Go de RAM |
| 400 à ~1 000 images (≤ 40 s à 24 i/s) | **`offload_video_to_cpu=True` + `offload_state_to_cpu=True` par défaut** : GPU ≤ 3 Go, RSS ~13 Go. Mesuré, sans perte. |
| > ~1 000 images (60 s à 30 i/s, toute vidéo 60 i/s longue) | **Segmentation par blocs** de 300 à 500 images : `init_state` par bloc, ré-amorçage du bloc suivant avec `add_new_mask` sur le dernier masque (ou les 2–3 derniers), chevauchement de quelques images pour contrôler la continuité. RAM bornée à ~4–6 Go quel que soit la durée. |

À faire aussi pour le worker de l'étape 1 :
- Remplacer le chargeur d'images de SAM 2 par un chargeur paresseux (uint8 sur CPU, normalisation à la volée), qui supprimerait le pic RSS fp32.
- Proposer une segmentation à cadence réduite (30 i/s) pour les sources à 60 i/s.
- Appeler `empty_cache` entre les clips.
- Afficher la progression par fenêtres, car le temps total d'un clip de 40 s est d'environ 3 min.
