# Workflows ComfyUI versionnés

Chaque fichier `<id>.v<version>.json` décrit un workflow au format API de ComfyUI et la manière dont videeo le paramètre :

- `graph` : le graphe au format API (`File > Export (API)` dans ComfyUI) ;
- `params` : pour chaque paramètre exposé (`prompt`, `width`, `height`, `steps`, `seed`), la liste des `[nœud, entrée]` à remplacer ;
- `output_node` : le nœud `SaveImage` dont on récupère l'image ;
- `requires.models` : pour chaque `Classe.entrée` de chargement, les fichiers attendus ; l'adaptateur vérifie leur présence via `/object_info` avant de soumettre ;
- `tested` : `true` seulement si le workflow a été exécuté de bout en bout sur ce Mac.

Règles appliquées par l'adaptateur (`app/src-tauri/src/comfy.rs`) :

- serveur **local uniquement** (`127.0.0.1`, `localhost` ou `[::1]`) ;
- refus de tout workflow contenant un **nœud d'API distant** (`api_node: true` dans `/object_info`, modules `comfy_api_nodes`) : aucun média ni prompt ne part vers un service en ligne ;
- refus si un nœud est inconnu du serveur ou si un fichier de modèle requis est absent ;
- une modification du graphe impose une nouvelle version du fichier : un décor généré garde dans ses métadonnées l'`id` et la `version` du workflow.
