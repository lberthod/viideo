//! Commandes Tauri exposées au frontend. Toute validation (chemins, bornes, invites) se fait ici,
//! côté Rust : le frontend ne fait que proposer.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::env::{self, Tools, ToolStatus};
use crate::error::{err, AppError, Result};
use crate::media::{self, ExportJob, Probe};
use crate::project::{self, Composite, ExportRecord, Mask, MaskStatus, Project, Prompt, Proxy, MAX_CLIP_SECONDS};
use crate::worker::Worker;

pub struct AppState {
    pub tools: Tools,
    pub project: Mutex<Option<Project>>,
    pub worker: Mutex<Option<Arc<Worker>>>,
    pub export_child: Arc<Mutex<Option<Child>>>,
    /// Une seule tâche lourde à la fois (propagation ou export).
    pub busy: Mutex<Option<&'static str>>,
}

impl AppState {
    pub fn new() -> Self {
        AppState { tools: Tools::detect(), project: Mutex::new(None), worker: Mutex::new(None),
                   export_child: Arc::new(Mutex::new(None)), busy: Mutex::new(None) }
    }
}

/// Garde de tâche lourde : libère le verrou `busy` à la sortie, même en cas d'erreur.
struct BusyGuard<'a>(&'a Mutex<Option<&'static str>>);

impl<'a> BusyGuard<'a> {
    fn acquire(m: &'a Mutex<Option<&'static str>>, what: &'static str) -> Result<Self> {
        let mut b = m.lock().unwrap();
        if let Some(cur) = *b {
            return err(format!("Une tâche est déjà en cours ({cur}). Attendez sa fin ou annulez-la."));
        }
        *b = Some(what);
        Ok(BusyGuard(m))
    }
}

impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        *self.0.lock().unwrap() = None;
    }
}

#[derive(Serialize)]
pub struct ProjectView {
    pub dir: String,
    pub manifest: project::Manifest,
    pub source_missing: bool,
    pub proxy_dir: Option<String>,
    pub mask_dir: Option<String>,
    pub background_path: Option<String>,
}

fn view(p: &Project) -> ProjectView {
    let m = &p.manifest;
    let abs = |rel: &str| p.resolve(rel).ok().filter(|x| x.exists()).map(|x| x.display().to_string());
    ProjectView {
        dir: p.dir.display().to_string(),
        manifest: m.clone(),
        source_missing: !p.source_path().exists(),
        proxy_dir: m.proxy.as_ref().and_then(|x| abs(&x.dir)),
        mask_dir: m.segmentation.mask.as_ref().filter(|_| m.segmentation.status == MaskStatus::Ready).and_then(|x| abs(&x.dir)),
        background_path: m.composite.background.as_deref().and_then(abs),
    }
}

fn with_project<T>(state: &AppState, f: impl FnOnce(&mut Project) -> Result<T>) -> Result<T> {
    let mut guard = state.project.lock().unwrap();
    let p = guard.as_mut().ok_or_else(|| AppError::Msg("Aucun projet ouvert.".into()))?;
    f(p)
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| AppError::Msg(format!("Tâche interrompue : {e}")))?
}

fn replace_worker(state: &AppState) {
    if let Some(w) = state.worker.lock().unwrap().take() {
        w.shutdown();
    }
}

#[tauri::command]
pub async fn check_environment(state: State<'_, AppState>) -> Result<Vec<ToolStatus>> {
    let tools = state.tools.clone();
    blocking(move || Ok(env::check(&tools))).await
}

#[tauri::command]
pub async fn probe_video(state: State<'_, AppState>, path: String) -> Result<Probe> {
    let ffprobe = state.tools.ffprobe.clone();
    blocking(move || media::probe(&ffprobe, Path::new(&path))).await
}

#[tauri::command]
pub async fn create_project(state: State<'_, AppState>, project_dir: String, source: String) -> Result<ProjectView> {
    let src = PathBuf::from(&source).canonicalize().map_err(|_| AppError::Msg("Vidéo source introuvable.".into()))?;
    let ffprobe = state.tools.ffprobe.clone();
    let probe = blocking({ let src = src.clone(); move || media::probe(&ffprobe, &src) }).await?;
    let p = Project::create(Path::new(&project_dir), &src, probe)?;
    replace_worker(&state);
    let v = view(&p);
    *state.project.lock().unwrap() = Some(p);
    Ok(v)
}

#[tauri::command]
pub async fn open_project(state: State<'_, AppState>, project_dir: String) -> Result<ProjectView> {
    let mut p = Project::open(Path::new(&project_dir))?;
    // Les caches peuvent avoir été nettoyés : on ne garde que ce qui existe encore sur disque.
    if let Some(px) = &p.manifest.proxy {
        if !p.resolve(&px.dir)?.exists() {
            p.manifest.proxy = None;
        }
    }
    if let Some(m) = &p.manifest.segmentation.mask {
        if !p.resolve(&m.dir)?.exists() {
            p.manifest.segmentation.mask = None;
        }
    }
    p.refresh_mask_status()?;
    replace_worker(&state);
    let v = view(&p);
    *state.project.lock().unwrap() = Some(p);
    Ok(v)
}

/// Mode développement : VIDEEO_DEV_PROJECT (+ VIDEEO_DEV_SOURCE pour le créer) ouvre un projet au démarrage.
#[tauri::command]
pub async fn dev_autoload(state: State<'_, AppState>) -> Result<Option<ProjectView>> {
    let Some(dir) = std::env::var_os("VIDEEO_DEV_PROJECT") else { return Ok(None) };
    let dir = dir.to_string_lossy().to_string();
    if Path::new(&dir).join(project::MANIFEST).exists() {
        return open_project(state, dir).await.map(Some);
    }
    match std::env::var("VIDEEO_DEV_SOURCE") {
        Ok(src) => create_project(state, dir, src).await.map(Some),
        Err(_) => Ok(None),
    }
}

/// Mode développement : VIDEEO_DEV_DEMO=<image de décor> déroule un scénario complet au démarrage.
#[tauri::command]
pub fn dev_demo() -> Option<String> {
    std::env::var("VIDEEO_DEV_DEMO").ok()
}

#[tauri::command]
pub fn dev_log(msg: String) {
    println!("{msg}");
}

#[tauri::command]
pub fn close_project(state: State<'_, AppState>) {
    replace_worker(&state);
    *state.project.lock().unwrap() = None;
}

#[tauri::command]
pub fn set_clip(state: State<'_, AppState>, start: f64, duration: f64) -> Result<ProjectView> {
    with_project(&state, |p| {
        let total = p.manifest.source.probe.duration;
        if !(start.is_finite() && duration.is_finite()) || start < 0.0 || start >= total {
            return err(format!("Début invalide : il doit être compris entre 0 et {total:.2} s."));
        }
        if duration < 0.2 || duration > MAX_CLIP_SECONDS {
            return err(format!("Durée invalide : entre 0,2 et {MAX_CLIP_SECONDS} s."));
        }
        p.manifest.clip.start = start;
        p.manifest.clip.duration = duration.min(total - start);
        if p.manifest.proxy.as_ref().map(|x| x.key != p.proxy_key()).unwrap_or(false) {
            p.manifest.proxy = None;
            // Les images clés n'ont plus de sens sur un autre extrait.
            p.manifest.segmentation.prompts.clear();
        }
        p.refresh_mask_status()?;
        p.touch_and_save()?;
        Ok(view(p))
    })
}

#[tauri::command]
pub async fn prepare_proxy(state: State<'_, AppState>) -> Result<ProjectView> {
    let (key, src, start, duration, fps, dir) = with_project(&state, |p| {
        let m = &p.manifest;
        if !p.source_path().exists() {
            return err("La vidéo source est introuvable (déplacée ou supprimée).");
        }
        let key = p.proxy_key();
        let dir = format!("media/proxy_{}", &key[..16]);
        Ok((key, p.source_path(), m.clip.start, m.clip.duration, (m.source.probe.fps_num, m.source.probe.fps_den), dir))
    })?;
    let already = with_project(&state, |p| Ok(p.manifest.proxy.as_ref().map(|x| x.key == key).unwrap_or(false) && p.resolve(&dir)?.exists()))?;
    if !already {
        let dest = with_project(&state, |p| p.resolve(&dir))?;
        let (ffmpeg, ffprobe) = (state.tools.ffmpeg.clone(), state.tools.ffprobe.clone());
        let r = blocking(move || media::extract_proxy(&ffmpeg, &ffprobe, &src, start, duration, fps, &dest)).await?;
        with_project(&state, |p| {
            p.manifest.proxy = Some(Proxy { key: key.clone(), dir: dir.clone(), width: r.width, height: r.height,
                                            frames: r.frames, fps_num: fps.0, fps_den: fps.1 });
            p.manifest.segmentation.prompts.retain(|pr| pr.frame < r.frames);
            p.refresh_mask_status()?;
            p.touch_and_save()
        })?;
    }
    with_project(&state, |p| Ok(view(p)))
}

#[tauri::command]
pub fn set_prompts(state: State<'_, AppState>, prompts: Vec<Prompt>) -> Result<ProjectView> {
    with_project(&state, |p| {
        p.validate_prompts(&prompts)?;
        p.manifest.segmentation.prompts = prompts;
        p.refresh_mask_status()?;
        p.touch_and_save()?;
        Ok(view(p))
    })
}

/// Démarre (ou réutilise) le worker du projet, charge le modèle et la vidéo proxy.
fn ensure_worker(state: &AppState, app: &AppHandle) -> Result<Arc<Worker>> {
    let (root, proxy_key, proxy_dir, model, dtype) = with_project(state, |p| {
        let px = p.manifest.proxy.as_ref().ok_or_else(|| AppError::Msg("Préparez d'abord l'extrait (proxy).".into()))?;
        Ok((p.dir.clone(), px.key.clone(), p.resolve(&px.dir)?, p.manifest.segmentation.model.clone(), p.manifest.segmentation.dtype.clone()))
    })?;
    let mut slot = state.worker.lock().unwrap();
    let reuse = slot.as_ref().filter(|w| w.is_alive() && w.root == root).cloned();
    let w = match reuse {
        Some(w) => w,
        None => {
            if let Some(old) = slot.take() {
                old.shutdown();
            }
            let app = app.clone();
            let w = Worker::spawn(&state.tools, &root, move |ev| {
                let _ = app.emit("mask-progress", json!({"done": ev["done"], "total": ev["total"]}));
            })?;
            *slot = Some(w.clone());
            w
        }
    };
    drop(slot);
    w.request("load_model", json!({"model": model, "dtype": dtype}))?;
    let mut loaded = w.loaded_video.lock().unwrap();
    if loaded.as_deref() != Some(proxy_key.as_str()) {
        w.request("init_video", json!({"frames_dir": proxy_dir}))?;
        *loaded = Some(proxy_key);
    }
    Ok(w.clone())
}

/// `ensure_worker` peut bloquer plusieurs secondes (démarrage Python, chargement du modèle) :
/// on l'exécute hors du runtime async.
async fn worker_for(app: &AppHandle) -> Result<Arc<Worker>> {
    let app = app.clone();
    blocking(move || ensure_worker(&app.state::<AppState>(), &app)).await
}

#[derive(Serialize)]
pub struct PreviewResult {
    pub frame: u32,
    pub png: String,
    pub area_pct: f64,
}

#[tauri::command]
pub async fn preview_mask(app: AppHandle, state: State<'_, AppState>, frame: u32) -> Result<PreviewResult> {
    let _busy = BusyGuard::acquire(&state.busy, "aperçu du masque")?;
    let (prompts, out) = with_project(&state, |p| {
        let prompts: Vec<Prompt> = p.manifest.segmentation.prompts.iter().filter(|pr| pr.frame == frame).cloned().collect();
        if prompts.is_empty() {
            return err("Ajoutez au moins un point ou un rectangle sur cette image.");
        }
        let out = p.dir.join(format!("cache/preview/f{frame:05}_{}.png", chrono::Local::now().timestamp_millis()));
        Ok((prompts, out))
    })?;
    let w = worker_for(&app).await?;
    let r = blocking(move || {
        if let Some(dir) = out.parent() {
            // On ne garde pas les anciens aperçus.
            if let Ok(rd) = fs::read_dir(dir) {
                for e in rd.flatten() {
                    let _ = fs::remove_file(e.path());
                }
            }
        }
        w.request("preview", json!({"out_png": out, "prompts": prompts}))
    }).await?;
    Ok(PreviewResult {
        frame,
        png: r["out_png"].as_str().unwrap_or_default().to_string(),
        area_pct: r["area_pct"].as_f64().unwrap_or(0.0),
    })
}

#[tauri::command]
pub async fn propagate_mask(app: AppHandle, state: State<'_, AppState>) -> Result<ProjectView> {
    let _busy = BusyGuard::acquire(&state.busy, "propagation du masque")?;
    let (key, rel_dir, out_dir, prompts) = with_project(&state, |p| {
        if p.manifest.segmentation.prompts.is_empty() {
            return err("Sélectionnez d'abord le sujet (points ou rectangle).");
        }
        let key = p.mask_key()?;
        let rel = format!("cache/masks/{}", &key[..16]);
        Ok((key, rel.clone(), p.resolve(&rel)?, p.manifest.segmentation.prompts.clone()))
    })?;
    let w = worker_for(&app).await?;
    let r = blocking(move || {
        let _ = fs::remove_dir_all(&out_dir);
        w.request("propagate", json!({"out_dir": out_dir, "prompts": prompts}))
    }).await?;
    with_project(&state, |p| {
        // Les anciens masques sont régénérables : on ne garde que le courant.
        if let Ok(rd) = fs::read_dir(p.dir.join("cache/masks")) {
            for e in rd.flatten() {
                if e.path() != p.dir.join(&rel_dir) {
                    let _ = fs::remove_dir_all(e.path());
                }
            }
        }
        p.manifest.segmentation.mask = Some(Mask {
            key,
            dir: rel_dir,
            frames: r["written"].as_u64().unwrap_or(0) as u32,
            seconds: r["seconds"].as_f64().unwrap_or(0.0),
            empty_frames: r["empty_frames"].as_u64().unwrap_or(0) as u32,
        });
        p.refresh_mask_status()?;
        p.touch_and_save()?;
        Ok(view(p))
    })
}

#[tauri::command]
pub fn cancel_job(state: State<'_, AppState>) -> Result<()> {
    if let Some(w) = state.worker.lock().unwrap().as_ref() {
        w.cancel()?;
    }
    if let Some(child) = state.export_child.lock().unwrap().as_mut() {
        let _ = child.kill();
    }
    Ok(())
}

#[tauri::command]
pub fn unload_models(state: State<'_, AppState>) {
    replace_worker(&state);
}

const IMAGE_EXT: [&str; 4] = ["png", "jpg", "jpeg", "webp"];

#[tauri::command]
pub fn set_background(state: State<'_, AppState>, path: Option<String>) -> Result<ProjectView> {
    with_project(&state, |p| {
        match path {
            None => p.manifest.composite.background = None,
            Some(src) => {
                let src = PathBuf::from(src);
                let ext = src.extension().and_then(|e| e.to_str()).map(|e| e.to_lowercase()).unwrap_or_default();
                if !IMAGE_EXT.contains(&ext.as_str()) {
                    return err("Format de décor non pris en charge (PNG, JPEG ou WebP).");
                }
                let size = fs::metadata(&src)?.len();
                if size > 100 * 1024 * 1024 {
                    return err("Image de décor trop lourde (100 Mo au maximum).");
                }
                let rel = format!("assets/background_{}.{ext}", &project::fingerprint(&src)?[..16]);
                let dest = p.resolve(&rel)?;
                if !dest.exists() {
                    fs::copy(&src, &dest)?;
                }
                p.manifest.composite.background = Some(rel);
            }
        }
        p.touch_and_save()?;
        Ok(view(p))
    })
}

#[tauri::command]
pub fn set_composite(state: State<'_, AppState>, composite: Composite) -> Result<ProjectView> {
    composite.validate()?;
    with_project(&state, |p| {
        if let Some(bg) = &composite.background {
            if !bg.starts_with("assets/") || !p.resolve(bg)?.exists() {
                return err("Le décor doit être importé dans le projet.");
            }
        }
        p.manifest.composite = composite;
        p.touch_and_save()?;
        Ok(view(p))
    })
}

#[tauri::command]
pub async fn export_video(app: AppHandle, state: State<'_, AppState>) -> Result<ExportRecord> {
    let _busy = BusyGuard::acquire(&state.busy, "export")?;
    let (job, rel) = with_project(&state, |p| {
        let m = &p.manifest;
        if m.segmentation.status != MaskStatus::Ready {
            return err("Calculez d'abord le masque sur tout l'extrait.");
        }
        if !p.source_path().exists() {
            return err("La vidéo source est introuvable.");
        }
        let proxy = m.proxy.as_ref().ok_or_else(|| AppError::Msg("Proxy manquant.".into()))?;
        let mask = m.segmentation.mask.as_ref().ok_or_else(|| AppError::Msg("Masque manquant.".into()))?;
        let (w, h) = media::output_size(m.source.probe.display_width, m.source.probe.display_height);
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let rel = format!("exports/videeo_{stamp}.mp4");
        Ok((ExportJob {
            src: p.source_path(),
            start: m.clip.start,
            duration: m.clip.duration,
            fps: (proxy.fps_num, proxy.fps_den),
            has_audio: m.source.probe.has_audio,
            out_width: w,
            out_height: h,
            proxy_height: proxy.height,
            masks_dir: p.resolve(&mask.dir)?,
            background: m.composite.background.as_deref().map(|b| p.resolve(b)).transpose()?,
            composite: m.composite.clone(),
            dest: p.resolve(&rel)?,
        }, rel))
    })?;
    let (ffmpeg, ffprobe) = (state.tools.ffmpeg.clone(), state.tools.ffprobe.clone());
    let slot = state.export_child.clone();
    let (dest, w, h) = (job.dest.clone(), job.out_width, job.out_height);
    let started = std::time::Instant::now();
    blocking(move || media::export(&ffmpeg, &job, slot, |f| { let _ = app.emit("export-progress", json!({"fraction": f})); })).await?;
    let check = blocking({ let dest = dest.clone(); move || media::probe(&ffprobe, &dest) }).await?;
    let record = ExportRecord {
        path: rel,
        created_at: project::now(),
        seconds: (started.elapsed().as_secs_f64() * 100.0).round() / 100.0,
        width: w,
        height: h,
        size_bytes: check.size_bytes,
    };
    with_project(&state, |p| {
        p.manifest.exports.push(record.clone());
        p.touch_and_save()
    })?;
    Ok(record)
}

#[tauri::command]
pub fn project_path(state: State<'_, AppState>, rel: String) -> Result<String> {
    with_project(&state, |p| Ok(p.resolve(&rel)?.display().to_string()))
}
