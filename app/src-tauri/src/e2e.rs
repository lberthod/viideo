//! Test de bout en bout du pipeline sans interface (lancé explicitement, il charge SAM 2) :
//!   VIDEEO_E2E_CLIP=/chemin/clip.mp4 cargo test e2e -- --ignored --nocapture

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::json;

use crate::env::Tools;
use crate::media::{self, ExportJob};
use crate::project::{Composite, Mask, MaskStatus, Point, Project, Prompt, Proxy};
use crate::worker::Worker;

#[test]
#[ignore]
fn full_pipeline() {
    let clip = PathBuf::from(std::env::var("VIDEEO_E2E_CLIP").expect("VIDEEO_E2E_CLIP"));
    let tools = Tools::detect();
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("projet");

    let probe = media::probe(&tools.ffprobe, &clip).unwrap();
    let mut p = Project::create(&dir, &clip.canonicalize().unwrap(), probe.clone()).unwrap();
    p.manifest.clip.duration = 3.0;
    let key = p.proxy_key();
    let rel = format!("media/proxy_{}", &key[..16]);
    let fps = (probe.fps_num, probe.fps_den);
    let r = media::extract_proxy(&tools.ffmpeg, &tools.ffprobe, &clip, 0.0, 3.0, fps, &p.resolve(&rel).unwrap()).unwrap();
    println!("proxy {}x{} {} images", r.width, r.height, r.frames);
    p.manifest.proxy = Some(Proxy { key, dir: rel.clone(), width: r.width, height: r.height, frames: r.frames, fps_num: fps.0, fps_den: fps.1 });

    // Invites du clip A de l'étape 0 (présentatrice) mises à l'échelle du proxy, plus une correction à mi-parcours.
    let sx = r.width as f64 / 1280.0;
    let prompts = vec![
        Prompt { frame: 0, points: vec![Point { x: 700.0 * sx, y: 180.0, positive: true }], r#box: Some([440.0 * sx, 60.0, 910.0 * sx, 719.0]) },
        Prompt { frame: r.frames / 2, points: vec![Point { x: 700.0 * sx, y: 340.0, positive: true }], r#box: None },
    ];
    p.validate_prompts(&prompts).unwrap();
    p.manifest.segmentation.prompts = prompts.clone();

    let progress = Arc::new(Mutex::new(0u64));
    let pr = progress.clone();
    let w = Worker::spawn(&tools, &p.dir, move |ev| { *pr.lock().unwrap() = ev["done"].as_u64().unwrap_or(0); }).unwrap();
    println!("{}", w.request("ping", json!({})).unwrap());
    w.request("load_model", json!({"model": "tiny", "dtype": "bf16"})).unwrap();
    let frames_dir = p.resolve(&rel).unwrap();
    let pv = w.request("preview", json!({"frames_dir": frames_dir, "frame": 0, "out_png": p.dir.join("cache/preview/x.png"), "prompts": [prompts[0].clone()]})).unwrap();
    println!("aperçu {pv}");
    // Un chemin hors du projet doit être refusé par le worker.
    assert!(w.request("preview", json!({"frames_dir": frames_dir, "frame": 0, "out_png": "/tmp/evil.png", "prompts": [prompts[0].clone()]})).is_err());

    let mask_key = p.mask_key().unwrap();
    let mask_rel = format!("cache/masks/{}", &mask_key[..16]);
    let res = w.request("propagate", json!({"frames_dir": frames_dir, "out_dir": p.resolve(&mask_rel).unwrap(), "prompts": prompts, "chunk_size": 32,
                                           "cleanup": {"min_island_ratio": 0.05, "max_hole_ratio": 0.002}})).unwrap();
    assert!(p.resolve(&mask_rel).unwrap().join("quality.json").exists(), "quality.json écrit");
    println!("propagation {res}");
    assert_eq!(res["written"].as_u64().unwrap() as u32, r.frames);
    assert!(*progress.lock().unwrap() > 0, "événements de progression reçus");
    p.manifest.segmentation.mask = Some(Mask { key: mask_key, dir: mask_rel.clone(), frames: r.frames, seconds: 0.0, empty_frames: 0, flagged: 0 });
    p.refresh_mask_status().unwrap();
    assert_eq!(p.manifest.segmentation.status, MaskStatus::Ready);
    w.shutdown();

    let (ow, oh) = media::output_size(probe.display_width, probe.display_height);
    let dest = p.resolve("exports/test.mp4").unwrap();
    let job = ExportJob {
        src: clip.clone(), start: 0.0, duration: 3.0, fps, has_audio: probe.has_audio, out_width: ow, out_height: oh,
        proxy_height: r.height, masks_dir: p.resolve(&mask_rel).unwrap(), background: None,
        composite: Composite { subject_scale: 0.8, subject_x: 0.1, edge: 0.15, temperature: 0.1, ..Composite::default() },
        quality: crate::project::ExportQuality::Fast, videotoolbox: tools.videotoolbox, dest: dest.clone(),
    };
    let mut last = 0.0;
    media::export(&tools.ffmpeg, &job, Arc::new(Mutex::new(None)), |f| last = f).unwrap();
    let out = media::probe(&tools.ffprobe, &dest).unwrap();
    println!("export {}x{} {:.2}s {} octets", out.display_width, out.display_height, out.duration, out.size_bytes);
    assert_eq!((out.display_width, out.display_height), (ow, oh));
    assert!((out.duration - 3.0).abs() < 0.2);
    assert_eq!(last, 1.0);
    p.save().unwrap();
    assert!(Project::open(&dir).is_ok(), "le projet se rouvre");
}

/// Adaptateur ComfyUI contre un vrai serveur local (lancé à part) :
///   VIDEEO_E2E_COMFY=http://127.0.0.1:8188 cargo test e2e_comfy -- --ignored --nocapture
#[test]
#[ignore]
fn e2e_comfy() {
    use std::collections::BTreeMap;
    use crate::comfy;
    let url = std::env::var("VIDEEO_E2E_COMFY").unwrap_or_else(|_| "http://127.0.0.1:8188".into());
    let tools = Tools::detect();
    let wfs = comfy::load_workflows(&tools.workflows).unwrap();
    let st = comfy::status(&url, &wfs);
    println!("serveur {} {:?} : {}", st.url, st.version, st.detail);
    for w in &st.workflows {
        println!("  {} v{} prêt={} {:?}", w.id, w.version, w.ready, w.problems);
    }
    assert!(st.reachable);

    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("out.png");
    let solid = wfs.iter().find(|w| w.id == "solid-color").unwrap();
    let mut params = BTreeMap::new();
    params.insert("width".to_string(), json!(320));
    params.insert("height".to_string(), json!(176));
    params.insert("seed".to_string(), json!(0x3366cc));
    let r = comfy::run(&url, solid, &params, &dest, &|| false).unwrap();
    let size = crate::media::probe_image(&tools.ffprobe, &dest).unwrap();
    println!("solid-color : {:.1} s, prompt {}, image {:?}", r.seconds, r.prompt_id, size);
    assert_eq!(size, (320, 176));

    // FLUX.2 sans les poids au format ComfyUI : refus explicite, sans soumission.
    let flux = wfs.iter().find(|w| w.id == "flux2-klein-t2i").unwrap();
    let e = comfy::run(&url, flux, &BTreeMap::new(), &tmp.path().join("f.png"), &|| false).err().unwrap().to_string();
    println!("flux2 : {e}");
    assert!(e.contains("modèle manquant"));

    // Un workflow utilisant un nœud d'API distant est refusé avant tout envoi.
    let mut remote = solid.clone();
    remote.graph["1"] = json!({"class_type": "Flux2ProImageNode", "inputs": {"prompt": "x", "width": 512, "height": 512, "seed": 0, "prompt_upsampling": false}});
    let e = comfy::run(&url, &remote, &BTreeMap::new(), &tmp.path().join("r.png"), &|| false).err().unwrap().to_string();
    println!("nœud distant : {e}");
    assert!(e.contains("service en ligne"));
}
