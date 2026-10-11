# Brief projet — studio local de transformation vidéo par IA

**Document de transmission à l’agent de développement**  
**Matériel cible :** MacBook Pro M5 Pro, mémoire unifiée 64 Go, Apple Silicon  
**Date de cadrage :** 11 octobre 2026  
**Statut :** vision et spécification initiale ; aucune implémentation n’est présumée

---

## 1. Résumé du projet

Construire une application macOS locale qui prend une courte vidéo filmée au téléphone, aide l’utilisateur à choisir ce qu’il veut changer, puis applique un pipeline adapté : détourage vidéo, remplacement ou transformation du décor, intégration du sujet, cohérence temporelle, aperçu et export.

Le produit n’est pas un clone de Kling ou Runway et ne doit pas commencer par générer chaque image d’une vidéo. Sa proposition est d’obtenir un résultat utile avec un coût d’API nul par défaut, en combinant des opérations déterministes peu coûteuses (décodage, masques, suivi, compositing, couleur) avec des modèles locaux seulement là où ils apportent une vraie amélioration. La génération vidéo lourde reste une option expérimentale, locale si possible ou distante si l’utilisateur l’active explicitement.

### Promesse réaliste du premier prototype

Pour une vidéo courte d’une personne filmée devant un arrière-plan simple, permettre de sélectionner la personne, de la détourer, de placer derrière elle une image de décor, de régler l’échelle et la position, de prévisualiser le résultat et d’exporter un MP4.

Le MVP ne doit pas promettre de modifier parfaitement le visage, les vêtements, la géométrie ou les interactions physiques. Un remplacement de décor par compositing conserve le sujet original ; il ne reconstitue pas la scène comme un modèle de génération vidéo.

---

## 2. Vision produit et utilisateurs

### Problème à résoudre

Les outils génératifs en ligne peuvent coûter cher quand on produit plusieurs minutes, et offrent parfois peu de contrôle sur la reproductibilité. Sur un Mac Apple Silicon doté de 64 Go, beaucoup d’étapes de préparation et de compositing peuvent être exécutées localement. Le défi est d’orchestrer ces étapes proprement, sans confondre un pipeline de postproduction avec une génération vidéo complète.

### Cas d’usage de départ

1. Remplacer le fond d’une vidéo avec un décor fixe.
2. Générer ou importer un décor, puis le recadrer et le placer derrière le sujet.
3. Stabiliser le masque du sujet et réduire le scintillement des contours.
4. Ajuster couleur, luminosité, netteté et ombre de contact pour mieux intégrer le sujet.
5. Sauvegarder un projet rééditable et exporter une vidéo.

### Expérience utilisateur souhaitée

L’utilisateur importe une vidéo, voit une analyse rapide, sélectionne le sujet à conserver, décrit le résultat voulu en langage naturel, puis reçoit un plan lisible et modifiable. Il peut choisir un mode **Rapide**, **Équilibré** ou **Qualité**, vérifier les étapes avant de lancer le calcul, comparer avant/après et revenir à une version précédente.

Le langage naturel facilite la création du plan ; il ne doit pas déclencher directement des commandes shell ni contrôler les fichiers arbitrairement. L’interface doit montrer ce qui sera fait, quels modèles seront chargés, l’espace disque estimé et si une étape risque de prendre beaucoup de temps.

---

## 3. Périmètre et limites

### Inclus au MVP

- Application macOS locale avec interface de bureau.
- Import MP4/MOV, lecture des métadonnées, création d’une copie de travail.
- Découpage d’un extrait et sélection d’une image de référence.
- Sélection du sujet par clic ou rectangle ; masque vidéo propagé dans le temps.
- Correction manuelle simple du masque (ajouter/retirer une zone, correction ponctuelle).
- Import d’un décor fixe, placement, échelle, recadrage et aperçu.
- Compositing, réglages colorimétriques simples et export MP4.
- Sauvegarde de projet, cache d’étapes réutilisables, annulation et reprise après erreur.
- Mesure du temps, mémoire approximative, espace disque et étapes terminées.

### Hors périmètre initial

- Génération photoréaliste de la totalité de la vidéo.
- Remplacement fidèle du visage ou du corps en mouvement.
- Reconstruction 3D exacte à partir d’une seule caméra.
- Résolution de toutes les occlusions (mains devant le visage, objets traversant le sujet, cheveux fins, transparences).
- Montage multipiste professionnel, étalonnage avancé ou mixage audio complet.
- Envoi automatique de médias vers un service cloud.

### Principes de vérité produit

- Une image de décor placée derrière le sujet n’est pas une scène 3D.
- La profondeur monoculaire est une estimation relative, pas une mesure fiable de distances.
- La segmentation vidéo peut dériver ; l’utilisateur doit pouvoir corriger le masque.
- Un rendu de démonstration sur quelques images ne prouve pas qu’un modèle est stable sur plusieurs minutes.
- Une compatibilité théorique Apple Silicon ne garantit ni vitesse, ni faible consommation mémoire, ni fonctionnement sans erreurs sur le modèle de Mac exact.

---

## 4. Architecture cible

### Principe

Garder une application centrale en Rust et isoler les moteurs ML dans des workers interchangeables. Rust gère les projets, le graphe d’opérations, les files de tâches, le cache, les ressources et les erreurs. Les workers traitent un type de tâche à la fois et exposent des entrées/sorties explicites.

```text
Interface Tauri + Vue/TypeScript
              │ commandes typées / événements de progression
              ▼
Noyau Rust : projets, plan, cache, tâches, ressources, annulation
       ┌──────┼────────────┬────────────┐
       ▼      ▼            ▼            ▼
Media worker  Vision worker Image worker  LLM planner (optionnel)
FFmpeg        SAM/video     ComfyUI ou     modèle local via runtime
VideoToolbox  segmentation  autre backend  compatible Apple Silicon
       └──────┴────────────┴────────────┘
              ▼
Rendu/compositing + export vidéo
```

### Choix par couche

| Couche | Choix de départ | Rôle | Règle d’architecture |
|---|---|---|---|
| UI | Tauri 2 + Vue 3 + TypeScript | Fenêtre, timeline simplifiée, contrôles, aperçu | Pas de calcul ML dans le frontend |
| Noyau | Rust | Projet, plan, tâches, cache, annulation, orchestration | API interne typée et versionnée |
| Média | FFmpeg CLI au départ ; VideoToolbox/AVFoundation si nécessaire | Probe, extraction, conversion, export | Garder les formats et paramètres dans le manifeste |
| Vision | Worker Python isolé | Segmentation, suivi, éventuellement profondeur | Environnement dédié ; versions et modèles verrouillés |
| Génération d’image | ComfyUI local ou backend d’image interchangeable | Décor ou éléments fixes | Démarrer derrière une interface d’adaptateur |
| LLM | Optionnel, local si utile | Transformer une demande en plan structuré | Le LLM propose ; Rust valide et exécute |
| Rendu | FFmpeg pour MVP ; Metal/Core Image à évaluer après profilage | Compositing, filtres, export | Ne pas écrire de kernels Metal avant d’avoir mesuré un goulot |

### Pourquoi ne pas tout écrire en Rust ?

Rust est adapté au contrôle d’état, aux tâches concurrentes, au cache et à la robustesse de l’application. Les modèles de vision et de génération sont d’abord disponibles dans des écosystèmes Python/PyTorch, MLX ou ComfyUI. Réécrire prématurément ces modèles en Rust serait coûteux et ne garantit pas un gain sur le Mac.

Le compromis recommandé est donc **Rust pour le produit et l’orchestration ; workers spécialisés pour l’inférence ; formats de données stables entre les deux**. Un worker peut ensuite être remplacé par Core ML, MLX, un backend Metal ou un service distant sans réécrire l’interface ni le gestionnaire de projets.

### Communication entre processus

Commencer simplement : Rust lance un worker local persistant avec un protocole JSON Lines sur stdin/stdout ou une API HTTP locale liée à `127.0.0.1`. Chaque requête porte un identifiant de tâche, un type d’opération, les chemins de fichiers, les options et une version de schéma. Les progrès sont émis comme événements structurés. Ne pas ajouter gRPC, message broker ou Kubernetes à un prototype mono-machine.

Les chemins doivent être validés et limités au répertoire de projet/cache. Le worker ne reçoit jamais de commande shell arbitraire. Les logs structurés excluent les contenus vidéo et les prompts privés sauf option explicite de diagnostic.

---

## 5. Pipeline de traitement

### A. Ingestion et analyse média

1. Lire les métadonnées : durée, dimensions, cadence, codec, orientation, audio.
2. Vérifier les pistes et les timestamps irréguliers ; normaliser seulement si nécessaire.
3. Créer une copie de travail ou référencer le média original sans le modifier.
4. Extraire une vignette et quelques images clés pour la prévisualisation et l’analyse.
5. Proposer un extrait court pour les premiers tests.

Privilégier une stratégie **proxy + original** : un proxy léger sert à l’aperçu et aux interactions ; le rendu final repart de la source et applique les mêmes réglages à la résolution de sortie. Toute normalisation doit être inscrite dans le plan.

### B. Plan de transformation

L’utilisateur peut choisir manuellement une opération ou demander un plan en langage naturel. Le plan interne est un graphe de nœuds déterministe, par exemple :

```json
{
  "schema_version": 1,
  "source": "media/source.mov",
  "operations": [
    {"id":"mask-1","type":"segment_video","input":"source","model":"sam2_video","options":{"prompt_frame":0}},
    {"id":"comp-1","type":"composite_background","input":"source","mask":"mask-1","background":"assets/background.png"},
    {"id":"export-1","type":"export_video","input":"comp-1","options":{"container":"mp4","video_codec":"h264"}}
  ]
}
```

Cet exemple décrit un format indicatif, pas une API à figer avant le prototype. Chaque opération doit avoir un schéma, des préconditions, des sorties, des ressources requises, une stratégie d’annulation et une version. Refuser les nœuds inconnus et les paramètres hors limites.

### C. Segmentation et suivi

Le modèle doit pouvoir recevoir un point, une boîte ou un masque initial, puis propager le sujet à travers la vidéo. SAM 2 est une piste sérieuse pour la segmentation vidéo guidée, mais ses performances et sa compatibilité d’exécution exacte doivent être testées sur le Mac cible. Ne pas promettre un détourage de qualité cinéma.

Le système doit afficher le masque sur l’image, autoriser des corrections à des images clés et propager ces corrections. Il faut enregistrer le masque en séquence réutilisable, idéalement avec précision suffisante et compression adaptée. Pour le MVP, mesurer la qualité avant d’investir dans un deuxième modèle de matting.

### D. Décor et génération d’image

Le décor peut être importé ou généré comme image fixe. Un LLM vision peut produire une description ou proposer des réglages de placement, mais l’image de fond doit rester un artefact indépendant du masque et du rendu afin d’éviter de recalculer inutilement.

L’intégration ComfyUI est utile comme backend expérimental : il fournit des graphes de génération et une API locale. L’application doit appeler un workflow versionné par adaptateur, et non dépendre de nœuds communautaires non vérifiés comme base critique du MVP.

### E. Compositing et intégration

Ordre de base :

1. Adapter le décor à la résolution, au ratio et au cadrage.
2. Déplacer/redimensionner le sujet si l’utilisateur le demande.
3. Appliquer le masque avec adoucissement contrôlé et décontamination de couleur de bord si disponible.
4. Composer avant/arrière avec ordre d’occlusion défini.
5. Appliquer les corrections de couleur, contraste, grain et netteté.
6. Ajouter une ombre de contact simple seulement si le plan permet de déterminer une zone de sol plausible.
7. Prévisualiser et exporter.

La cohérence d’éclairage du sujet est limitée si la lumière de prise de vue ne correspond pas au décor. Les corrections classiques doivent être contrôlables et réversibles ; le relighting génératif sera une étape ultérieure.

### F. Cohérence temporelle

Comparer les masques consécutifs, détecter les ruptures, changements de taille, trous et scintillements. Propager les corrections depuis les images clés. Utiliser le flux optique ou le lissage uniquement après tests : le lissage naïf peut brouiller les contours ou faire disparaître des mouvements légitimes.

Prévoir une vue de diagnostic avec source, masque, sujet isolé et composite. Le meilleur outil de dépannage est une comparaison par image avec les étapes intermédiaires conservées.

### G. Export

Pour le MVP, exporter en MP4 H.264 avec audio préservé si possible, résolution et cadence de la source par défaut. Faire un contrôle des pistes, durée et lecture après export. Proposer une option haute qualité et une option rapide seulement après mesure sur des clips de référence.

VideoToolbox est une piste d’encodage/décodage matériel sous macOS. Vérifier les capacités et résultats réels du FFmpeg installé ; garder un encodeur logiciel de repli. Ne pas confondre accélération d’encodage avec accélération de l’inférence IA.

---

## 6. Modèles et moteurs à évaluer

Les noms ci-dessous sont des candidats, pas un engagement à tous les intégrer. La priorité est l’essai reproductible sur trois clips représentatifs, le relevé des contraintes de licence, puis la décision.

| Tâche | Candidat / famille | Utilité | Risques et validation requise |
|---|---|---|---|
| Planification texte | Qwen instruct local ou modèle équivalent via MLX/llama.cpp | Traduire une intention en plan JSON et expliquer les étapes | Ne pas le laisser exécuter du code ; vérifier qualité JSON, temps et mémoire |
| Compréhension d’image/vidéo | Qwen-VL local ou analyse d’images clés | Décrire la scène, repérer sujet, décor et intention | Pas un moteur de segmentation ni un tracker précis |
| Segmentation vidéo | SAM 2 / version maintenue compatible | Sélectionner et suivre un sujet après prompt | Installer et tester MPS/Core ML/chemin disponible ; mesurer dérive, vitesse et mémoire |
| Matting | Modèle spécialisé à choisir après MVP | Raffiner cheveux, flou et transparences | Peut ralentir fortement ; comparer réellement aux masques SAM et corrections manuelles |
| Profondeur | Depth Anything V2 ou modèle maintenu équivalent | Estimation relative pour aide à l’intégration | N’est pas une géométrie métrique ni un suivi fiable des pieds |
| Décor image | ComfyUI avec modèle local approprié (par ex. FLUX ou modèle plus léger compatible) | Créer un fond fixe contrôlable | Tester mémoire/latence et vérifier licence des poids ; pas requis pour premier MVP |
| Vidéo générative | Wan et autres familles open source maintenues | Essais de transformation de clips ou génération de plans | Très coûteux en mémoire/temps ; la compatibilité Apple Silicon dépend du modèle et du runtime ; mode expérimental |
| Rendu vidéo | FFmpeg, filtres et accélération VideoToolbox | Conversion, compositing de base, export | Certains filtres peuvent être CPU ; profiler avant de promettre du temps réel |

### Répartition d’exécution recommandée

- **MLX** : candidat naturel pour LLM locaux sur Apple Silicon lorsque le modèle est disponible ou converti proprement.
- **PyTorch/MPS** : utile pour des modèles de vision dont les implémentations sont disponibles en PyTorch, avec tests de compatibilité nécessaires.
- **Core ML** : envisager quand un modèle converti et validé existe et que les bénéfices justifient le coût d’intégration.
- **ComfyUI** : backend de génération d’images/vidéos facultatif, piloté par workflows enregistrés.
- **FFmpeg/VideoToolbox** : média, conversion, compositing initial et export ; ce n’est pas un remplacement du runtime IA.

Éviter de maintenir simultanément trois installations lourdes de chaque moteur. Épingler versions, checkpoints, licences et empreintes des modèles dans un inventaire local. Charger/décharger les modèles de manière explicite pour éviter de faire cohabiter plusieurs poids lourds en mémoire.

---

## 7. Optimisation pour le MacBook Pro M5 Pro 64 Go

### Règles de mémoire

- La mémoire est partagée par macOS, GPU, modèles, buffers vidéo et autres applications ; 64 Go ne sont pas 64 Go disponibles pour les poids.
- Mesurer mémoire résidente et pression mémoire à chaque scénario.
- Traiter en lots courts et en tuiles lorsque possible ; éviter de charger toute la vidéo décodée en RAM.
- Garder une file de tâches bornée et une seule tâche générative lourde par défaut.
- Décharger les modèles inactifs et afficher l’occupation approximative.
- Utiliser proxy basse résolution pour l’aperçu et réserver la pleine résolution au rendu final.
- Mettre en cache les sorties coûteuses (masque, profondeur, décor, rendu proxy) avec invalidation par paramètres et versions.

### Traitement vidéo

Ne pas matérialiser toutes les images en tableaux RGB pleine résolution sans raison. Utiliser des lecteurs/encodeurs en flux, décoder uniquement les images nécessaires et respecter le timestamp/cadence d’origine. Si un modèle travaille à une cadence réduite, réinjecter les résultats selon une stratégie déclarée et vérifier la stabilité.

### Cache déterministe

La clé de cache d’une étape devrait intégrer : hash/empreinte des entrées, type et version d’opération, modèle et poids, paramètres normalisés, version du worker et options de précision. Un changement de décor ne doit pas invalider le masque. Un changement de masque invalide le composite et l’export, mais pas l’analyse média.

### Profilage avant optimisation manuelle

Mesurer chaque étape : temps, mémoire, taille des intermédiaires, utilisation CPU/GPU si disponible. N’écrire un kernel Metal personnalisé que si un profil montre qu’une étape de pixel/compositing est dominante et que les API existantes ne suffisent pas. Ne pas optimiser à l’aveugle en Rust un traitement qui passe la majorité de son temps dans un modèle Python.

### Thermique et calcul long

Les générations peuvent être longues et faire chauffer le Mac. L’interface doit offrir pause/annulation propre, sauvegarde du dernier état cohérent, et éviter de lancer des jobs lourds en concurrence par défaut. Faire des tests soutenus, pas seulement un rendu isolé.

---

## 8. Sécurité, confidentialité et fiabilité

- Local-first : aucun média ne quitte le Mac sans action explicite et visible de l’utilisateur.
- Réseau désactivé par défaut pour les traitements média ; une éventuelle API distante doit être opt-in, présenter les fichiers transmis et annoncer le coût.
- Pas de shell arbitraire fourni par le LLM. Rust choisit une opération dans une liste blanche.
- Valider les chemins, extensions, tailles, durées, options et schémas de messages.
- Conserver l’original en lecture seule ; écrire les dérivés dans le dossier du projet/cache.
- Stocker séparément les médias originaux référencés et les artefacts calculés ; laisser l’utilisateur nettoyer le cache.
- Signaler les dépendances et licences des modèles avant distribution commerciale.
- Assurer des erreurs compréhensibles, logs structurés, annulation, reprise et export atomique.

---

## 9. Format de projet et modèle de données

Un projet devrait contenir un manifeste JSON versionné, des références aux médias, les opérations, les réglages, les sorties et l’état d’exécution. Les gros médias ne doivent pas être copiés dans le manifeste.

```text
MonProjet/
  project.json
  media/                 # optionnel : copies locales ou proxies
  assets/                # décors, masques importés, références
  cache/                 # régénérable et nettoyable
  exports/               # rendus finaux
  logs/                  # diagnostics sans contenu sensible par défaut
```

Le projet reste ouvrable si un modèle n’est plus installé : afficher l’opération manquante et permettre de remplacer le moteur ou d’exporter les éléments déjà calculés. Toute évolution du schéma passe par migration testée.

---

## 10. Plan de livraison par étapes

### Étape 0 — preuve technique sur le Mac

- Vérifier versions de macOS, Xcode/Command Line Tools, FFmpeg et disponibilité des frameworks.
- Tester un import/export vidéo avec préservation audio et orientation.
- Tester SAM 2 sur trois clips courts ; consigner installation, backend, temps, mémoire, qualité, limites.
- Tester un backend image local sans l’intégrer à l’application.
- Produire un rapport de résultats avant de choisir les dépendances définitives.

**Sortie attendue :** tableau de faisabilité et vidéos comparatives, sans UI complexe.

### Étape 1 — MVP manuel sans LLM

- Squelette Tauri/Vue et noyau Rust.
- Import, player, découpage simple, sélection de sujet, masque, correction minimale.
- Fond fixe et compositing.
- Export MP4 et projet réouvrable.

**Critère :** un utilisateur peut réaliser un remplacement de fond basique sans terminal.

### Étape 2 — cohérence et qualité

- Propagation vidéo, édition de points clés, détection de dérive.
- Proxy, aperçu fluide, cache d’étapes, reprise/annulation.
- Réglages couleur et contrôles avant/après.
- Tests sur cheveux, mouvements rapides, occultations, fond complexe.

### Étape 3 — création assistée

- Génération ou import de décors.
- Intégration ComfyUI via adaptateur versionné.
- Presets de cadrage et d’éclairage ; profondeur seulement si elle améliore les cas testés.

### Étape 4 — planification par LLM

- Le LLM transforme l’instruction en plan schématique.
- Afficher le plan et les coûts estimés avant exécution.
- Valider chaque nœud et chaque paramètre en Rust.
- Ajouter des explications et alternatives (rapide/équilibré/qualité).

### Étape 5 — moteurs génératifs vidéo

- Comparer des modèles sur Apple Silicon et, si utile, des GPU distants opt-in.
- Limiter la génération aux plans ou zones où le compositing ne suffit pas.
- Mesurer coût, temps, qualité, droits/licence et stabilité avant de l’exposer comme fonction standard.

---

## 11. Critères d’acceptation et benchmark

Créer un petit jeu de clips autorisés et reproductibles :

1. Caméra fixe, personne presque immobile, arrière-plan simple.
2. Personne marchant avec gestes et mains visibles.
3. Sujet avec cheveux/contours fins, mouvements ou occultation partielle.
4. Caméra légèrement mobile (cas de diagnostic, pas nécessairement accepté au MVP).

Pour chaque essai, enregistrer :

- MacOS, version de l’application, modèle/runtime/checkpoint/licence.
- Résolution, durée, cadence, codec, paramètres.
- Temps par étape, mémoire maximale observée, espace temporaire.
- Taux d’images réussies, erreurs, qualité du masque sur images choisies.
- Scintillement visible, pertes du sujet, bords halo, désynchronisation audio.
- Temps d’aperçu et d’export.

Les seuils chiffrés de vitesse et de qualité doivent être définis après le benchmark de l’étape 0, pas inventés à l’avance. Pour le MVP, l’acceptation est fonctionnelle : résultat exploitable sur un clip court, projet réouvrable, corrections possibles et absence de corruption de la source.

---

## 12. Instructions de travail pour l’agent de développement

1. Commencer par inspecter le Mac et le dépôt existant. Ne pas créer une nouvelle architecture si une base de projet est déjà présente.
2. Lire les consignes du dépôt (`AGENTS.md`, README, manifests) et respecter l’environnement existant.
3. Avant d’installer un gros modèle ou une dépendance, proposer/consigner la taille, licence, runtime et effet mémoire ; commencer par un test minimal et réversible.
4. Livrer d’abord l’étape 0 et ses preuves, puis construire le MVP manuel.
5. Ne pas ajouter le LLM au chemin critique tant que le pipeline sans LLM ne fonctionne pas.
6. Ne pas prétendre qu’un modèle est compatible ou rapide sans test local vérifiable.
7. Garder les opérations et workers remplaçables ; ne pas lier le format du projet à un modèle unique.
8. Ne pas exécuter de commandes destructives, déplacer ou supprimer les médias sources, ni envoyer de fichiers vers un service externe sans demande explicite.
9. À chaque jalon, fournir : fichiers créés/modifiés, commandes de lancement, résultats du benchmark, limites connues et prochaine étape concrète.
10. Faire des commits ou sauvegardes cohérents si le projet est sous Git ; éviter les réécritures massives non vérifiées.

### Première tâche concrète à donner à l’agent

> Inspecte l’environnement du Mac et propose un rapport de faisabilité pour le projet décrit dans ce document. Ne code pas encore l’application complète et ne télécharge pas de gros modèles sans vérifier d’abord leur taille et licence. Teste uniquement les éléments nécessaires à un prototype : import/probe/export média, un essai de segmentation vidéo sur un clip court fourni, et un test de génération d’image si un modèle déjà installé le permet. Mesure temps, mémoire, erreurs et qualité observable. Rends un rapport avec les choix recommandés et les limites avant de commencer l’UI.

---

## 13. Références techniques à vérifier au moment de l’implémentation

Les dépôts et frameworks évoluent ; épingler les versions et relire leurs licences, instructions Apple Silicon et prérequis avant intégration.

- [MLX — framework Apple Silicon](https://github.com/ml-explore/mlx)
- [MLX-LM — inférence et fine-tuning de LLM avec MLX](https://github.com/ml-explore/mlx-lm)
- [SAM 2 — segmentation d’images et de vidéos](https://github.com/facebookresearch/sam2)
- [ComfyUI — moteur de workflows locaux et API](https://github.com/Comfy-Org/ComfyUI)
- [Wan2.2 — modèles de génération vidéo open source](https://github.com/Wan-Video/Wan2.2)
- [Tauri 2 — framework d’applications desktop](https://tauri.app/)
- [Apple VideoToolbox — encodage et décodage vidéo](https://developer.apple.com/documentation/videotoolbox)
- [FFmpeg — documentation](https://ffmpeg.org/documentation.html)

---

## Décision de conception à retenir

Construire d’abord un **éditeur local intelligent de compositing vidéo**, avec un pipeline robuste et mesurable. Ajouter la génération d’images comme moteur interchangeable. Introduire le LLM uniquement pour rendre les opérations plus accessibles et planifier des traitements validés. Garder la génération vidéo complète comme capacité avancée, expérimentale et facultative.
