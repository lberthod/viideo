//! Décor animé (expérimental) : un décor fixe devient un plan court animé (eau, nuages, végétation),
//! caméra fixe, puis une boucle aller-retour qui couvre toute la durée de l'extrait.
//!
//! Les moteurs sont décrits dans `workflows/video/engines.json` (registre versionné) et lancés sans
//! shell, par liste d'arguments. Un moteur dont un chemin contient « uncensored » est refusé
//! (consigne du guide ~/ai-video/creation_video.md).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::error::{err, AppError, Result};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cost {
    pub load_s: f64,
    pub s_per_frame: f64,
    pub peak_gb: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Engine {
    pub id: String,
    pub version: u32,
    pub title: String,
    pub kind: String,
    pub license: String,
    pub tested: bool,
    #[serde(default)]
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub requires: Vec<String>,
    pub frames_rule: String,
    pub max_frames: u32,
    pub fps: u32,
    #[serde(default = "default_align")]
    pub align: u32,
    pub cost: Cost,
    #[serde(default)]
    pub notes: String,
}

fn default_align() -> u32 {
    16
}

#[derive(Deserialize)]
struct Registry {
    engines: Vec<Engine>,
}

#[derive(Debug, Serialize)]
pub struct EngineStatus {
    #[serde(flatten)]
    pub engine: Engine,
    pub available: bool,
    pub problems: Vec<String>,
}

pub fn ai_video_dir() -> PathBuf {
    std::env::var_os("VIDEEO_AI_VIDEO").map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join("ai-video"))
}

fn expand(s: &str, vars: &[(&str, String)]) -> String {
    let mut out = s.to_string();
    for (k, v) in vars {
        out = out.replace(&format!("{{{k}}}"), v);
    }
    out
}

pub fn load(dir: &Path) -> Result<Vec<Engine>> {
    let text = std::fs::read_to_string(dir.join("engines.json"))
        .map_err(|_| AppError::Msg(format!("Registre des moteurs vidéo introuvable : {}", dir.display())))?;
    let reg: Registry = serde_json::from_str(&text).map_err(|e| AppError::Msg(format!("Registre des moteurs vidéo illisible : {e}")))?;
    Ok(reg.engines)
}

pub fn status(engines: &[Engine]) -> Vec<EngineStatus> {
    let ai = ai_video_dir().display().to_string();
    engines.iter().map(|e| {
        let mut problems = Vec::new();
        let vars = [("ai_video", ai.clone())];
        if e.kind == "command" {
            let program = expand(&e.program, &vars);
            if !Path::new(&program).exists() {
                problems.push(format!("programme absent : {program}"));
            }
            for r in &e.requires {
                let p = expand(r, &vars);
                if !Path::new(&p).exists() {
                    problems.push(format!("modèle absent : {p}"));
                }
            }
            if std::iter::once(&e.program).chain(&e.args).chain(&e.requires).any(|a| a.to_lowercase().contains("uncensored")) {
                problems.push("variante « uncensored » refusée".into());
            }
        }
        EngineStatus { available: problems.is_empty(), problems, engine: e.clone() }
    }).collect()
}

/// Nombre d'images respectant la grille du moteur (8k+1 pour LTX, 4k+1 pour Wan), sans dépasser le maximum.
pub fn frame_count(rule: &str, seconds: f64, fps: u32, max: u32) -> u32 {
    let want = ((seconds * fps as f64).round() as u32).clamp(9, max.max(9));
    let step = match rule { "8k+1" => 8, "4k+1" => 4, _ => return want };
    // Grille la plus proche, sans dépasser `max` (lui-même sur la grille).
    let k = ((want - 1) as f64 / step as f64).round() as u32;
    let n = k * step + 1;
    if n <= max { n } else { ((max - 1) / step) * step + 1 }
}

/// Taille de génération : grand côté 1024, alignée sur `align`, au format de la sortie.
pub fn gen_size(out_w: u32, out_h: u32, align: u32) -> (u32, u32) {
    let (lw, lh) = if out_w >= out_h { (1024.0, 1024.0 * out_h as f64 / out_w as f64) } else { (1024.0 * out_w as f64 / out_h as f64, 1024.0) };
    let a = align.max(1) as f64;
    (((lw / a).round() * a) as u32, ((lh / a).round() * a) as u32)
}

/// Fin de chaîne d'au plus `n` octets, coupée sur une frontière de caractère (les barres tqdm
/// contiennent des caractères multi-octets comme « █ »).
fn tail_str(s: &str, n: usize) -> &str {
    let mut start = s.len().saturating_sub(n);
    while !s.is_char_boundary(start) {
        start += 1;
    }
    &s[start..]
}

/// Dernier motif « n/N » d'une sortie de barre de progression.
pub fn last_progress(text: &str) -> Option<(f64, f64)> {
    let tail = tail_str(text, 200);
    // Le dernier « / » entouré de chiffres (pas celui de « s/it »).
    tail.match_indices('/').rev().find_map(|(idx, _)| {
        let before: String = tail[..idx].chars().rev().take_while(|c| c.is_ascii_digit()).collect::<String>().chars().rev().collect();
        let after: String = tail[idx + 1..].chars().take_while(|c| c.is_ascii_digit()).collect();
        Some((before.parse().ok()?, after.parse().ok()?))
    })
}

/// Tue le processus du moteur si la tâche s'arrête avant sa fin normale (erreur, panique) :
/// un moteur vidéo orphelin garderait jusqu'à ~53 Go de mémoire.
struct KillOnDrop(Arc<Mutex<Option<Child>>>);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        if let Ok(mut slot) = self.0.lock() {
            if let Some(mut c) = slot.take() {
                let _ = c.kill();
                let _ = c.wait();
            }
        }
    }
}

pub struct Job {
    pub image: PathBuf,
    pub prompt: String,
    pub frames: u32,
    pub width: u32,
    pub height: u32,
    pub seed: u32,
    pub out: PathBuf,
}

/// Lance le moteur. Progression lue dans la sortie (motifs « n/N » des barres de progression).
pub fn run(engine: &Engine, ffmpeg: &Path, job: &Job, slot: Arc<Mutex<Option<Child>>>, mut on_progress: impl FnMut(f64)) -> Result<()> {
    let st = status(std::slice::from_ref(engine));
    if !st[0].available {
        return err(format!("Moteur {} indisponible : {}", engine.title, st[0].problems.join(" ; ")));
    }
    let mut cmd = if engine.kind == "builtin" {
        // Ken Burns : zoom de 5 % sur toute la durée, centré.
        let n = job.frames;
        let mut c = Command::new(ffmpeg);
        c.args(["-v", "error", "-y", "-loop", "1", "-i"]).arg(&job.image)
            .args(["-vf", &format!("scale={}:{},zoompan=z='1+0.05*on/{n}':x='iw/2-(iw/zoom/2)':y='ih/2-(ih/zoom/2)':d={n}:s={}x{}:fps={}",
                                   job.width * 4, job.height * 4, job.width, job.height, engine.fps),
                   "-frames:v", &n.to_string(), "-c:v", "libx264", "-pix_fmt", "yuv420p", "-crf", "16"])
            .arg(&job.out);
        c
    } else {
        let vars = [
            ("ai_video", ai_video_dir().display().to_string()),
            ("image", job.image.display().to_string()),
            ("prompt", job.prompt.clone()),
            ("frames", job.frames.to_string()),
            ("width", job.width.to_string()),
            ("height", job.height.to_string()),
            ("seed", job.seed.to_string()),
            ("out", job.out.display().to_string()),
        ];
        let mut c = Command::new(expand(&engine.program, &vars));
        // Chaque argument est passé tel quel : le prompt ne peut pas injecter d'option ni de commande.
        c.args(engine.args.iter().map(|a| expand(a, &vars)));
        c
    };
    let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()
        .map_err(|e| AppError::Msg(format!("Impossible de lancer {} : {e}", engine.title)))?;
    let mut stderr = child.stderr.take().expect("stderr");
    let mut stdout = child.stdout.take().expect("stdout");
    *slot.lock().unwrap() = Some(child);
    let guard = KillOnDrop(slot.clone());
    let out_reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stdout.read_to_string(&mut s);
        s
    });
    // Les barres tqdm réécrivent la ligne avec \r : on lit par blocs et on cherche le dernier « n/N ».
    let mut log = String::new();
    let mut buf = [0u8; 4096];
    let mut stage = 0usize;
    let mut last_total = 0.0;
    loop {
        match stderr.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                log.push_str(&String::from_utf8_lossy(&buf[..n]));
                if log.len() > 200_000 {
                    log = tail_str(&log, 100_000).to_string();
                }
                if let Some((d, t)) = last_progress(&log) {
                    if t > 0.0 && d <= t {
                        if t != last_total && d <= 1.0 {
                            stage += 1;
                            last_total = t;
                        }
                        // Deux phases de débruitage pour LTX distillé : on répartit 0–0,5 puis 0,5–0,95.
                        let base = if stage <= 1 { 0.0 } else { 0.5 };
                        on_progress((base + 0.45 * d / t).min(0.95));
                    }
                }
            }
        }
    }
    let status = slot.lock().unwrap().take().map(|mut c| c.wait()).transpose()?;
    drop(guard);
    let stdout_text = out_reader.join().unwrap_or_default();
    match status {
        Some(s) if s.success() && job.out.exists() => { on_progress(1.0); Ok(()) }
        Some(s) if s.success() => err(format!("{} n'a pas produit de vidéo.", engine.title)),
        _ => {
            let tail: String = log.lines().chain(stdout_text.lines()).filter(|l| !l.trim().is_empty()).rev().take(4)
                .collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join(" | ");
            if tail.is_empty() { err("Animation annulée.") } else { err(format!("{} a échoué : {tail}", engine.title)) }
        }
    }
}

/// Graphe de la boucle. Sans zones : plan généré en aller-retour. Avec zones (cinémagraphe) :
/// le décor fixe partout, le plan généré seulement dans l'union des masques de zones, adoucie.
/// Entrées : 0 = plan brut ; 1 = décor fixe ; 2.. = masques de zones.
pub fn loop_graph(w: u32, h: u32, fps: u32, zones: usize, feather: f64) -> String {
    let pingpong = format!("[0:v]fps={fps},scale={w}:{h}:flags=lanczos,setsar=1,split[a][b];[b]reverse,trim=start_frame=1,setpts=PTS-STARTPTS[r];[a][r]concat=n=2:v=1:a=0");
    if zones == 0 {
        return format!("{pingpong}[v]");
    }
    let mut g = format!("{pingpong},format=gbrp[anim];[1:v]fps={fps},scale={w}:{h}:flags=lanczos,setsar=1,format=gbrp[still];");
    let mut last = "[2:v]".to_string();
    for i in 1..zones {
        let out = format!("[u{i}]");
        g.push_str(&format!("{last}[{}:v]blend=all_mode=lighten{out};", i + 2));
        last = out;
    }
    g.push_str(&format!("{last}scale={w}:{h},format=gray,gblur=sigma={feather:.1},format=gbrp[mask];[still][anim][mask]maskedmerge,format=yuv420p[v]"));
    g
}

/// Format de la boucle : taille, cadence et, pour un cinémagraphe, décor fixe et masques de zones
/// (PNG, blanc = zone animée).
pub struct LoopSpec<'a> {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub still: Option<&'a Path>,
    pub zones: &'a [PathBuf],
}

/// Boucle aller-retour (sans saut à la jointure), sans audio.
pub fn make_loop(ffmpeg: &Path, raw: &Path, dest: &Path, spec: &LoopSpec) -> Result<()> {
    let (w, h, fps, still, zones) = (spec.width, spec.height, spec.fps, spec.still, spec.zones);
    let tmp = dest.with_extension("part.mp4");
    let raw_frames = Command::new(ffmpeg.with_file_name("ffprobe")).args(["-v", "error", "-count_frames", "-select_streams", "v:0",
        "-show_entries", "stream=nb_read_frames", "-of", "csv=p=0"]).arg(raw).output()
        .ok().and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<u32>().ok()).unwrap_or(0);
    let mut cmd = Command::new(ffmpeg);
    cmd.args(["-v", "error", "-y", "-i"]).arg(raw);
    let use_zones = still.is_some() && !zones.is_empty();
    if use_zones {
        cmd.args(["-loop", "1", "-framerate", &fps.to_string(), "-i"]).arg(still.unwrap());
        for z in zones {
            cmd.args(["-loop", "1", "-framerate", &fps.to_string(), "-i"]).arg(z);
        }
    }
    cmd.args(["-filter_complex", &loop_graph(w, h, fps, if use_zones { zones.len() } else { 0 }, (w.max(h) as f64) / 120.0),
              "-map", "[v]", "-an"]);
    if raw_frames > 1 {
        cmd.args(["-frames:v", &(2 * raw_frames - 1).to_string()]);
    }
    let out = cmd.args(["-c:v", "libx264", "-pix_fmt", "yuv420p", "-crf", "16", "-movflags", "+faststart"]).arg(&tmp).output()?;
    if !out.status.success() {
        let _ = std::fs::remove_file(&tmp);
        return err(format!("Création de la boucle impossible : {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    std::fs::rename(tmp, dest)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_grid_and_sizes() {
        assert_eq!(frame_count("8k+1", 5.0, 24, 121), 121);
        assert_eq!(frame_count("8k+1", 3.0, 24, 121), 73);
        assert_eq!(frame_count("8k+1", 10.0, 24, 121), 121);
        assert_eq!(frame_count("4k+1", 5.0, 24, 49), 49);
        assert_eq!(frame_count("any", 5.0, 24, 2400), 120);
        assert_eq!(gen_size(1920, 1080, 32), (1024, 576));
        assert_eq!(gen_size(1080, 1920, 32), (576, 1024));
    }

    #[test]
    fn progress_parsing_is_utf8_safe() {
        // Sortie réelle de ltx-2-mlx : barres « █ » (3 octets) et réécritures par \r.
        let mut s = String::from("Denoising:  12%|█▎        | 1/8 [00:03<00:27,  3.95s/it]\r");
        for _ in 0..40 {
            s.push_str("Denoising:  88%|████████▊ | 7/8 [00:27<00:03,  3.95s/it]\r");
        }
        for cut in 0..s.len() {
            let _ = tail_str(&s, cut); // ne doit jamais paniquer
        }
        assert_eq!(last_progress(&s), Some((7.0, 8.0)));
        assert_eq!(last_progress("rien"), None);
    }

    #[test]
    fn loop_graph_with_zones() {
        let g = loop_graph(1024, 576, 24, 0, 8.0);
        assert!(g.ends_with("concat=n=2:v=1:a=0[v]") && !g.contains("maskedmerge"));
        let g = loop_graph(1024, 576, 24, 3, 8.5);
        assert!(g.contains("[2:v][3:v]blend=all_mode=lighten[u1]"));
        assert!(g.contains("[u1][4:v]blend=all_mode=lighten[u2]"));
        assert!(g.contains("[u2]scale=1024:576,format=gray,gblur=sigma=8.5"));
        assert!(g.contains("[still][anim][mask]maskedmerge"));
    }

    #[test]
    fn registry_loads_and_refuses_uncensored() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../workflows/video");
        let engines = load(&dir).unwrap();
        assert!(engines.iter().any(|e| e.id == "kenburns" && e.kind == "builtin"));
        let mut bad = engines.iter().find(|e| e.id == "sulphur2").unwrap().clone();
        bad.requires = vec!["{ai_video}/models/flux2-klein-4b-uncensored-q4".into()];
        let st = status(&[bad]);
        assert!(!st[0].available && st[0].problems.iter().any(|p| p.contains("uncensored")));
    }
}
