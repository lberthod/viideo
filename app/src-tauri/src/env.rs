//! Localisation des outils externes (FFmpeg, Python du worker vision, poids SAM 2).
//!
//! En développement, tout est résolu depuis la racine du dépôt ; chaque chemin peut être
//! remplacé par une variable d'environnement (VIDEEO_FFMPEG, VIDEEO_FFPROBE, VIDEEO_PYTHON,
//! VIDEEO_WORKER, VIDEEO_MODELS).

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;

/// Racine du dépôt au moment de la compilation (app/src-tauri/../..).
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap_or_else(|_| PathBuf::from("."))
}

fn from_env_or(var: &str, default: PathBuf) -> PathBuf {
    std::env::var_os(var).map(PathBuf::from).unwrap_or(default)
}

/// Une app macOS lancée hors terminal n'hérite pas du PATH Homebrew : on cherche aux emplacements usuels.
fn find_binary(name: &str, var: &str) -> PathBuf {
    if let Some(p) = std::env::var_os(var) {
        return PathBuf::from(p);
    }
    for dir in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"] {
        let p = Path::new(dir).join(name);
        if p.exists() {
            return p;
        }
    }
    PathBuf::from(name)
}

#[derive(Debug, Clone)]
pub struct Tools {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
    pub python: PathBuf,
    pub worker: PathBuf,
    pub models: PathBuf,
    /// Worker de génération d'image et son Python (environnement séparé : diffusers).
    pub image_python: PathBuf,
    pub image_worker: PathBuf,
    /// Workflows ComfyUI versionnés.
    pub workflows: PathBuf,
    /// Encodeur matériel H.264 disponible dans ce FFmpeg.
    pub videotoolbox: bool,
}

impl Tools {
    pub fn detect() -> Self {
        let root = repo_root();
        let ffmpeg = find_binary("ffmpeg", "VIDEEO_FFMPEG");
        let videotoolbox = Command::new(&ffmpeg).args(["-hide_banner", "-encoders"]).output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains("h264_videotoolbox"))
            .unwrap_or(false);
        Tools {
            videotoolbox,
            ffmpeg,
            ffprobe: find_binary("ffprobe", "VIDEEO_FFPROBE"),
            python: from_env_or("VIDEEO_PYTHON", root.join(".venvs/vision/bin/python")),
            worker: from_env_or("VIDEEO_WORKER", root.join("workers/vision/worker.py")),
            models: from_env_or("VIDEEO_MODELS", root.join("models/sam2")),
            image_python: from_env_or("VIDEEO_IMAGE_PYTHON", root.join(".venvs/image/bin/python")),
            image_worker: from_env_or("VIDEEO_IMAGE_WORKER", root.join("workers/image/worker.py")),
            workflows: from_env_or("VIDEEO_WORKFLOWS", root.join("workflows/comfyui")),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ToolStatus {
    pub name: String,
    pub path: String,
    pub ok: bool,
    pub detail: String,
}

fn first_line(cmd: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new(cmd).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    text.lines().next().map(|l| l.trim().to_string())
}

/// Vérifie la présence de chaque dépendance, pour l'écran de diagnostic au démarrage.
pub fn check(tools: &Tools) -> Vec<ToolStatus> {
    let mut v = Vec::new();
    for (name, path) in [("FFmpeg", &tools.ffmpeg), ("FFprobe", &tools.ffprobe)] {
        let line = first_line(path, &["-version"]);
        v.push(ToolStatus { name: name.into(), path: path.display().to_string(), ok: line.is_some(),
                            detail: line.unwrap_or_else(|| "introuvable".into()) });
    }
    let py = first_line(&tools.python, &["-c", "import torch, sam2; print('torch', torch.__version__, 'mps', torch.backends.mps.is_available())"]);
    v.push(ToolStatus { name: "Python vision".into(), path: tools.python.display().to_string(), ok: py.is_some(),
                        detail: py.unwrap_or_else(|| "environnement absent ou SAM 2 non installé".into()) });
    v.push(ToolStatus { name: "Worker vision".into(), path: tools.worker.display().to_string(), ok: tools.worker.exists(),
                        detail: if tools.worker.exists() { "présent".into() } else { "introuvable".into() } });
    let img = first_line(&tools.image_python, &["-c", "import diffusers; from diffusers import Flux2KleinPipeline; print('diffusers', diffusers.__version__)"]);
    v.push(ToolStatus { name: "Python image (décors)".into(), path: tools.image_python.display().to_string(), ok: img.is_some(),
                        detail: img.unwrap_or_else(|| "optionnel : génération de décors indisponible".into()) });
    let tiny = tools.models.join("sam2.1_hiera_tiny.pt");
    v.push(ToolStatus { name: "Poids SAM 2.1 tiny".into(), path: tiny.display().to_string(), ok: tiny.exists(),
                        detail: if tiny.exists() { "présent".into() } else { "manquant".into() } });
    v
}
