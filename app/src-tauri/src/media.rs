//! Opérations média via FFmpeg/FFprobe en ligne de commande : probe, proxy JPEG et export composite.
//!
//! Proxy et export appliquent la même cadence constante (`fps=N/D`) depuis le même point d'entrée :
//! l'image n du proxy, donc le masque n, correspond à l'image n de l'export, y compris pour une
//! source à cadence variable (cas fréquent sur iPhone).

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::error::{err, Result};
use crate::project::{Composite, ExportQuality, PROXY_HEIGHT};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Probe {
    pub duration: f64,
    /// Dimensions stockées dans le fichier.
    pub width: u32,
    pub height: u32,
    /// Dimensions d'affichage, rotation appliquée.
    pub display_width: u32,
    pub display_height: u32,
    pub rotation: i32,
    pub fps_num: u32,
    pub fps_den: u32,
    pub codec: String,
    pub has_audio: bool,
    pub size_bytes: u64,
}

fn parse_rate(s: &str) -> Option<(u32, u32)> {
    let (n, d) = s.split_once('/')?;
    let (n, d): (u32, u32) = (n.parse().ok()?, d.parse().ok()?);
    (n > 0 && d > 0 && (n as f64 / d as f64) <= 240.0).then_some((n, d))
}

pub fn probe(ffprobe: &Path, path: &Path) -> Result<Probe> {
    let out = Command::new(ffprobe)
        .args(["-v", "error", "-print_format", "json", "-show_format", "-show_streams"])
        .arg(path)
        .output()?;
    if !out.status.success() {
        return err(format!("FFprobe ne reconnaît pas ce fichier : {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout)?;
    let streams = v["streams"].as_array().cloned().unwrap_or_default();
    let Some(video) = streams.iter().find(|s| s["codec_type"] == "video" && s["disposition"]["attached_pic"] != 1) else {
        return err("Ce fichier ne contient pas de piste vidéo.");
    };
    let has_audio = streams.iter().any(|s| s["codec_type"] == "audio");
    let width = video["width"].as_u64().unwrap_or(0) as u32;
    let height = video["height"].as_u64().unwrap_or(0) as u32;
    let mut rotation = 0i32;
    if let Some(sd) = video["side_data_list"].as_array() {
        for d in sd {
            if let Some(r) = d["rotation"].as_i64() {
                rotation = r as i32;
            }
        }
    }
    if rotation == 0 {
        if let Some(r) = video["tags"]["rotate"].as_str().and_then(|r| r.parse::<i32>().ok()) {
            rotation = r;
        }
    }
    let rotated = rotation.rem_euclid(180) == 90;
    let (fps_num, fps_den) = video["avg_frame_rate"].as_str().and_then(parse_rate)
        .or_else(|| video["r_frame_rate"].as_str().and_then(parse_rate))
        .unwrap_or((30, 1));
    let duration = v["format"]["duration"].as_str().and_then(|d| d.parse::<f64>().ok())
        .or_else(|| video["duration"].as_str().and_then(|d| d.parse::<f64>().ok()))
        .unwrap_or(0.0);
    if width == 0 || height == 0 || duration <= 0.0 {
        return err("Dimensions ou durée de la vidéo illisibles.");
    }
    Ok(Probe {
        duration,
        width,
        height,
        display_width: if rotated { height } else { width },
        display_height: if rotated { width } else { height },
        rotation,
        fps_num,
        fps_den,
        codec: video["codec_name"].as_str().unwrap_or("?").to_string(),
        has_audio,
        size_bytes: v["format"]["size"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0),
    })
}

fn run(cmd: &mut Command) -> Result<()> {
    let out = cmd.output()?;
    if !out.status.success() {
        return err(format!("FFmpeg a échoué : {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(())
}

pub struct ProxyResult {
    pub width: u32,
    pub height: u32,
    pub frames: u32,
}

/// Extrait le proxy JPEG (hauteur 720) dans `dest`, via un dossier temporaire renommé à la fin.
pub fn extract_proxy(ffmpeg: &Path, ffprobe: &Path, src: &Path, start: f64, duration: f64, fps: (u32, u32), dest: &Path) -> Result<ProxyResult> {
    let tmp = dest.with_extension("part");
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp)?;
    run(Command::new(ffmpeg)
        .args(["-v", "error", "-y", "-ss", &format!("{start:.3}")])
        .arg("-i").arg(src)
        .args(["-t", &format!("{duration:.3}"), "-an",
               "-vf", &format!("fps={}/{},scale=-2:{PROXY_HEIGHT}:flags=bicubic", fps.0, fps.1),
               "-q:v", "3", "-start_number", "0"])
        .arg(tmp.join("%05d.jpg")))?;
    let frames = fs::read_dir(&tmp)?.filter(|e| e.as_ref().map(|e| e.path().extension().is_some_and(|x| x == "jpg")).unwrap_or(false)).count() as u32;
    if frames == 0 {
        let _ = fs::remove_dir_all(&tmp);
        return err("Aucune image extraite : vérifiez le début et la durée de l'extrait.");
    }
    let first = probe_image(ffprobe, &tmp.join("00000.jpg"))?;
    let _ = fs::remove_dir_all(dest);
    fs::rename(&tmp, dest)?;
    Ok(ProxyResult { width: first.0, height: first.1, frames })
}

pub(crate) fn probe_image(ffprobe: &Path, path: &Path) -> Result<(u32, u32)> {
    let out = Command::new(ffprobe)
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=width,height", "-of", "csv=p=0:s=x"])
        .arg(path).output()?;
    let s = String::from_utf8_lossy(&out.stdout);
    let (w, h) = s.trim().split_once('x').ok_or_else(|| crate::error::AppError::Msg("Image proxy illisible.".into()))?;
    Ok((w.parse().unwrap_or(0), h.parse().unwrap_or(0)))
}

pub struct ExportJob {
    pub src: PathBuf,
    pub start: f64,
    pub duration: f64,
    pub fps: (u32, u32),
    pub has_audio: bool,
    pub out_width: u32,
    pub out_height: u32,
    pub proxy_height: u32,
    pub masks_dir: PathBuf,
    pub background: Option<PathBuf>,
    pub composite: Composite,
    pub quality: ExportQuality,
    pub videotoolbox: bool,
    pub dest: PathBuf,
}

/// Encodeur par préréglage. « Rapide » utilise l'encodeur matériel VideoToolbox (3 à 3,5× plus
/// rapide que x264 en 4K, mesuré à l'étape 0) ; repli sur x264 veryfast s'il est absent.
pub fn encoder_args(q: &ExportQuality, videotoolbox: bool) -> Vec<&'static str> {
    match q {
        ExportQuality::Fast if videotoolbox => vec!["-c:v", "h264_videotoolbox", "-q:v", "65", "-allow_sw", "1"],
        ExportQuality::Fast => vec!["-c:v", "libx264", "-preset", "veryfast", "-crf", "20"],
        ExportQuality::Balanced => vec!["-c:v", "libx264", "-preset", "medium", "-crf", "18"],
        ExportQuality::High => vec!["-c:v", "libx264", "-preset", "slow", "-crf", "16"],
    }
}

/// Courbe de bord appliquée au masque adouci (mêmes formules dans compositor.ts) :
/// alpha' = clip((alpha − seuil) × 2 + 128), seuil = 128 + bord × 120.
pub fn edge_threshold(edge: f64) -> f64 {
    128.0 + edge * 120.0
}

/// Décalage rouge/bleu de la température (mêmes formules dans compositor.ts).
pub fn temperature_offset(t: f64) -> f64 {
    t * 60.0
}

fn even(v: f64) -> u32 {
    ((v / 2.0).round() as u32).max(1) * 2
}

/// Taille de sortie : celle de la source (rotation appliquée), plafonnée à 3840 px sur le grand côté.
pub fn output_size(display_w: u32, display_h: u32) -> (u32, u32) {
    let long = display_w.max(display_h) as f64;
    let k = if long > 3840.0 { 3840.0 / long } else { 1.0 };
    (even(display_w as f64 * k), even(display_h as f64 * k))
}

/// Graphe de filtres FFmpeg du composite. Entrées : 0 = source, 1 = masques PNG, 2 = décor.
pub fn filter_graph(job: &ExportJob) -> String {
    let (w, h) = (job.out_width, job.out_height);
    let c = &job.composite;
    let (fn_, fd) = job.fps;
    let feather = c.feather * h as f64 / job.proxy_height.max(1) as f64;
    let (sw, sh) = (even(w as f64 * c.subject_scale), even(h as f64 * c.subject_scale));
    let x = format!("(W-w)/2+{:.4}*W", c.subject_x);
    let y = format!("(H-h)/2+{:.4}*H", c.subject_y);
    let blur = if feather >= 0.5 { format!(",boxblur={feather:.2}:1") } else { String::new() };
    let edge = if c.edge.abs() > 1e-3 {
        format!(",lut=c0='clip((val-{:.2})*2+128\\,0\\,255)'", edge_threshold(c.edge))
    } else {
        String::new()
    };
    // Flou du décor exprimé pour une sortie 720p, mis à l'échelle de la sortie réelle.
    let bgblur = if c.bg_blur >= 0.5 { format!(",gblur=sigma={:.2}", c.bg_blur * h as f64 / 720.0) } else { String::new() };
    let temp = if c.temperature.abs() > 1e-3 {
        let t = temperature_offset(c.temperature);
        format!(",lutrgb=r='clip(val+{t:.2}\\,0\\,255)':b='clip(val-{t:.2}\\,0\\,255)'")
    } else {
        String::new()
    };
    format!(
        "[0:v]fps={fn_}/{fd},scale={w}:{h}:flags=bicubic,setsar=1,\
eq=brightness={b:.3}:contrast={ct:.3}:saturation={sa:.3},format=rgba{temp}[src];\
[1:v]scale={w}:{h}:flags=bilinear,format=gray{blur}{edge}[m];\
[src][m]alphamerge,scale={sw}:{sh}:flags=bicubic[fg];\
[2:v]fps={fn_}/{fd},scale={w}:{h}:force_original_aspect_ratio=increase,crop={w}:{h},setsar=1{bgblur},format=rgba[bg];\
[bg][fg]overlay=x={x}:y={y}:shortest=1:format=auto,format=yuv420p[out]",
        b = c.brightness, ct = c.contrast, sa = c.saturation,
    )
}

/// Lance l'export ; `on_progress` reçoit une fraction 0..1. `slot` permet l'annulation (kill).
pub fn export(ffmpeg: &Path, job: &ExportJob, slot: Arc<Mutex<Option<Child>>>, mut on_progress: impl FnMut(f64)) -> Result<()> {
    let part = job.dest.with_extension("part.mp4");
    let fps = format!("{}/{}", job.fps.0, job.fps.1);
    let mut cmd = Command::new(ffmpeg);
    cmd.args(["-v", "error", "-y", "-nostats", "-progress", "pipe:1", "-ss", &format!("{:.3}", job.start)])
        .arg("-i").arg(&job.src)
        .args(["-framerate", &fps, "-start_number", "0", "-i"]).arg(job.masks_dir.join("%05d.png"));
    match &job.background {
        // Décor animé : boucle (déjà aller-retour) répétée autant que nécessaire.
        Some(bg) if bg.extension().is_some_and(|e| e.eq_ignore_ascii_case("mp4")) => {
            cmd.args(["-stream_loop", "-1", "-i"]).arg(bg);
        }
        Some(bg) => { cmd.args(["-loop", "1", "-framerate", &fps, "-i"]).arg(bg); }
        None => { cmd.args(["-f", "lavfi", "-i", &format!("color=c={}:s={}x{}:r={fps}",
                                                          job.composite.background_color.replace('#', "0x"), job.out_width, job.out_height)]); }
    }
    cmd.args(["-t", &format!("{:.3}", job.duration), "-filter_complex", &filter_graph(job), "-map", "[out]"]);
    if job.has_audio {
        cmd.args(["-map", "0:a:0?", "-c:a", "aac", "-b:a", "192k"]);
    }
    cmd.args(encoder_args(&job.quality, job.videotoolbox))
        .args(["-pix_fmt", "yuv420p", "-movflags", "+faststart"])
        .arg(&part)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn()?;
    let stdout = child.stdout.take().expect("stdout capturé");
    let stderr = child.stderr.take().expect("stderr capturé");
    *slot.lock().unwrap() = Some(child);
    let err_reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = std::io::Read::read_to_string(&mut BufReader::new(stderr), &mut s);
        s
    });
    let total_us = job.duration * 1e6;
    for line in BufReader::new(stdout).lines().map_while(|l| l.ok()) {
        if let Some(v) = line.strip_prefix("out_time_us=").and_then(|v| v.parse::<f64>().ok()) {
            on_progress((v / total_us).clamp(0.0, 1.0));
        }
    }
    let status = slot.lock().unwrap().take().map(|mut c| c.wait()).transpose()?;
    let stderr_text = err_reader.join().unwrap_or_default();
    match status {
        Some(s) if s.success() => {
            fs::rename(&part, &job.dest)?;
            on_progress(1.0);
            Ok(())
        }
        Some(_) | None => {
            let _ = fs::remove_file(&part);
            if stderr_text.trim().is_empty() {
                err("Export annulé.")
            } else {
                err(format!("L'export a échoué : {}", stderr_text.trim()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates_and_sizes() {
        assert_eq!(parse_rate("30000/1001"), Some((30000, 1001)));
        assert_eq!(parse_rate("0/0"), None);
        assert_eq!(output_size(1080, 1920), (1080, 1920));
        assert_eq!(output_size(4096, 2160), (3840, 2026));
        assert_eq!(output_size(1281, 721), (1282, 722));
    }

    #[test]
    fn graph_contains_expected_stages() {
        let job = ExportJob {
            src: "a.mov".into(), start: 0.0, duration: 2.0, fps: (25, 1), has_audio: false,
            out_width: 1920, out_height: 1080, proxy_height: 720, masks_dir: "m".into(), background: None,
            composite: Composite { subject_scale: 0.5, subject_x: 0.25, edge: 0.25, temperature: -0.1, ..Composite::default() },
            quality: ExportQuality::Fast, videotoolbox: true, dest: "o.mp4".into(),
        };
        let g = filter_graph(&job);
        assert!(g.contains("fps=25/1"));
        assert!(g.contains("alphamerge,scale=960:540"));
        assert!(g.contains("boxblur=2.25:1"), "l'adoucissement suit l'échelle proxy -> sortie : {g}");
        assert!(g.contains("x=(W-w)/2+0.2500*W"));
        assert!(g.contains("lut=c0='clip((val-158.00)*2+128"), "{g}");
        assert!(g.contains("lutrgb=r='clip(val+-6.00"), "{g}");
    }
}
