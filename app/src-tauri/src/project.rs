//! Projet videeo : manifeste `project.json` versionné, arborescence et clés de cache.
//!
//! ```text
//! MonProjet/
//!   project.json   manifeste (aucun média dedans)
//!   media/         proxies JPEG (régénérables)
//!   assets/        décors importés
//!   cache/         masques calculés (régénérables)
//!   exports/       rendus finaux
//!   logs/
//! ```
//! La source vidéo est référencée, jamais copiée ni modifiée.

use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{err, Result};
use crate::media::Probe;

pub const SCHEMA_VERSION: u32 = 2;
pub const MANIFEST: &str = "project.json";
const SUBDIRS: [&str; 5] = ["media", "assets", "cache", "exports", "logs"];

/// Bornes de validation des réglages : tout paramètre hors bornes est refusé.
pub const MAX_CLIP_SECONDS: f64 = 300.0;
pub const PROXY_HEIGHT: u32 = 720;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub schema_version: u32,
    pub app_version: String,
    pub created_at: String,
    pub updated_at: String,
    pub source: Source,
    pub clip: Clip,
    pub proxy: Option<Proxy>,
    pub segmentation: Segmentation,
    pub composite: Composite,
    #[serde(default)]
    pub exports: Vec<ExportRecord>,
    #[serde(default)]
    pub export_settings: ExportSettings,
}

/// Préréglage d'export : rapide (veryfast), équilibré (medium), qualité (slow).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ExportQuality {
    Fast,
    #[default]
    Balanced,
    High,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ExportSettings {
    pub quality: ExportQuality,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Source {
    /// Chemin absolu du média original (lecture seule).
    pub path: String,
    /// Empreinte : taille + date de modification + SHA-256 du premier Mo.
    pub fingerprint: String,
    pub probe: Probe,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Clip {
    pub start: f64,
    pub duration: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Proxy {
    pub key: String,
    /// Relatif au projet.
    pub dir: String,
    pub width: u32,
    pub height: u32,
    pub frames: u32,
    /// Cadence constante appliquée au proxy comme à l'export (fps=N/D).
    pub fps_num: u32,
    pub fps_den: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
    pub positive: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Prompt {
    pub frame: u32,
    #[serde(default)]
    pub points: Vec<Point>,
    /// [x0, y0, x1, y1] en pixels du proxy.
    #[serde(default)]
    pub r#box: Option<[f64; 4]>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum MaskStatus {
    None,
    Stale,
    Ready,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mask {
    pub key: String,
    pub dir: String,
    pub frames: u32,
    pub seconds: f64,
    pub empty_frames: u32,
    /// Images signalées par la détection de dérive (détail dans `<dir>/quality.json`).
    #[serde(default)]
    pub flagged: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Segmentation {
    pub model: String,
    pub dtype: String,
    pub prompts: Vec<Prompt>,
    pub status: MaskStatus,
    pub mask: Option<Mask>,
    /// Nettoyage des îlots parasites et des petits trous à la propagation.
    #[serde(default = "default_true")]
    pub cleanup: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Composite {
    /// Relatif au projet (assets/...). None : fond uni `background_color`.
    pub background: Option<String>,
    pub background_color: String,
    /// Échelle du sujet (1 = taille d'origine).
    pub subject_scale: f64,
    /// Décalage du sujet en fraction de la largeur / hauteur de sortie.
    pub subject_x: f64,
    pub subject_y: f64,
    /// Adoucissement du bord du masque, en pixels du proxy.
    pub feather: f64,
    pub brightness: f64,
    pub contrast: f64,
    pub saturation: f64,
    /// Bord du masque : > 0 rétrécit, < 0 étend (-0,5 à 0,5).
    #[serde(default)]
    pub edge: f64,
    /// Température de couleur du sujet : > 0 plus chaud, < 0 plus froid (-0,5 à 0,5).
    #[serde(default)]
    pub temperature: f64,
}

impl Default for Composite {
    fn default() -> Self {
        Composite {
            background: None,
            background_color: "#1f2a33".into(),
            subject_scale: 1.0,
            subject_x: 0.0,
            subject_y: 0.0,
            feather: 1.5,
            brightness: 0.0,
            contrast: 1.0,
            saturation: 1.0,
            edge: 0.0,
            temperature: 0.0,
        }
    }
}

impl Composite {
    pub fn validate(&self) -> Result<()> {
        let checks = [
            ("échelle", self.subject_scale, 0.1, 4.0),
            ("position X", self.subject_x, -1.0, 1.0),
            ("position Y", self.subject_y, -1.0, 1.0),
            ("adoucissement", self.feather, 0.0, 20.0),
            ("luminosité", self.brightness, -0.5, 0.5),
            ("contraste", self.contrast, 0.5, 2.0),
            ("saturation", self.saturation, 0.0, 2.0),
            ("bord", self.edge, -0.5, 0.5),
            ("température", self.temperature, -0.5, 0.5),
        ];
        for (name, v, lo, hi) in checks {
            if !v.is_finite() || v < lo || v > hi {
                return err(format!("Réglage « {name} » hors limites ({v}, attendu entre {lo} et {hi})."));
            }
        }
        let c = &self.background_color;
        if !(c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|ch| ch.is_ascii_hexdigit())) {
            return err(format!("Couleur de fond invalide : {c} (format #rrggbb)."));
        }
        if let Some(bg) = &self.background {
            if !is_safe_relative(bg) {
                return err("Chemin de décor invalide.");
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportRecord {
    pub path: String,
    pub created_at: String,
    pub seconds: f64,
    pub width: u32,
    pub height: u32,
    pub size_bytes: u64,
}

/// Chemin relatif sans `..` ni racine : seul format accepté dans le manifeste.
pub fn is_safe_relative(p: &str) -> bool {
    let path = Path::new(p);
    !p.is_empty() && path.components().all(|c| matches!(c, Component::Normal(_)))
}

pub fn now() -> String {
    chrono::Local::now().to_rfc3339()
}

pub fn sha256_hex(parts: &[&[u8]]) -> String {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p);
        h.update([0u8]);
    }
    hex::encode(h.finalize())
}

/// Empreinte d'un média sans le lire en entier.
pub fn fingerprint(path: &Path) -> Result<String> {
    let meta = fs::metadata(path)?;
    let mtime = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
    let mut buf = vec![0u8; 1 << 20];
    let mut f = fs::File::open(path)?;
    let n = f.read(&mut buf)?;
    Ok(sha256_hex(&[meta.len().to_string().as_bytes(), mtime.to_string().as_bytes(), &buf[..n]]))
}

pub struct Project {
    pub dir: PathBuf,
    pub manifest: Manifest,
}

impl Project {
    pub fn create(dir: &Path, source: &Path, probe: Probe) -> Result<Project> {
        if dir.join(MANIFEST).exists() {
            return err("Ce dossier contient déjà un projet videeo : ouvrez-le plutôt.");
        }
        fs::create_dir_all(dir)?;
        let dir = dir.canonicalize()?;
        if source.parent().and_then(|p| p.canonicalize().ok()).as_deref() == Some(dir.as_path()) {
            return err("Choisissez un dossier de projet distinct du dossier de la vidéo source.");
        }
        for sub in SUBDIRS {
            fs::create_dir_all(dir.join(sub))?;
        }
        let duration = probe.duration.clamp(0.0, 10.0);
        let ts = now();
        let manifest = Manifest {
            schema_version: SCHEMA_VERSION,
            app_version: env!("CARGO_PKG_VERSION").into(),
            created_at: ts.clone(),
            updated_at: ts,
            source: Source { path: source.display().to_string(), fingerprint: fingerprint(source)?, probe },
            clip: Clip { start: 0.0, duration },
            proxy: None,
            segmentation: Segmentation { model: "tiny".into(), dtype: "bf16".into(), prompts: vec![], status: MaskStatus::None, mask: None, cleanup: true },
            composite: Composite::default(),
            exports: vec![],
            export_settings: ExportSettings::default(),
        };
        let p = Project { dir, manifest };
        p.save()?;
        Ok(p)
    }

    pub fn open(dir: &Path) -> Result<Project> {
        let dir = dir.canonicalize()?;
        let text = fs::read_to_string(dir.join(MANIFEST))
            .map_err(|_| crate::error::AppError::Msg("Aucun project.json dans ce dossier.".into()))?;
        let value: serde_json::Value = serde_json::from_str(&text)?;
        let version = value.get("schema_version").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        if version > SCHEMA_VERSION {
            return err(format!("Projet créé par une version plus récente (schéma {version})."));
        }
        let manifest: Manifest = serde_json::from_value(migrate(value, version)?)?;
        for sub in SUBDIRS {
            fs::create_dir_all(dir.join(sub))?;
        }
        Ok(Project { dir, manifest })
    }

    /// Écriture atomique : fichier temporaire puis renommage.
    pub fn save(&self) -> Result<()> {
        let tmp = self.dir.join("project.json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(&self.manifest)?)?;
        fs::rename(tmp, self.dir.join(MANIFEST))?;
        Ok(())
    }

    pub fn touch_and_save(&mut self) -> Result<()> {
        self.manifest.updated_at = now();
        self.save()
    }

    /// Résout un chemin relatif du manifeste en refusant toute sortie du projet.
    pub fn resolve(&self, rel: &str) -> Result<PathBuf> {
        if !is_safe_relative(rel) {
            return err(format!("Chemin hors du projet refusé : {rel}"));
        }
        Ok(self.dir.join(rel))
    }

    pub fn source_path(&self) -> PathBuf {
        PathBuf::from(&self.manifest.source.path)
    }

    pub fn proxy_key(&self) -> String {
        let m = &self.manifest;
        sha256_hex(&[b"proxy-v1", m.source.fingerprint.as_bytes(), format!("{:.3}", m.clip.start).as_bytes(),
                     format!("{:.3}", m.clip.duration).as_bytes(), PROXY_HEIGHT.to_string().as_bytes()])
    }

    /// Clé du masque : proxy + modèle + précision + invites. Un changement de décor ne l'invalide pas.
    pub fn mask_key(&self) -> Result<String> {
        let m = &self.manifest;
        let proxy = m.proxy.as_ref().map(|p| p.key.clone()).unwrap_or_default();
        let prompts = serde_json::to_string(&m.segmentation.prompts)?;
        let cleanup: &[u8] = if m.segmentation.cleanup { b"cleanup-v1" } else { b"raw" };
        Ok(sha256_hex(&[b"mask-v2", proxy.as_bytes(), m.segmentation.model.as_bytes(), m.segmentation.dtype.as_bytes(), prompts.as_bytes(), cleanup]))
    }

    /// Recalcule le statut du masque après un changement de clip ou d'invites.
    pub fn refresh_mask_status(&mut self) -> Result<()> {
        let key = self.mask_key()?;
        let seg = &mut self.manifest.segmentation;
        seg.status = match &seg.mask {
            Some(m) if m.key == key => MaskStatus::Ready,
            Some(_) => MaskStatus::Stale,
            None => MaskStatus::None,
        };
        Ok(())
    }

    pub fn validate_prompts(&self, prompts: &[Prompt]) -> Result<()> {
        let Some(proxy) = &self.manifest.proxy else { return err("Préparez d'abord l'extrait (proxy).") };
        if prompts.len() > 64 {
            return err("Trop d'images clés (64 au maximum).");
        }
        let (w, h) = (proxy.width as f64, proxy.height as f64);
        for p in prompts {
            if p.frame >= proxy.frames {
                return err(format!("Image clé {} hors de l'extrait ({} images).", p.frame, proxy.frames));
            }
            if p.points.len() > 32 {
                return err("Trop de points sur une image clé (32 au maximum).");
            }
            let inside = |x: f64, y: f64| x.is_finite() && y.is_finite() && (0.0..=w).contains(&x) && (0.0..=h).contains(&y);
            if p.points.iter().any(|pt| !inside(pt.x, pt.y)) {
                return err("Un point est hors de l'image.");
            }
            if let Some(b) = p.r#box {
                if !inside(b[0], b[1]) || !inside(b[2], b[3]) || b[2] - b[0] < 4.0 || b[3] - b[1] < 4.0 {
                    return err("Rectangle de sélection invalide.");
                }
            }
        }
        Ok(())
    }
}

/// Migrations de schéma, appliquées pas à pas jusqu'à la version courante.
fn migrate(mut value: serde_json::Value, from: u32) -> Result<serde_json::Value> {
    let mut v = from;
    while v < SCHEMA_VERSION {
        match v {
            0 => return err("Manifeste sans version de schéma : format non reconnu."),
            // 1 -> 2 : nettoyage du masque (activé), bord, température, préréglage d'export.
            // La clé de masque change de formule : un masque v1 devient « à recalculer ».
            1 => {
                let seg = &mut value["segmentation"];
                seg["cleanup"] = serde_json::Value::Bool(true);
                if let Some(mask) = seg.get_mut("mask").filter(|m| m.is_object()) {
                    mask["flagged"] = 0.into();
                }
                value["composite"]["edge"] = 0.0.into();
                value["composite"]["temperature"] = 0.0.into();
                value["export_settings"] = serde_json::json!({"quality": "balanced"});
            }
            _ => return err(format!("Aucune migration connue depuis le schéma {v}.")),
        }
        v += 1;
        value["schema_version"] = v.into();
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe() -> Probe {
        Probe { duration: 12.0, width: 1280, height: 720, display_width: 1280, display_height: 720, rotation: 0,
                fps_num: 25, fps_den: 1, codec: "h264".into(), has_audio: false, size_bytes: 10 }
    }

    #[test]
    fn safe_relative_paths() {
        assert!(is_safe_relative("assets/bg.png"));
        assert!(!is_safe_relative("../etc/passwd"));
        assert!(!is_safe_relative("/etc/passwd"));
        assert!(!is_safe_relative(""));
    }

    #[test]
    fn create_open_roundtrip_and_cache_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("clip.mp4");
        fs::write(&src, b"fake video").unwrap();
        let dir = tmp.path().join("proj");
        let mut p = Project::create(&dir, &src, probe()).unwrap();
        assert_eq!(p.manifest.clip.duration, 10.0);
        let k1 = p.mask_key().unwrap();
        p.manifest.composite.background = Some("assets/x.png".into());
        assert_eq!(k1, p.mask_key().unwrap(), "le décor ne doit pas invalider le masque");
        p.manifest.segmentation.prompts.push(Prompt { frame: 0, points: vec![Point { x: 1.0, y: 1.0, positive: true }], r#box: None });
        assert_ne!(k1, p.mask_key().unwrap(), "les invites doivent invalider le masque");
        p.save().unwrap();
        let q = Project::open(&dir).unwrap();
        assert_eq!(q.manifest.segmentation.prompts.len(), 1);
        assert!(Project::create(&dir, &src, probe()).is_err(), "pas d'écrasement d'un projet existant");
    }

    #[test]
    fn migrates_v1_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("clip.mp4");
        fs::write(&src, b"fake video").unwrap();
        let dir = tmp.path().join("proj");
        let p = Project::create(&dir, &src, probe()).unwrap();
        // Fabrique un manifeste v1 : sans les champs ajoutés en v2.
        let mut v = serde_json::to_value(&p.manifest).unwrap();
        v["schema_version"] = 1.into();
        v["segmentation"].as_object_mut().unwrap().remove("cleanup");
        v["composite"].as_object_mut().unwrap().remove("edge");
        v["composite"].as_object_mut().unwrap().remove("temperature");
        v.as_object_mut().unwrap().remove("export_settings");
        fs::write(dir.join(MANIFEST), serde_json::to_vec(&v).unwrap()).unwrap();
        let q = Project::open(&dir).unwrap();
        assert_eq!(q.manifest.schema_version, SCHEMA_VERSION);
        assert!(q.manifest.segmentation.cleanup);
        assert_eq!(q.manifest.composite.edge, 0.0);
        assert_eq!(q.manifest.export_settings.quality, ExportQuality::Balanced);
    }

    #[test]
    fn rejects_newer_schema_and_bad_settings() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join(MANIFEST), r#"{"schema_version": 99}"#).unwrap();
        assert!(Project::open(tmp.path()).is_err());
        let c = Composite { subject_scale: 9.0, ..Composite::default() };
        assert!(c.validate().is_err());
        let c = Composite { background: Some("../x.png".into()), ..Composite::default() };
        assert!(c.validate().is_err());
        assert!(Composite::default().validate().is_ok());
    }
}
