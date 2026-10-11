//! Commandes Tauri exposées au frontend. Toute validation (chemins, bornes, invites) se fait ici,
//! côté Rust : le frontend ne fait que proposer.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::backgrounds::{self, BackgroundItem};
use crate::comfy;
use crate::llm;
use crate::video;
use crate::plan::{self, Mode, Plan};
use crate::env::{self, Tools, ToolStatus};
use crate::error::{err, AppError, Result};
use crate::media::{self, ExportJob, Probe};
use crate::project::{self, Composite, ExportQuality, ExportRecord, GenerationSettings, Mask, MaskStatus, Project, Prompt, Proxy, MAX_CLIP_SECONDS};
use crate::worker::Worker;

pub struct AppState {
    pub tools: Tools,
    pub project: Mutex<Option<Project>>,
    pub worker: Mutex<Option<Arc<Worker>>>,
    /// Worker de génération d'image. Jamais chargé en même temps que le worker vision :
    /// un seul modèle lourd en mémoire (SAM 2 ≤ 7 Go, FLUX.2 klein ≈ 24 Go).
    pub image_worker: Mutex<Option<Arc<Worker>>>,
    pub export_child: Arc<Mutex<Option<Child>>>,
    /// Processus d'animation de décor en cours (moteur vidéo).
    pub video_child: Arc<Mutex<Option<Child>>>,
    /// Annulation d'une génération via ComfyUI.
    pub comfy_cancel: Arc<std::sync::atomic::AtomicBool>,
    /// Une seule tâche lourde à la fois (propagation, génération ou export).
    pub busy: Mutex<Option<&'static str>>,
    /// Planificateur LLM : (adresse Ollama, modèle). Réglage de l'application, pas du projet.
    pub planner: Mutex<(String, String)>,
}

impl AppState {
    pub fn new() -> Self {
        AppState { tools: Tools::detect(), project: Mutex::new(None), worker: Mutex::new(None), image_worker: Mutex::new(None),
                   export_child: Arc::new(Mutex::new(None)), video_child: Arc::new(Mutex::new(None)), comfy_cancel: Arc::new(Default::default()), busy: Mutex::new(None),
                   planner: Mutex::new((llm::DEFAULT_URL.into(), llm::DEFAULT_MODEL.into())) }
    }
}

fn stop_image_worker(state: &AppState) {
    let w = state.image_worker.lock().unwrap().take();
    if let Some(w) = w {
        w.shutdown();
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
    /// Ombres de contact calculées pour le masque courant.
    pub shadow_dir: Option<String>,
    /// Normales approchées du sujet (relighting), calculées pour le masque courant.
    pub normals_dir: Option<String>,
    pub element_path: Option<String>,
}

/// Dossier des ombres de contact : dérivé de la clé du masque (régénérable, invalidé avec lui).
fn shadow_rel(p: &Project) -> Option<String> {
    let seg = &p.manifest.segmentation;
    seg.mask.as_ref().filter(|_| seg.status == MaskStatus::Ready).map(|m| format!("cache/shadows/{}", &m.key[..16]))
}

/// Dossier des normales du sujet : même principe que les ombres.
fn normals_rel(p: &Project) -> Option<String> {
    shadow_rel(p).map(|r| r.replace("cache/shadows/", "cache/normals/"))
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
        shadow_dir: shadow_rel(p).and_then(|r| abs(&r)),
        normals_dir: normals_rel(p).and_then(|r| abs(&r)),
        element_path: m.composite.foreground.as_ref().and_then(|f| abs(&f.path)),
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
    let w = state.worker.lock().unwrap().take();
    if let Some(w) = w {
        w.shutdown();
    }
    stop_image_worker(state);
}

fn stop_vision_worker(state: &AppState) {
    let w = state.worker.lock().unwrap().take();
    if let Some(w) = w {
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

/// Mode développement : VIDEEO_DEV_DEMO=<fichier JSON du scénario> déroule un parcours complet au démarrage.
/// Une seule fois par processus : le rechargement à chaud du frontend ne relance pas le scénario.
#[tauri::command]
pub fn dev_demo() -> Option<String> {
    static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if STARTED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return None;
    }
    std::env::var("VIDEEO_DEV_DEMO").ok().and_then(|p| fs::read_to_string(p).ok())
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

/// Démarre (ou réutilise) le worker du projet et charge le modèle.
fn ensure_worker(state: &AppState, app: &AppHandle) -> Result<Arc<Worker>> {
    let (root, model, dtype) = with_project(state, |p| {
        if p.manifest.proxy.is_none() {
            return err("Préparez d'abord l'extrait (proxy).");
        }
        Ok((p.dir.clone(), p.manifest.segmentation.model.clone(), p.manifest.segmentation.dtype.clone()))
    })?;
    let mut slot = state.worker.lock().unwrap();
    let reuse = slot.as_ref().filter(|w| w.is_alive() && w.root == root).cloned();
    let w = match reuse {
        Some(w) => w,
        None => {
            stop_image_worker(state);
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
    // Le worker ne charge plus la vidéo entière : aperçu et propagation lisent seulement les images utiles.
    w.request("load_model", json!({"model": model, "dtype": dtype}))?;
    Ok(w)
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
    let (prompts, out, frames_dir) = with_project(&state, |p| {
        let prompts: Vec<Prompt> = p.manifest.segmentation.prompts.iter().filter(|pr| pr.frame == frame).cloned().collect();
        if prompts.is_empty() {
            return err("Ajoutez au moins un point ou un rectangle sur cette image.");
        }
        let px = p.manifest.proxy.as_ref().ok_or_else(|| AppError::Msg("Proxy manquant.".into()))?;
        let out = p.dir.join(format!("cache/preview/f{frame:05}_{}.png", chrono::Local::now().timestamp_millis()));
        Ok((prompts, out, p.resolve(&px.dir)?))
    })?;
    let w = worker_for(&app).await?;
    let r = blocking(move || {
        if let Some(dir) = out.parent() {
            // On ne garde pas les anciens aperçus.
            if let Ok(rd) = fs::read_dir(dir) {
                for e in rd.flatten() {
                    if e.path().is_file() {
                        let _ = fs::remove_file(e.path());
                    }
                }
            }
        }
        w.request("preview", json!({"frames_dir": frames_dir, "frame": frame, "out_png": out, "prompts": prompts}))
    }).await?;
    Ok(PreviewResult {
        frame,
        png: r["out_png"].as_str().unwrap_or_default().to_string(),
        area_pct: r["area_pct"].as_f64().unwrap_or(0.0),
    })
}

/// Taille des blocs de propagation : ~5 Go de RAM au plus (≈ 12,6 Mo par image, mesuré à l'étape 0).
const CHUNK_FRAMES: u32 = 400;

#[tauri::command]
pub async fn propagate_mask(app: AppHandle, state: State<'_, AppState>) -> Result<ProjectView> {
    let _busy = BusyGuard::acquire(&state.busy, "propagation du masque")?;
    let (key, rel_dir, out_dir, prompts, cleanup, frames_dir) = with_project(&state, |p| {
        if p.manifest.segmentation.prompts.is_empty() {
            return err("Sélectionnez d'abord le sujet (points ou rectangle).");
        }
        let key = p.mask_key()?;
        let rel = format!("cache/masks/{}", &key[..16]);
        let cleanup = p.manifest.segmentation.cleanup.then(|| json!({"min_island_ratio": 0.05, "max_hole_ratio": 0.002}));
        let px = p.manifest.proxy.as_ref().ok_or_else(|| AppError::Msg("Proxy manquant.".into()))?;
        Ok((key, rel.clone(), p.resolve(&rel)?, p.manifest.segmentation.prompts.clone(), cleanup, p.resolve(&px.dir)?))
    })?;
    let w = worker_for(&app).await?;
    // Le dossier du masque n'est pas vidé : s'il contient une propagation interrompue pour les mêmes
    // invites, le worker reprend au premier bloc non terminé (chunks.json).
    let r = blocking(move || {
        w.request("propagate", json!({"frames_dir": frames_dir, "out_dir": out_dir, "prompts": prompts,
                                      "cleanup": cleanup, "chunk_size": CHUNK_FRAMES, "resume": true}))
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
            flagged: r["flagged"].as_u64().unwrap_or(0) as u32,
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
    if let Some(w) = state.image_worker.lock().unwrap().as_ref() {
        w.cancel()?;
    }
    state.comfy_cancel.store(true, std::sync::atomic::Ordering::SeqCst);
    if let Some(child) = state.export_child.lock().unwrap().as_mut() {
        let _ = child.kill();
    }
    if let Some(child) = state.video_child.lock().unwrap().as_mut() {
        let _ = child.kill();
    }
    Ok(())
}

#[tauri::command]
pub fn unload_models(state: State<'_, AppState>) {
    replace_worker(&state);
}

/// Importe une image externe dans la bibliothèque du projet et l'utilise comme décor (None : fond uni).
#[tauri::command]
pub fn set_background(state: State<'_, AppState>, path: Option<String>) -> Result<ProjectView> {
    with_project(&state, |p| {
        p.manifest.composite.background = match path {
            None => None,
            Some(src) => Some(backgrounds::import(p, Path::new(&src))?),
        };
        p.touch_and_save()?;
        Ok(view(p))
    })
}

/// Choisit un décor déjà présent dans la bibliothèque du projet.
#[tauri::command]
pub fn select_background(state: State<'_, AppState>, rel: String) -> Result<ProjectView> {
    with_project(&state, |p| {
        if !rel.starts_with("assets/") || !p.resolve(&rel)?.exists() {
            return err("Ce décor n'existe pas dans le projet.");
        }
        p.manifest.composite.background = Some(rel);
        p.touch_and_save()?;
        Ok(view(p))
    })
}

#[tauri::command]
pub fn list_backgrounds(state: State<'_, AppState>) -> Result<Vec<BackgroundItem>> {
    with_project(&state, |p| Ok(backgrounds::list(p)))
}

#[tauri::command]
pub fn delete_background(state: State<'_, AppState>, rel: String) -> Result<Vec<BackgroundItem>> {
    with_project(&state, |p| {
        backgrounds::delete(p, &rel)?;
        Ok(backgrounds::list(p))
    })
}

#[derive(Serialize)]
pub struct ImageBackends {
    pub builtin_ok: bool,
    pub builtin_detail: String,
    pub comfy: comfy::ServerStatus,
    pub settings: GenerationSettings,
}

#[tauri::command]
pub async fn image_backends(state: State<'_, AppState>) -> Result<ImageBackends> {
    let settings = with_project(&state, |p| Ok(p.manifest.generation.clone()))?;
    let tools = state.tools.clone();
    blocking(move || {
        let builtin_ok = tools.image_python.exists() && tools.image_worker.exists();
        let workflows = comfy::load_workflows(&tools.workflows).unwrap_or_default();
        let comfy = comfy::status(&settings.comfy_url, &workflows);
        Ok(ImageBackends {
            builtin_ok,
            builtin_detail: if builtin_ok { "FLUX.2 [klein] 4B (Apache-2.0), local, depuis le cache Hugging Face".into() }
                            else { format!("environnement absent : {}", tools.image_python.display()) },
            comfy,
            settings,
        })
    }).await
}

#[tauri::command]
pub fn set_generation(state: State<'_, AppState>, settings: GenerationSettings) -> Result<ProjectView> {
    comfy::validate_url(&settings.comfy_url)?;
    if settings.backend != "builtin" && !settings.backend.starts_with("comfyui:") {
        return err("Moteur de génération inconnu.");
    }
    with_project(&state, |p| {
        p.manifest.generation = settings;
        p.touch_and_save()?;
        Ok(view(p))
    })
}

fn image_worker(state: &AppState, app: &AppHandle, root: &Path) -> Result<Arc<Worker>> {
    let mut slot = state.image_worker.lock().unwrap();
    if let Some(w) = slot.as_ref().filter(|w| w.is_alive() && w.root == root) {
        return Ok(w.clone());
    }
    if let Some(old) = slot.take() {
        old.shutdown();
    }
    stop_vision_worker(state);
    let app = app.clone();
    let w = Worker::spawn_image(&state.tools, root, move |ev| {
        let _ = app.emit("image-progress", json!({"done": ev["done"], "total": ev["total"]}));
    })?;
    *slot = Some(w.clone());
    Ok(w)
}

/// Génère `count` décors (graines successives) avec le moteur du projet et les ajoute à la bibliothèque.
#[tauri::command]
pub async fn generate_backgrounds(app: AppHandle, state: State<'_, AppState>, prompt: String, count: u32, seed: Option<u32>) -> Result<Vec<BackgroundItem>> {
    let _busy = BusyGuard::acquire(&state.busy, "génération de décor")?;
    let prompt = prompt.trim().to_string();
    if prompt.is_empty() || prompt.chars().count() > 1500 {
        return err("Décrivez le décor (1 500 caractères au maximum).");
    }
    if !(1..=4).contains(&count) {
        return err("Entre 1 et 4 variantes.");
    }
    let (root, settings, (w, h)) = with_project(&state, |p| {
        let pr = &p.manifest.source.probe;
        let (ow, oh) = media::output_size(pr.display_width, pr.display_height);
        Ok((p.dir.clone(), p.manifest.generation.clone(), backgrounds::generation_size(ow, oh)))
    })?;
    let base_seed = seed.unwrap_or_else(|| (chrono::Local::now().timestamp_subsec_nanos() ^ 0x5bd1e995) & 0x00FF_FFFF);
    state.comfy_cancel.store(false, std::sync::atomic::Ordering::SeqCst);

    let mut made = Vec::new();
    for i in 0..count {
        let seed_i = (base_seed + i) & 0x00FF_FFFF;
        let (rel, abs) = with_project(&state, |p| backgrounds::new_generated(p, seed_i))?;
        let _ = app.emit("image-progress", json!({"index": i, "count": count, "done": 0, "total": 0}));
        let meta = if settings.backend == "builtin" {
            let app2 = app.clone();
            let root2 = root.clone();
            let w_img = blocking(move || image_worker(&app2.state::<AppState>(), &app2, &root2)).await?;
            let (pr, out) = (prompt.clone(), abs.clone());
            let r = blocking(move || w_img.request("generate", json!({"prompt": pr, "width": w, "height": h, "seed": seed_i, "steps": 4, "out_png": out}))).await?;
            let mut m = r;
            if let Some(o) = m.as_object_mut() {
                o.remove("out_png");
            }
            m
        } else {
            let wf_id = settings.backend.trim_start_matches("comfyui:").to_string();
            // ComfyUI a sa propre mémoire : on libère nos modèles avant.
            stop_vision_worker(&state);
            stop_image_worker(&state);
            let tools = state.tools.clone();
            let url = settings.comfy_url.clone();
            let cancel = state.comfy_cancel.clone();
            let (pr, out) = (prompt.clone(), abs.clone());
            blocking(move || {
                let wfs = comfy::load_workflows(&tools.workflows)?;
                let wf = wfs.iter().filter(|x| x.id == wf_id).max_by_key(|x| x.version)
                    .ok_or_else(|| AppError::Msg(format!("Workflow ComfyUI inconnu : {wf_id}")))?;
                let mut params = std::collections::BTreeMap::new();
                params.insert("prompt".into(), json!(pr));
                params.insert("width".into(), json!(w));
                params.insert("height".into(), json!(h));
                params.insert("steps".into(), json!(4));
                params.insert("seed".into(), json!(seed_i));
                let r = comfy::run(&url, wf, &params, &out, &|| cancel.load(std::sync::atomic::Ordering::SeqCst))?;
                Ok(json!({"backend": "comfyui", "workflow": wf.id, "workflow_version": wf.version, "workflow_tested": wf.tested,
                          "comfy_prompt_id": r.prompt_id, "prompt": pr, "width": w, "height": h, "steps": 4, "seed": seed_i,
                          "seconds": r.seconds}))
            }).await?
        };
        let mut meta = meta;
        meta["source"] = json!("generated");
        meta["created_at"] = json!(project::now());
        backgrounds::write_meta(&abs, &meta)?;
        made.push(rel);
    }
    with_project(&state, |p| {
        let all = backgrounds::list(p);
        Ok(all.into_iter().filter(|b| made.contains(&b.path)).collect())
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
        if let Some(f) = &composite.foreground {
            if !p.resolve(&f.path)?.exists() {
                return err("Élément de premier plan introuvable dans le projet.");
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
            quality: m.export_settings.quality.clone(),
            videotoolbox: state.tools.videotoolbox,
            shadows_dir: shadow_rel(p).map(|r| p.resolve(&r)).transpose()?.filter(|d| d.exists()),
            element: m.composite.foreground.as_ref().map(|f| p.resolve(&f.path)).transpose()?.filter(|e| e.exists()),
            normals_dir: normals_rel(p).map(|r| p.resolve(&r)).transpose()?.filter(|d| d.exists()),
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

/// Statistiques par image et images suspectes du masque courant (quality.json écrit par le worker).
#[tauri::command]
pub fn mask_quality(state: State<'_, AppState>) -> Result<Option<serde_json::Value>> {
    with_project(&state, |p| {
        let seg = &p.manifest.segmentation;
        let Some(mask) = seg.mask.as_ref().filter(|_| seg.status == MaskStatus::Ready) else { return Ok(None) };
        let path = p.resolve(&mask.dir)?.join("quality.json");
        match fs::read_to_string(path) {
            Ok(text) => Ok(Some(serde_json::from_str(&text)?)),
            Err(_) => Ok(None),
        }
    })
}

#[tauri::command]
pub fn set_cleanup(state: State<'_, AppState>, enabled: bool) -> Result<ProjectView> {
    with_project(&state, |p| {
        p.manifest.segmentation.cleanup = enabled;
        p.refresh_mask_status()?;
        p.touch_and_save()?;
        Ok(view(p))
    })
}

#[tauri::command]
pub fn set_export_quality(state: State<'_, AppState>, quality: ExportQuality) -> Result<ProjectView> {
    with_project(&state, |p| {
        p.manifest.export_settings.quality = quality;
        p.touch_and_save()?;
        Ok(view(p))
    })
}

fn dir_size(path: &Path) -> u64 {
    let Ok(rd) = fs::read_dir(path) else { return 0 };
    rd.flatten().map(|e| match e.metadata() {
        Ok(m) if m.is_dir() => dir_size(&e.path()),
        Ok(m) => m.len(),
        Err(_) => 0,
    }).sum()
}

#[derive(Serialize)]
pub struct DiskUsage {
    pub proxy_current: u64,
    pub proxy_stale: u64,
    pub masks_current: u64,
    pub masks_stale: u64,
    pub previews: u64,
    pub assets: u64,
    pub exports: u64,
}

/// Répartit l'espace disque du projet entre éléments courants et caches périmés.
fn disk_usage_of(p: &Project) -> DiskUsage {
    let cur_proxy = p.manifest.proxy.as_ref().map(|x| p.dir.join(&x.dir));
    let cur_mask = p.manifest.segmentation.mask.as_ref().map(|x| p.dir.join(&x.dir));
    let split = |dir: &Path, current: &Option<PathBuf>| -> (u64, u64) {
        let (mut cur, mut stale) = (0, 0);
        for e in fs::read_dir(dir).into_iter().flatten().flatten() {
            let size = if e.path().is_dir() { dir_size(&e.path()) } else { e.metadata().map(|m| m.len()).unwrap_or(0) };
            if current.as_deref() == Some(e.path().as_path()) { cur += size } else { stale += size }
        }
        (cur, stale)
    };
    let (proxy_current, proxy_stale) = split(&p.dir.join("media"), &cur_proxy);
    let (masks_current, masks_stale) = split(&p.dir.join("cache/masks"), &cur_mask);
    DiskUsage {
        proxy_current, proxy_stale, masks_current, masks_stale,
        previews: dir_size(&p.dir.join("cache/preview")),
        assets: dir_size(&p.dir.join("assets")),
        exports: dir_size(&p.dir.join("exports")),
    }
}

#[tauri::command]
pub fn disk_usage(state: State<'_, AppState>) -> Result<DiskUsage> {
    with_project(&state, |p| Ok(disk_usage_of(p)))
}

/// Supprime les caches régénérables qui ne servent plus (anciens proxies, masques, ombres et
/// normales d'un ancien masque, aperçus).
/// Les médias sources, décors importés et exports ne sont jamais touchés.
#[tauri::command]
pub fn clean_cache(state: State<'_, AppState>) -> Result<DiskUsage> {
    if state.busy.lock().unwrap().is_some() {
        return err("Une tâche est en cours : nettoyez le cache après sa fin.");
    }
    with_project(&state, |p| {
        let keep: Vec<PathBuf> = [p.manifest.proxy.as_ref().map(|x| p.dir.join(&x.dir)),
                                  p.manifest.segmentation.mask.as_ref().map(|x| p.dir.join(&x.dir)),
                                  shadow_rel(p).map(|r| p.dir.join(r)),
                                  normals_rel(p).map(|r| p.dir.join(r))]
            .into_iter().flatten().collect();
        for sub in ["media", "cache/masks", "cache/preview", "cache/shadows", "cache/normals"] {
            for e in fs::read_dir(p.dir.join(sub)).into_iter().flatten().flatten() {
                if keep.contains(&e.path()) {
                    continue;
                }
                let _ = if e.path().is_dir() { fs::remove_dir_all(e.path()) } else { fs::remove_file(e.path()) };
            }
        }
        Ok(disk_usage_of(p))
    })
}

// --- Planification par LLM -------------------------------------------------------------

#[derive(Serialize)]
pub struct PlannerStatus {
    pub llm: llm::LlmStatus,
    pub model: String,
}

#[tauri::command]
pub async fn planner_status(state: State<'_, AppState>) -> Result<PlannerStatus> {
    let (url, model) = state.planner.lock().unwrap().clone();
    let st = blocking(move || Ok(llm::status(&url))).await?;
    // Repli automatique si le modèle par défaut n'est pas installé.
    let model = if st.models.contains(&model) || st.models.is_empty() { model }
                else if st.models.iter().any(|m| m == llm::FALLBACK_MODEL) { llm::FALLBACK_MODEL.to_string() }
                else { st.models[0].clone() };
    state.planner.lock().unwrap().1 = model.clone();
    Ok(PlannerStatus { llm: st, model })
}

/// Libère la mémoire du LLM avant d'exécuter un plan (SAM 2 ou FLUX.2 vont être chargés).
#[tauri::command]
pub async fn release_planner(state: State<'_, AppState>) -> Result<()> {
    let (url, model) = state.planner.lock().unwrap().clone();
    blocking(move || { llm::release(&url, &model); Ok(()) }).await
}

#[tauri::command]
pub fn set_planner_model(state: State<'_, AppState>, model: String) -> Result<()> {
    if !llm::ALLOWED_MODELS.contains(&model.as_str()) {
        return err("Modèle non proposé pour la planification.");
    }
    state.planner.lock().unwrap().1 = model;
    Ok(())
}

/// État du projet transmis au LLM : uniquement des métadonnées (aucune image, aucun chemin absolu).
fn planner_context(p: &Project) -> (serde_json::Value, plan::Context) {
    let m = &p.manifest;
    let pr = &m.source.probe;
    let fps = pr.fps_num as f64 / pr.fps_den.max(1) as f64;
    let bgs = backgrounds::list(p);
    let state = json!({
        "video": {"duree_totale_s": (pr.duration * 100.0).round() / 100.0, "images_par_seconde": (fps * 100.0).round() / 100.0,
                  "format": format!("{}x{}", pr.display_width, pr.display_height),
                  "orientation": if pr.display_height > pr.display_width { "verticale" } else { "horizontale" },
                  "audio": pr.has_audio},
        "extrait": {"debut_s": m.clip.start, "duree_s": m.clip.duration, "pret": m.proxy.is_some()},
        "sujet_selectionne": !m.segmentation.prompts.is_empty(),
        "masque": match m.segmentation.status { MaskStatus::Ready => "prêt", MaskStatus::Stale => "à recalculer", MaskStatus::None => "absent" },
        "decor_actuel": m.composite.background.as_deref().map(|b| b.rsplit('/').next().unwrap_or("").split('.').next().unwrap_or("").to_string()),
        "decors_disponibles": bgs.iter().take(12).map(|b| json!({"id": b.id, "source": b.source,
            "description": b.meta["prompt"].as_str().map(|s| s.chars().take(120).collect::<String>())})).collect::<Vec<_>>(),
        "reglages": {"echelle": m.composite.subject_scale, "luminosite": m.composite.brightness, "temperature": m.composite.temperature,
                     "flou_decor": m.composite.bg_blur},
    });
    let ctx = plan::Context::from_project(p, bgs.iter().map(|b| b.id.clone()).collect());
    (state, ctx)
}

#[tauri::command]
pub async fn propose_plan(state: State<'_, AppState>, instruction: String, mode: Mode) -> Result<llm::PlannerResult> {
    let _busy = BusyGuard::acquire(&state.busy, "planification")?;
    let (url, model) = state.planner.lock().unwrap().clone();
    let (proj_state, ctx) = with_project(&state, |p| Ok(planner_context(p)))?;
    blocking(move || llm::propose(&url, &model, &instruction, &proj_state, &ctx, mode)).await
}

/// Recalcule prérequis et coûts après modification du plan par l'utilisateur (étapes activées,
/// paramètres édités, mode changé). Rien n'est exécuté ici.
#[tauri::command]
pub fn revalidate_plan(state: State<'_, AppState>, mut plan: Plan, mode_changed: bool) -> Result<Plan> {
    if mode_changed {
        // Le mode fixe alors le nombre de variantes et le préréglage d'export.
        for s in plan.steps.iter_mut() {
            if let Some(o) = s.params.as_object_mut() {
                if s.op == "generate_background" { o.remove("variants"); }
                if s.op == "export" { o.remove("quality"); }
            }
        }
    }
    with_project(&state, |p| {
        let (_, ctx) = planner_context(p);
        Ok(plan::revalidate(plan, &ctx))
    })
}

// --- Intégration : ombre de contact et éléments de premier plan --------------------------------

/// Calcule les ombres de contact du masque courant (une image par image ; vide si les pieds ne
/// sont pas visibles). Résultat mis en cache avec la clé du masque.
#[tauri::command]
pub async fn compute_shadows(app: AppHandle, state: State<'_, AppState>) -> Result<ProjectView> {
    let _busy = BusyGuard::acquire(&state.busy, "ombre de contact")?;
    let (masks, out) = with_project(&state, |p| {
        let seg = &p.manifest.segmentation;
        let mask = seg.mask.as_ref().filter(|_| seg.status == MaskStatus::Ready)
            .ok_or_else(|| AppError::Msg("Calculez d'abord le masque du sujet.".into()))?;
        Ok((p.resolve(&mask.dir)?, p.resolve(&shadow_rel(p).unwrap())?))
    })?;
    if !out.exists() {
        let w = worker_for(&app).await?;
        let tmp = out.with_extension("part");
        let (m2, t2) = (masks.clone(), tmp.clone());
        blocking(move || w.request("contact_shadows", json!({"masks_dir": m2, "out_dir": t2}))).await?;
        std::fs::rename(&tmp, &out)?;
    }
    with_project(&state, |p| Ok(view(p)))
}

/// Calcule les normales approchées du sujet (relighting directionnel), une image par image, à partir
/// du masque courant. Résultat mis en cache avec la clé du masque.
#[tauri::command]
pub async fn compute_normals(app: AppHandle, state: State<'_, AppState>) -> Result<ProjectView> {
    let _busy = BusyGuard::acquire(&state.busy, "relighting")?;
    let (masks, out) = with_project(&state, |p| {
        let seg = &p.manifest.segmentation;
        let mask = seg.mask.as_ref().filter(|_| seg.status == MaskStatus::Ready)
            .ok_or_else(|| AppError::Msg("Calculez d'abord le masque du sujet.".into()))?;
        Ok((p.resolve(&mask.dir)?, p.resolve(&normals_rel(p).unwrap())?))
    })?;
    if !out.exists() {
        let w = worker_for(&app).await?;
        let tmp = out.with_extension("part");
        let (m2, t2) = (masks.clone(), tmp.clone());
        blocking(move || w.request("normal_maps", json!({"masks_dir": m2, "out_dir": t2}))).await?;
        std::fs::rename(&tmp, &out)?;
    }
    with_project(&state, |p| Ok(view(p)))
}

#[derive(Serialize)]
pub struct ElementItem {
    pub path: String,
    pub abs_path: String,
    pub meta: serde_json::Value,
    pub width: u32,
    pub height: u32,
}

const ELEMENTS: &str = "assets/elements";

fn list_elements_of(p: &Project, ffprobe: &Path) -> Vec<ElementItem> {
    let mut items: Vec<ElementItem> = fs::read_dir(p.dir.join(ELEMENTS)).into_iter().flatten().flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "png") && !e.path().to_string_lossy().contains(".tmp."))
        .map(|e| {
            let path = e.path();
            let (w, h) = media::probe_image(ffprobe, &path).unwrap_or((0, 0));
            ElementItem {
                path: format!("{ELEMENTS}/{}", path.file_name().unwrap().to_string_lossy()),
                abs_path: path.display().to_string(),
                meta: fs::read_to_string(path.with_extension("json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(json!({})),
                width: w, height: h,
            }
        }).collect();
    items.sort_by(|a, b| b.path.cmp(&a.path));
    items
}

#[tauri::command]
pub fn list_elements(state: State<'_, AppState>) -> Result<Vec<ElementItem>> {
    let ffprobe = state.tools.ffprobe.clone();
    with_project(&state, |p| Ok(list_elements_of(p, &ffprobe)))
}

/// Détoure l'objet central d'une image (SAM 2 : rectangle presque plein cadre + point central),
/// puis écrit un PNG RGBA recadré sur l'objet.
async fn cutout_element(app: &AppHandle, state: &AppState, src: &Path, dest: &Path) -> Result<()> {
    let work = dest.with_extension("cut");
    let frames = work.join("frames");
    fs::create_dir_all(&frames)?;
    let ffmpeg = state.tools.ffmpeg.clone();
    let (ffm, s, j) = (ffmpeg.clone(), src.to_path_buf(), frames.join("00000.jpg"));
    blocking(move || {
        let o = std::process::Command::new(&ffm).args(["-v", "error", "-y", "-i"]).arg(&s).args(["-q:v", "2"]).arg(&j).output()?;
        if o.status.success() { Ok(()) } else { err("Image d'élément illisible.") }
    }).await?;
    let (iw, ih) = media::probe_image(&state.tools.ffprobe, &frames.join("00000.jpg"))?;
    let w = worker_for(app).await?;
    let mask = work.join("mask.png");
    let (f2, m2) = (frames.clone(), mask.clone());
    let (bx, by) = (iw as f64 * 0.03, ih as f64 * 0.03);
    let r = blocking(move || w.request("preview", json!({"frames_dir": f2, "frame": 0, "out_png": m2,
        "prompts": [{"frame": 0, "box": [bx, by, iw as f64 - bx, ih as f64 - by],
                     "points": [{"x": iw as f64 / 2.0, "y": ih as f64 / 2.0, "positive": true}]}]}))).await?;
    let bbox = r["bbox"].as_array().map(|a| a.iter().map(|v| v.as_u64().unwrap_or(0) as u32).collect::<Vec<_>>())
        .filter(|b| b.len() == 4).ok_or_else(|| AppError::Msg("Aucun objet détecté dans l'image.".into()))?;
    let (x0, y0, x1, y1) = (bbox[0], bbox[1], bbox[2], bbox[3]);
    let (s2, m3, d2) = (src.to_path_buf(), mask.clone(), dest.to_path_buf());
    let res = blocking(move || {
        let tmp = d2.with_extension("tmp.png");
        let o = std::process::Command::new(&ffmpeg).args(["-v", "error", "-y", "-i"]).arg(&s2).arg("-i").arg(&m3)
            .args(["-filter_complex", &format!("[0:v]scale={iw}:{ih},format=rgba[i];[1:v]format=gray,gblur=sigma=0.8[m];[i][m]alphamerge,crop={}:{}:{x0}:{y0}", x1 - x0, y1 - y0),
                   "-frames:v", "1"]).arg(&tmp).output()?;
        if !o.status.success() {
            return err(format!("Détourage de l'élément impossible : {}", String::from_utf8_lossy(&o.stderr).trim()));
        }
        fs::rename(&tmp, &d2)?;
        Ok(())
    }).await;
    let _ = fs::remove_dir_all(&work);
    res
}

/// Génère un élément (FLUX.2, objet isolé sur fond uni) puis le détoure automatiquement.
#[tauri::command]
pub async fn create_element(app: AppHandle, state: State<'_, AppState>, prompt: String, seed: Option<u32>) -> Result<Vec<ElementItem>> {
    let _busy = BusyGuard::acquire(&state.busy, "création d'élément")?;
    let prompt = prompt.trim().to_string();
    if prompt.is_empty() || prompt.chars().count() > 600 {
        return err("Décrivez l'élément (600 caractères au maximum).");
    }
    let seed_v = seed.unwrap_or_else(|| chrono::Local::now().timestamp_subsec_nanos() & 0x00FF_FFFF);
    let (root, raw, dest) = with_project(&state, |p| {
        fs::create_dir_all(p.dir.join(ELEMENTS))?;
        let id = format!("el_{}_{seed_v}", chrono::Local::now().format("%Y%m%d-%H%M%S"));
        Ok((p.dir.clone(), p.resolve(&format!("cache/elements/{id}_raw.png"))?, p.resolve(&format!("{ELEMENTS}/{id}.png"))?))
    })?;
    fs::create_dir_all(raw.parent().unwrap())?;
    let full = format!("{prompt}. Un seul objet entier, centré, vue de face à hauteur d'yeux, isolé sur un fond de studio uni gris clair, éclairage doux, photographie réaliste, aucune personne.");
    let app2 = app.clone();
    let w_img = blocking(move || image_worker(&app2.state::<AppState>(), &app2, &root)).await?;
    let (fp, rp) = (full.clone(), raw.clone());
    let gen = blocking(move || w_img.request("generate", json!({"prompt": fp, "width": 1024, "height": 1024, "seed": seed_v, "steps": 4, "out_png": rp}))).await?;
    stop_image_worker(&state);
    cutout_element(&app, &state, &raw, &dest).await?;
    let _ = fs::remove_file(&raw);
    let _ = fs::remove_file(raw.with_extension("json"));
    backgrounds::write_meta(&dest, &json!({"source": "generated", "prompt": prompt, "seed": seed_v, "model": gen["model"],
        "license": gen["license"], "seconds": gen["seconds"], "cutout": "SAM 2.1 tiny", "created_at": project::now()}))?;
    let ffprobe = state.tools.ffprobe.clone();
    with_project(&state, |p| Ok(list_elements_of(p, &ffprobe)))
}

/// Importe une image d'élément. Un PNG qui a déjà une transparence est gardé tel quel ; sinon
/// l'objet central est détouré automatiquement.
#[tauri::command]
pub async fn import_element(app: AppHandle, state: State<'_, AppState>, path: String) -> Result<Vec<ElementItem>> {
    let _busy = BusyGuard::acquire(&state.busy, "import d'élément")?;
    let src = PathBuf::from(&path);
    let ext = src.extension().and_then(|e| e.to_str()).map(|e| e.to_lowercase()).unwrap_or_default();
    if !backgrounds::IMAGE_EXT.contains(&ext.as_str()) {
        return err("Format d'élément non pris en charge (PNG, JPEG ou WebP).");
    }
    let dest = with_project(&state, |p| {
        fs::create_dir_all(p.dir.join(ELEMENTS))?;
        p.resolve(&format!("{ELEMENTS}/imp_{}.png", &project::fingerprint(&src)?[..16]))
    })?;
    let pix = std::process::Command::new(&state.tools.ffprobe).args(["-v", "error", "-select_streams", "v:0", "-show_entries",
        "stream=pix_fmt", "-of", "csv=p=0"]).arg(&src).output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
    let has_alpha = pix.contains("rgba") || pix.contains("ya") || pix.starts_with("pal8") || pix.contains("argb");
    if has_alpha {
        let o = std::process::Command::new(&state.tools.ffmpeg).args(["-v", "error", "-y", "-i"]).arg(&src)
            .args(["-frames:v", "1", "-pix_fmt", "rgba"]).arg(&dest).output()?;
        if !o.status.success() {
            return err("Image d'élément illisible.");
        }
    } else {
        cutout_element(&app, &state, &src, &dest).await?;
    }
    backgrounds::write_meta(&dest, &json!({"source": "imported", "original_name": src.file_name().map(|n| n.to_string_lossy().to_string()),
        "cutout": if has_alpha { "transparence d'origine" } else { "SAM 2.1 tiny" }, "created_at": project::now()}))?;
    let ffprobe = state.tools.ffprobe.clone();
    with_project(&state, |p| Ok(list_elements_of(p, &ffprobe)))
}

// --- Décor animé (étape 5, expérimental) ----------------------------------------------------

#[derive(Serialize)]
pub struct VideoEngines {
    pub engines: Vec<video::EngineStatus>,
    /// Durée à couvrir (extrait) et taille de génération pour ce projet.
    pub clip_seconds: f64,
    pub gen_width: u32,
    pub gen_height: u32,
}

#[tauri::command]
pub async fn video_engines(state: State<'_, AppState>) -> Result<VideoEngines> {
    let dir = state.tools.video_engines.clone();
    let (secs, (ow, oh)) = with_project(&state, |p| {
        let pr = &p.manifest.source.probe;
        Ok((p.manifest.clip.duration, media::output_size(pr.display_width, pr.display_height)))
    })?;
    blocking(move || {
        let engines = video::load(&dir)?;
        let (gw, gh) = video::gen_size(ow, oh, 32);
        Ok(VideoEngines { engines: video::status(&engines), clip_seconds: secs, gen_width: gw, gen_height: gh })
    }).await
}

/// Anime le décor fixe courant : plan court généré par le moteur, puis boucle aller-retour.
/// Tous les autres modèles sont déchargés avant (pic mesuré jusqu'à ~53 Go pour Sulphur-2).
#[tauri::command]
pub async fn animate_background(app: AppHandle, state: State<'_, AppState>, engine: String, prompt: String, seconds: f64, seed: Option<u32>,
                                zones: Option<Vec<[f64; 2]>>) -> Result<BackgroundItem> {
    let zones = zones.unwrap_or_default();
    if zones.len() > 8 || zones.iter().any(|[x, y]| !(0.0..=1.0).contains(x) || !(0.0..=1.0).contains(y)) {
        return err("Zones invalides (8 points au maximum, coordonnées entre 0 et 1).");
    }
    let _busy = BusyGuard::acquire(&state.busy, "animation du décor")?;
    let prompt = prompt.trim().to_string();
    if prompt.is_empty() || prompt.chars().count() > 1000 {
        return err("Décrivez le mouvement voulu (1 000 caractères au maximum).");
    }
    if !(1.0..=10.0).contains(&seconds) {
        return err("Durée du plan animé : entre 1 et 10 secondes.");
    }
    let engines = video::load(&state.tools.video_engines)?;
    let eng = engines.into_iter().find(|e| e.id == engine).ok_or_else(|| AppError::Msg(format!("Moteur inconnu : {engine}")))?;
    let (still, still_rel, (ow, oh), out) = with_project(&state, |p| {
        let rel = p.manifest.composite.background.clone().ok_or_else(|| AppError::Msg("Choisissez d'abord un décor fixe à animer.".into()))?;
        if backgrounds::is_video(&rel) {
            return err("Ce décor est déjà animé : choisissez son image d'origine.");
        }
        let pr = &p.manifest.source.probe;
        let seed_v = seed.unwrap_or_else(|| chrono::Local::now().timestamp_subsec_nanos() & 0x00FF_FFFF);
        Ok((p.resolve(&rel)?, rel, media::output_size(pr.display_width, pr.display_height), backgrounds::new_animated(p, &eng.id, seed_v).map(|o| (o, seed_v))?))
    })?;
    let ((rel, dest, raw), seed_v) = out;
    // Un seul modèle lourd : on libère SAM 2, FLUX.2 et le LLM.
    replace_worker(&state);
    let (url, model) = state.planner.lock().unwrap().clone();
    blocking(move || { llm::release(&url, &model); Ok(()) }).await?;

    let (gw, gh) = video::gen_size(ow, oh, eng.align);
    let frames = video::frame_count(&eng.frames_rule, seconds, eng.fps, eng.max_frames);
    // L'image source est mise au format exact de génération (recadrage « cover »).
    let still_fit = raw.with_file_name(format!("{}_still.png", raw.file_stem().unwrap().to_string_lossy()));
    let ffmpeg = state.tools.ffmpeg.clone();
    {
        let (ffm, s_in, s_out) = (ffmpeg.clone(), still.clone(), still_fit.clone());
        blocking(move || {
            let o = std::process::Command::new(&ffm).args(["-v", "error", "-y", "-i"]).arg(&s_in)
                .args(["-vf", &format!("scale={gw}:{gh}:force_original_aspect_ratio=increase,crop={gw}:{gh}")]).arg(&s_out).output()?;
            if o.status.success() { Ok(()) } else { err(format!("Préparation de l'image impossible : {}", String::from_utf8_lossy(&o.stderr))) }
        }).await?;
    }
    // Cinémagraphe : chaque point désigne une zone (eau, ciel…) détourée par SAM 2 sur le décor fixe.
    // On le fait avant la génération, puis on décharge SAM 2 pour laisser la mémoire au moteur vidéo.
    let mut zone_masks: Vec<PathBuf> = Vec::new();
    if !zones.is_empty() {
        let zone_dir = raw.with_file_name(format!("{}_zones", raw.file_stem().unwrap().to_string_lossy()));
        let frames_dir = zone_dir.join("frames");
        std::fs::create_dir_all(&frames_dir)?;
        {
            let (ffm, s_in, jpg) = (ffmpeg.clone(), still_fit.clone(), frames_dir.join("00000.jpg"));
            blocking(move || {
                let o = std::process::Command::new(&ffm).args(["-v", "error", "-y", "-i"]).arg(&s_in).args(["-q:v", "2"]).arg(&jpg).output()?;
                if o.status.success() { Ok(()) } else { err("Préparation des zones impossible.") }
            }).await?;
        }
        let w = worker_for(&app).await?;
        for (i, [x, y]) in zones.iter().enumerate() {
            let out_png = zone_dir.join(format!("zone_{i}.png"));
            let req = json!({"frames_dir": frames_dir, "frame": 0, "out_png": out_png,
                             "prompts": [{"frame": 0, "points": [{"x": x * gw as f64, "y": y * gh as f64, "positive": true}]}]});
            let w2 = w.clone();
            blocking(move || w2.request("preview", req)).await?;
            zone_masks.push(out_png);
        }
        stop_vision_worker(&state);
    }
    let job = video::Job { image: still_fit.clone(), prompt: prompt.clone(), frames, width: gw, height: gh, seed: seed_v, out: raw.clone() };
    let slot = state.video_child.clone();
    let started = std::time::Instant::now();
    let (eng2, ffm2, app2) = (eng.clone(), ffmpeg.clone(), app.clone());
    let result = blocking(move || video::run(&eng2, &ffm2, &job, slot, |f| { let _ = app2.emit("video-progress", json!({"fraction": f})); })).await;
    let cleanup = |still_fit: &Path, raw: &Path, masks: &[PathBuf]| {
        let _ = std::fs::remove_file(still_fit);
        let _ = std::fs::remove_file(raw);
        if let Some(d) = masks.first().and_then(|m| m.parent()) {
            let _ = std::fs::remove_dir_all(d);
        }
    };
    if let Err(e) = result {
        cleanup(&still_fit, &raw, &zone_masks);
        return Err(e);
    }
    let gen_seconds = started.elapsed().as_secs_f64();
    let (ffm3, raw2, dest2, still2, masks2) = (ffmpeg.clone(), raw.clone(), dest.clone(), still_fit.clone(), zone_masks.clone());
    let fps_e = eng.fps;
    let looped = blocking(move || video::make_loop(&ffm3, &raw2, &dest2, &video::LoopSpec {
        width: gw, height: gh, fps: fps_e, still: Some(&still2), zones: &masks2 })).await;
    cleanup(&still_fit, &raw, &zone_masks);
    looped?;
    let meta = json!({
        "source": "animated", "engine": eng.id, "engine_version": eng.version, "engine_title": eng.title,
        "engine_tested": eng.tested, "license": eng.license, "motion_prompt": prompt, "seed": seed_v,
        "frames": frames, "width": gw, "height": gh, "fps": eng.fps, "loop": "aller-retour",
        "zones": zones, "mode": if zones.is_empty() { "plan entier" } else { "cinémagraphe (zones animées, reste fixe)" },
        "seconds": (gen_seconds * 10.0).round() / 10.0, "from_still": still_rel, "created_at": project::now(),
    });
    backgrounds::write_meta(&dest, &meta)?;
    with_project(&state, |p| {
        backgrounds::list(p).into_iter().find(|b| b.path == rel).ok_or_else(|| AppError::Msg("Décor animé introuvable après création.".into()))
    })
}

#[tauri::command]
pub fn project_path(state: State<'_, AppState>, rel: String) -> Result<String> {
    with_project(&state, |p| Ok(p.resolve(&rel)?.display().to_string()))
}
