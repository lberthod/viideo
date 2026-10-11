//! Bibliothèque de décors du projet : `assets/backgrounds/<id>.<ext>` + `<id>.json` (métadonnées).
//!
//! Un décor généré garde tout ce qu'il faut pour être reproduit : moteur, modèle, licence, prompt,
//! graine, taille, étapes et, pour ComfyUI, l'id et la version du workflow.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{json, Value};

use crate::error::{err, Result};
use crate::project::{self, Project};

pub const DIR: &str = "assets/backgrounds";
pub const IMAGE_EXT: [&str; 4] = ["png", "jpg", "jpeg", "webp"];
/// Décors animés (étape 5) : boucles MP4 produites par `video::make_loop`.
pub const VIDEO_EXT: [&str; 1] = ["mp4"];

pub fn is_video(rel: &str) -> bool {
    rel.rsplit('.').next().is_some_and(|e| VIDEO_EXT.contains(&e.to_lowercase().as_str()))
}

#[derive(Debug, Clone, Serialize)]
pub struct BackgroundItem {
    pub id: String,
    /// Chemin relatif au projet.
    pub path: String,
    pub abs_path: String,
    pub source: String,
    /// "image" ou "video" (décor animé).
    pub kind: String,
    pub created_at: String,
    pub meta: Value,
}

fn meta_path(img: &Path) -> PathBuf {
    img.with_extension("json")
}

pub fn list(p: &Project) -> Vec<BackgroundItem> {
    let mut items = Vec::new();
    // Décors importés avant la bibliothèque (étapes 1–2) : assets/background_*.ext
    for (dir, rel_dir) in [(p.dir.join(DIR), DIR), (p.dir.join("assets"), "assets")] {
        for e in fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = e.path();
            let ext = path.extension().and_then(|x| x.to_str()).map(|x| x.to_lowercase()).unwrap_or_default();
            let in_library = rel_dir == DIR;
            let accepted = IMAGE_EXT.contains(&ext.as_str()) || (in_library && VIDEO_EXT.contains(&ext.as_str()));
            if !accepted || path.to_string_lossy().contains(".tmp.") || path.to_string_lossy().contains(".part.") || path.to_string_lossy().contains("_raw.") {
                continue;
            }
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            let meta: Value = fs::read_to_string(meta_path(&path)).ok().and_then(|t| serde_json::from_str(&t).ok())
                .unwrap_or_else(|| json!({"source": "imported"}));
            let created_at = meta["created_at"].as_str().map(String::from).unwrap_or_else(|| {
                e.metadata().and_then(|m| m.modified()).map(|t| chrono::DateTime::<chrono::Local>::from(t).to_rfc3339()).unwrap_or_default()
            });
            items.push(BackgroundItem {
                id: path.file_stem().unwrap().to_string_lossy().to_string(),
                path: format!("{rel_dir}/{name}"),
                abs_path: path.display().to_string(),
                source: meta["source"].as_str().unwrap_or("imported").to_string(),
                kind: if VIDEO_EXT.contains(&ext.as_str()) { "video".into() } else { "image".into() },
                created_at,
                meta,
            });
        }
    }
    items.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    items
}

/// Copie une image externe dans la bibliothèque (jamais de déplacement ni de modification de l'original).
pub fn import(p: &Project, src: &Path) -> Result<String> {
    let ext = src.extension().and_then(|e| e.to_str()).map(|e| e.to_lowercase()).unwrap_or_default();
    if !IMAGE_EXT.contains(&ext.as_str()) {
        return err("Format de décor non pris en charge (PNG, JPEG ou WebP).");
    }
    if fs::metadata(src)?.len() > 100 * 1024 * 1024 {
        return err("Image de décor trop lourde (100 Mo au maximum).");
    }
    fs::create_dir_all(p.dir.join(DIR))?;
    let id = format!("imp_{}", &project::fingerprint(src)?[..16]);
    let rel = format!("{DIR}/{id}.{ext}");
    let dest = p.resolve(&rel)?;
    if !dest.exists() {
        fs::copy(src, &dest)?;
        let meta = json!({"source": "imported", "original_name": src.file_name().map(|n| n.to_string_lossy().to_string()),
                          "created_at": project::now()});
        fs::write(meta_path(&dest), serde_json::to_vec_pretty(&meta)?)?;
    }
    Ok(rel)
}

/// Emplacement d'un nouveau décor généré.
pub fn new_generated(p: &Project, seed: u32) -> Result<(String, PathBuf)> {
    fs::create_dir_all(p.dir.join(DIR))?;
    let id = format!("gen_{}_{seed}", chrono::Local::now().format("%Y%m%d-%H%M%S%3f"));
    let rel = format!("{DIR}/{id}.png");
    let abs = p.resolve(&rel)?;
    Ok((rel, abs))
}

pub fn write_meta(img: &Path, meta: &Value) -> Result<()> {
    fs::write(meta_path(img), serde_json::to_vec_pretty(meta)?)?;
    Ok(())
}

pub fn delete(p: &Project, rel: &str) -> Result<()> {
    if !(rel.starts_with("assets/")) {
        return err("Seuls les décors du projet peuvent être supprimés.");
    }
    if p.manifest.composite.background.as_deref() == Some(rel) {
        return err("Ce décor est utilisé : choisissez-en un autre avant de le supprimer.");
    }
    let path = p.resolve(rel)?;
    fs::remove_file(&path)?;
    let _ = fs::remove_file(meta_path(&path));
    Ok(())
}

/// Emplacement d'un nouveau décor animé (boucle finale) et de la sortie brute du moteur.
pub fn new_animated(p: &Project, engine: &str, seed: u32) -> Result<(String, PathBuf, PathBuf)> {
    fs::create_dir_all(p.dir.join(DIR))?;
    let id = format!("anim_{engine}_{}_{seed}", chrono::Local::now().format("%Y%m%d-%H%M%S"));
    let rel = format!("{DIR}/{id}.mp4");
    let abs = p.resolve(&rel)?;
    let raw = p.resolve(&format!("{DIR}/{id}_raw.mp4"))?;
    Ok((rel, abs, raw))
}

/// Taille de génération : ~1 mégapixel au format de la sortie, multiples de 16.
pub fn generation_size(out_w: u32, out_h: u32) -> (u32, u32) {
    let ar = out_w as f64 / out_h.max(1) as f64;
    let w = (1.0e6 * ar).sqrt();
    let h = w / ar;
    let r16 = |v: f64| ((v / 16.0).round() as u32 * 16).clamp(256, 2048);
    (r16(w), r16(h))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_follow_output_aspect() {
        assert_eq!(generation_size(1920, 1080), (1328, 752));
        assert_eq!(generation_size(1080, 1920), (752, 1328));
        assert_eq!(generation_size(1000, 1000), (1008, 1008));
    }
}
