# Intégration du sujet : analyse et corrections

À partir des premiers rendus (vidéo `showcase/videeo_avant_apres.mp4`), tu as relevé trois défauts :
- les pieds du pupitre qui flottent derrière la présentatrice ;
- un corps qui s'arrête net ;
- des chevilles qui se fondent dans le sol.

Ce document explique **pourquoi**, puis ce qui a été corrigé.

![Source, premier rendu, rendu corrigé](comparison.jpg)

## Pourquoi ce n'était pas bon

| Défaut visible | Cause réelle |
|---|---|
| Deux « poteaux » sombres sous les mains de la présentatrice | Le rectangle de sélection englobait le pupitre. Sous le plateau, le masque contient un **morceau du pupitre** (x 660–700) et le **vrai bas du corps** (x 729–870), en partie caché par le pupitre dans la vidéo d'origine. Sur un autre décor, ces deux fragments ressemblent à des poteaux, et les mains s'appuient sur un plateau qui n'existe plus. |
| Corps coupé net | Plan à mi-corps : la vidéo d'origine coupe le sujet en bas. Dans son studio, rien ne justifie cette coupure ; dans le nouveau décor, elle saute aux yeux. |
| Chevilles qui se fondent dans le sol | Les herbes passent devant les jambes dans la vidéo source. Le masque s'arrête aux tibias à certaines images, et aucune ombre ne relie le sujet au sol. |
| Aspect « autocollant » | Aucune lumière du décor ne déborde sur les bords du sujet : le contour reste celui de la prise de vue d'origine. |
| Bande sombre en bas | **Mon erreur dans la vidéo de démonstration** : la légende était posée par-dessus l'image. Elle est maintenant dans une bande au-dessus. |

Point commun : le détourage était bon. Ce qui manquait, ce sont les **outils d'intégration** que les compositeurs utilisent pour qu'un sujet appartienne au décor.

## Ce qui a été ajouté

### 1. Élément de premier plan, généré et détouré automatiquement

On décrit un objet (« comptoir en bois massif à façade pleine », « touffe d'herbes »…) :
1. FLUX.2 le génère isolé sur un fond uni ;
2. SAM 2 le détoure ;
3. l'app le recadre en PNG transparent et le range dans `assets/elements/`.

On peut aussi importer une image : un PNG déjà transparent est gardé tel quel, une photo est détourée.

Le bouton **« Placer devant le bas du sujet »** calcule la position. Le bord haut de l'élément arrive à la hauteur choisie, par exemple juste sous les mains (≈ 36 % du sujet). L'élément est au moins 10 % plus large que le sujet, et il peut déborder sous le cadre : un objet au premier plan coupé par le bord de l'image est naturel. Il ne remonte jamais au-dessus de la ligne demandée.

Deux échecs pendant les essais ont dicté ces règles :
- **Table à pieds** : on voyait les fragments du corps entre ses pieds. D'où la consigne « façade pleine fermée jusqu'au sol ».
- **Panneau vertical** : FLUX a produit un panneau plus haut que large, que le placement a agrandi au point de **cacher entièrement** la présentatrice. D'où la règle « jamais au-dessus de la ligne », couverte par 9 vérifications (`npm test`).

### 2. Points « Exclure »

Deux points d'exclusion, posés sur le pupitre, nettoient le masque. Comme le bas du corps est réellement incomplet dans la vidéo d'origine, c'est l'élément de premier plan qui règle vraiment le problème.

### 3. Ombre de contact

Elle est calculée **image par image** à partir du masque : une ellipse adoucie sous l'empreinte des pieds, qui suit la marche. Elle n'est produite **que si les pieds sont visibles**, c'est-à-dire si le masque ne touche pas le bas de l'image ; sinon il n'y a pas de sol déterminable. C'est la règle du brief. Résultat : 88 images sur 120 pour le randonneur, 0 pour la présentatrice. L'ombre suit exactement la transformation du sujet (échelle, position).

### 4. Light wrap

La lumière floutée du décor déborde légèrement sur une bande de bord du sujet. La bande vaut masque × (1 − flou du masque) ; le mélange se fait vers le décor flouté. L'aperçu et l'export utilisent les mêmes constantes (`WRAP_*_720` dans `media.rs` et `compositor.ts`).

### 5. Relighting directionnel

Le sujet garde la lumière de son tournage : souvent une face uniformément éclairée de studio, alors que le décor a une lumière qui vient d'un côté. Le relighting éclaire le sujet **du côté d'où vient la lumière du décor, avec sa couleur**.

- **Lumière du décor** (« Lire la lumière du décor ») : direction = barycentre des 3 % de pixels les plus lumineux par rapport au centre ; couleur = moyenne de ces pixels. Une lumière n'arrive jamais franchement par en dessous, pour qu'un sol clair (neige, eau) n'éclaire pas le visage par le menton. Direction, hauteur et couleur restent réglables.
- **Forme du sujet** : des normales approchées, calculées **à partir du seul masque** (`workers/vision/relight.py`). Chaque segment horizontal (tronc, bras, jambe) est vu comme un cylindre vertical ; le haut de la tête et des épaules regarde vers le haut. Un sujet coupé par le cadre ne « tourne » pas au bord de l'image. Calcul : ~0,07 s par image, mis en cache avec la clé du masque.
- **Éclairage** : gain = 1 − 0,8·s + 1,6·s·couleur·max(0, N·L). Un ombrage moyen laisse l'exposition inchangée : le côté éclairé monte jusqu'à ×(1 + 0,8·s), le côté opposé descend jusqu'à ×(1 − 0,8·s).
- **Export** : deux `colorchannelmixer` (N·L puis gain, la composante alpha = 255 servant de constante) et un `blend` multiplicatif ; l'aperçu applique les mêmes formules, vérifiées des deux côtés par des tests (`cargo test`, `npm test`).

![Sans / avec relighting : la lumière du couchant vient de la droite](relighting-closeup.jpg)

*Export de l'app, même image. À droite, le côté droit du visage et de la veste prend la lumière chaude du couchant ; le côté gauche passe dans l'ombre.*

Dans l'app (scénarios `app/dev/demo_v3_*.json`), la lumière lue sur le décor alpin vaut x = 0,71, y = −0,43, couleur `#fbebcd` ; pour le lac, x = 0,32, y = −0,83 (lumière du ciel). Normales : 125 images en quelques secondes, 19 Mo de cache. Comparaison vidéo : [`showcase/videeo_relighting_v3.mp4`](../../showcase/videeo_relighting_v3.mp4) (`make_showcase.py v3`).

Pourquoi pas un relighting génératif (IC-Light, etc.) : aucun modèle de ce type n'est installé sur la machine, et un modèle image appliqué image par image scintille sur une vidéo. Ici, l'ombrage ne dépend que du masque, déjà stabilisé et contrôlé : il ne scintille pas.

### Ordre de composition (identique dans l'aperçu et l'export)

```text
décor (flou éventuel) → ombre de contact → sujet (couleur, puis relighting) → light wrap → élément de premier plan
```

Le graphe FFmpeg n'ajoute chaque étape que si elle est activée. Il a été vérifié par un test qui contrôle les indices d'entrée et les filtres, puis par un export réel avec les trois effets (3,5 s pour 5 s en 720p).

## Résultat

Vidéo de démonstration : [`showcase/videeo_avant_apres_v2.mp4`](../../showcase/videeo_avant_apres_v2.mp4), en trois colonnes (source, premier rendu, rendu corrigé). La construire : `.venvs/vision/bin/python showcase/make_showcase.py`.

| Exemple | Ce qui change, visiblement |
|---|---|
| Présentatrice, terrasse alpine | Comptoir en bois de chalet devant elle : appui des mains, plus de pieds de pupitre flottants, coupure du corps cachée. |
| Présentatrice, studio TV | Comptoir de plateau TV en verre bleu : même correction, cohérente avec le décor généré. |
| Randonneur, lac animé | Ombre de contact sous les pieds et bords adoucis par le light wrap. L'amélioration est **subtile** à cette taille ; le premier rendu était déjà le plus réussi. |

Coût : la génération et le détourage d'un élément prennent ~15–20 s (FLUX.2 puis SAM 2) ; l'ombre de contact ~2 s pour 120 images.

## Bug corrigé : poche de paysage entre le bras et le sac

![Avant / après : la poche entre la bretelle, le bras et la jambe montrait le feuillage de la vidéo source](pocket-fix.jpg)

Quand le randonneur tient son sac à la main, la bretelle, le bras et la jambe referment une poche de décor. SAM la détourait correctement comme un vide, mais le nettoyage du masque bouchait tous les trous fermés jusqu'à 0,2 % de l'image (~1 840 px en 720p) : la poche était remplie avec l'image source (feuillage vert). Mesuré sur les masques bruts : poches de 800 à 2 700 px, vrais défauts de SAM de quelques dizaines de px (médiane 71). Le seuil passe à 0,02 % (~180 px) ; les masques nettoyés avec l'ancienne règle sont marqués à recalculer (clé `cleanup-v2`). Test : `test_clean_keeps_scenery_pocket_closed_by_subject`.

## Ce qui reste perfectible

- **Lumière du sujet** : le relighting directionnel donne le bon côté et la bonne couleur, mais sa forme vient du masque : il ne connaît ni le relief du visage (nez, orbites) ni les plis des vêtements, et il n'efface pas les ombres de la prise de vue d'origine. Un contre-jour marqué (soleil derrière le sujet) n'est pas reproduit.
- **Perspective de l'élément généré** : FLUX produit une vue de face à hauteur d'yeux. Si la caméra du décor est plus haute ou plus basse, le comptoir peut paraître légèrement « collé ».
- **Qualité variable de l'élément** : FLUX ne suit pas toujours la description (le panneau vertical, par exemple). On régénère avec une autre graine, ou on importe une image.

## Projet

**Schéma 5**, avec des migrations testées : ombre, light wrap, élément de premier plan (schéma 4), relighting (`relight`, `light_x`, `light_y`, `light_color`, schéma 5).
