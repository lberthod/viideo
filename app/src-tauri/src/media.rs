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
    /// Ombres de contact image par image (PNG gris, taille du proxy), si calculées.
    pub shadows_dir: Option<PathBuf>,
    /// Image RGBA de l'élément de premier plan (réglages dans `composite.foreground`).
    pub element: Option<PathBuf>,
    /// Normales approchées du sujet image par image (PNG RGB, taille du proxy), pour le relighting.
    pub normals_dir: Option<PathBuf>,
    pub dest: PathBuf,
}

/// Paramètres du light wrap, exprimés pour 720 px de haut (mêmes valeurs dans compositor.ts) :
/// bande de bord = masque × (1 − flou(masque)), mélange vers le décor flouté.
pub const WRAP_MASK_BLUR_720: f64 = 10.0;
pub const WRAP_BG_BLUR_720: f64 = 24.0;
pub fn wrap_gain(light_wrap: f64) -> f64 {
    2.0 * light_wrap
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

/// Relighting directionnel (mêmes formules dans compositor.ts).
///
/// Normale encodée n = (N + 1) / 2 × 255 ; lumière L = normalise(x, y, RELIGHT_LZ).
/// - ombrage (0..255) = clip(Σ 2·L_c·n_c − 255·Σ L_c) = 255 × max(0, N·L) ;
/// - gain (codé 127,5 = ×1) = 127,5 × (1 − A·s) + A·s·k_c × ombrage, A = RELIGHT_AMP, k = couleur de la lumière
///   normalisée à une moyenne de 1. Un ombrage moyen (≈ 0,5) laisse l'exposition inchangée ;
/// - sujet × gain / 127,5.
pub const RELIGHT_LZ: f64 = 0.6;
/// Amplitude du relighting à pleine intensité : côté éclairé jusqu'à ×1,8, côté opposé jusqu'à ×0,2.
pub const RELIGHT_AMP: f64 = 0.8;

pub struct RelightCoeffs {
    /// Coefficients (R, G, B, A) appliqués à la normale encodée (A = 255) : ombrage.
    pub shade: [f64; 4],
    /// Gain par canal : échelle de l'ombrage, et décalage (multiplié par A = 255).
    pub scale: [f64; 3],
    pub offset: f64,
}

pub fn light_key(hex: &str) -> [f64; 3] {
    let ch = |i: usize| u8::from_str_radix(hex.get(i..i + 2).unwrap_or("ff"), 16).unwrap_or(255) as f64;
    let (r, g, b) = (ch(1), ch(3), ch(5));
    let sum = (r + g + b).max(1.0);
    [r, g, b].map(|v| (3.0 * v / sum).min(2.0))
}

pub fn relight_coeffs(c: &Composite) -> RelightCoeffs {
    let (x, y, z) = (c.light_x, c.light_y, RELIGHT_LZ);
    let n = (x * x + y * y + z * z).sqrt();
    let l = [x / n, y / n, z / n];
    let s = c.relight;
    let k = light_key(&c.light_color);
    RelightCoeffs {
        shade: [2.0 * l[0], 2.0 * l[1], 2.0 * l[2], -(l[0] + l[1] + l[2])],
        scale: k.map(|kc| RELIGHT_AMP * s * kc),
        offset: 0.5 * (1.0 - RELIGHT_AMP * s),
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
    let fps = format!("{fn_}/{fd}");
    let shadow = job.shadows_dir.is_some() && c.shadow > 0.01;
    let wrap = c.light_wrap > 0.01;
    let element = job.element.is_some() && c.foreground.is_some();
    let relight = job.normals_dir.is_some() && c.relight > 0.01;
    let sh_idx = 3;
    let el_idx = if shadow { 4 } else { 3 };
    let nm_idx = el_idx + element as usize;
    let mut g = String::new();
    let src_out = if relight {
        let r = relight_coeffs(c);
        let [a, b, z, d] = r.shade;
        let shade: String = ["r", "g", "b"].iter()
            .map(|o| format!("{o}r={a:.4}:{o}g={b:.4}:{o}b={z:.4}:{o}a={d:.4}")).collect::<Vec<_>>().join(":");
        let [kr, kg, kb] = r.scale;
        let o = r.offset;
        g.push_str(&format!(
            "[{nm_idx}:v]scale={w}:{h}:flags=bilinear,format=rgba,colorchannelmixer={shade},\
colorchannelmixer=rr={kr:.4}:rg=0:rb=0:ra={o:.4}:gr=0:gg={kg:.4}:gb=0:ga={o:.4}:br=0:bg=0:bb={kb:.4}:ba={o:.4},format=gbrp[gain];"));
        "[s0]"
    } else {
        "[src]"
    };
    g.push_str(&format!(
        "[0:v]fps={fps},scale={w}:{h}:flags=bicubic,setsar=1,\
eq=brightness={b:.3}:contrast={ct:.3}:saturation={sa:.3},format=rgba{temp}{src_out};",
        b = c.brightness, ct = c.contrast, sa = c.saturation));
    if relight {
        g.push_str("[s0]format=gbrp[s1];[s1][gain]blend=all_expr='min(255\\,A*B/127.5)',format=rgba[src];");
    }
    g.push_str(&format!(
        "\
[1:v]scale={w}:{h}:flags=bilinear,format=gray{blur}{edge}{split};\
[src][m1]alphamerge,scale={sw}:{sh}:flags=bicubic[fg];\
[2:v]fps={fps},scale={w}:{h}:force_original_aspect_ratio=increase,crop={w}:{h},setsar=1{bgblur},format=rgba{bgsplit};",
        split = if wrap { ",split[m1][m2]" } else { "[m1]" },
        bgsplit = if wrap { ",split[bg][bgw]" } else { "[bg]" },
    ));
    let mut base = "[bg]".to_string();
    if shadow {
        // L'ombre suit exactement la transformation du sujet (échelle, position).
        g.push_str(&format!(
            "[{sh_idx}:v]scale={sw}:{sh}:flags=bilinear,format=gray[shm];color=c=black:s={sw}x{sh}:r={fps},format=rgba[blk];\
[blk][shm]alphamerge,colorchannelmixer=aa={op:.3}[sh];{base}[sh]overlay=x={x}:y={y}:shortest=1:format=auto[bgs];",
            op = c.shadow));
        base = "[bgs]".into();
    }
    g.push_str(&format!("{base}[fg]overlay=x={x}:y={y}:shortest=1:format=auto[comp];"));
    let mut cur = "[comp]".to_string();
    if wrap {
        let s1 = WRAP_MASK_BLUR_720 * h as f64 / 720.0;
        let s2 = WRAP_BG_BLUR_720 * h as f64 / 720.0;
        let k = wrap_gain(c.light_wrap);
        g.push_str(&format!(
            "color=c=black:s={w}x{h}:r={fps}[cb];[m2]scale={sw}:{sh}:flags=bilinear[m2s];[cb][m2s]overlay=x={x}:y={y}:shortest=1,format=gray,split[ma][mb];\
[mb]gblur=sigma={s1:.2},negate[nb];[ma][nb]blend=all_mode=multiply,lut=c0='clip(val*{k:.3}\\,0\\,255)',format=gbrp[band];\
[bgw]gblur=sigma={s2:.2},format=gbrp[bgb];{cur}format=gbrp[cg];[cg][bgb][band]maskedmerge,format=rgba[wrapped];"));
        cur = "[wrapped]".into();
    }
    if element {
        let f = c.foreground.as_ref().unwrap();
        let ew = even(w as f64 * f.width);
        g.push_str(&format!("[{el_idx}:v]scale={ew}:-2:flags=lanczos,format=rgba[el];{cur}[el]overlay=x={:.4}*W-w/2:y={:.4}*H-h:format=auto[withel];",
                            f.x, f.y));
        cur = "[withel]".into();
    }
    g.push_str(&format!("{cur}format=yuv420p[out]"));
    g
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
    if let Some(dir) = job.shadows_dir.as_ref().filter(|_| job.composite.shadow > 0.01) {
        cmd.args(["-framerate", &fps, "-start_number", "0", "-i"]).arg(dir.join("%05d.png"));
    }
    if let Some(el) = job.element.as_ref().filter(|_| job.composite.foreground.is_some()) {
        cmd.args(["-loop", "1", "-framerate", &fps, "-i"]).arg(el);
    }
    if let Some(dir) = job.normals_dir.as_ref().filter(|_| job.composite.relight > 0.01) {
        cmd.args(["-framerate", &fps, "-start_number", "0", "-i"]).arg(dir.join("%05d.png"));
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
            quality: ExportQuality::Fast, videotoolbox: true, shadows_dir: None, element: None, normals_dir: None, dest: "o.mp4".into(),
        };
        let g = filter_graph(&job);
        assert!(g.contains("fps=25/1"));
        assert!(g.contains("alphamerge,scale=960:540"));
        assert!(g.contains("boxblur=2.25:1"), "l'adoucissement suit l'échelle proxy -> sortie : {g}");
        assert!(g.contains("x=(W-w)/2+0.2500*W"));
        assert!(g.contains("lut=c0='clip((val-158.00)*2+128"), "{g}");
        assert!(g.contains("lutrgb=r='clip(val+-6.00"), "{g}");
        assert!(!g.contains("maskedmerge") && !g.contains("[el]"), "options désactivées : graphe minimal");
        // Ombre + light wrap + élément : indices d'entrée et étapes attendus.
        let mut job2 = job;
        job2.composite.shadow = 0.5;
        job2.composite.light_wrap = 0.5;
        job2.composite.foreground = Some(crate::project::Foreground { path: "assets/elements/a.png".into(), width: 0.5, x: 0.5, y: 1.0 });
        job2.shadows_dir = Some("s".into());
        job2.element = Some("e.png".into());
        let g = filter_graph(&job2);
        assert!(g.contains("[3:v]scale=960:540:flags=bilinear,format=gray[shm]"), "{g}");
        assert!(g.contains("colorchannelmixer=aa=0.500"));
        assert!(g.contains("[cg][bgb][band]maskedmerge"));
        assert!(g.contains("[4:v]scale=960:-2:flags=lanczos,format=rgba[el]"), "{g}");
        assert!(g.contains("overlay=x=0.5000*W-w/2:y=1.0000*H-h"));
        assert!(g.ends_with("[withel]format=yuv420p[out]"));
        // Relighting : normales en entrée 5 (après ombre et élément), gain multiplié au sujet.
        let mut job3 = job2;
        job3.composite.relight = 0.5;
        job3.composite.light_x = 1.0;
        job3.composite.light_y = 0.0;
        job3.normals_dir = Some("n".into());
        let g = filter_graph(&job3);
        assert!(g.starts_with("[5:v]scale=1920:1080:flags=bilinear,format=rgba,colorchannelmixer=rr=1.7150:rg=0.0000:rb=1.0290:ra=-1.3720"), "{g}");
        assert!(g.contains("[s1][gain]blend=all_expr='min(255\\,A*B/127.5)',format=rgba[src]"), "{g}");
    }

    #[test]
    fn relight_keeps_mean_exposure() {
        let c = Composite { relight: 1.0, light_color: "#ffffff".into(), ..Composite::default() };
        let r = relight_coeffs(&c);
        // Ombrage moyen 0,5 (127,5) : gain codé 127,5, soit ×1.
        assert!((r.offset * 255.0 + r.scale[0] * 127.5 - 127.5).abs() < 1e-9);
        assert_eq!(light_key("#ffffff"), [1.0, 1.0, 1.0]);
        let k = light_key("#ff8000");
        assert!(k[0] > 1.5 && k[2] == 0.0);
    }
}
