//! Adaptateur ComfyUI : exécute des workflows versionnés (workflows/comfyui/*.json) sur un serveur
//! ComfyUI **local**, et récupère l'image produite.
//!
//! Garde-fous : hôte local uniquement ; refus des nœuds d'API distants (`api_node`) ; vérification
//! des nœuds et des fichiers de modèles avant soumission ; aucun envoi de média.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::{err, AppError, Result};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workflow {
    pub id: String,
    pub version: u32,
    pub title: String,
    pub description: String,
    #[serde(default)]
    pub tested: bool,
    pub requires: Requires,
    /// paramètre -> liste de [nœud, entrée] à remplacer
    pub params: BTreeMap<String, Vec<[String; 2]>>,
    pub output_node: String,
    pub graph: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Requires {
    /// "Classe.entrée" -> fichiers attendus
    #[serde(default)]
    pub models: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Serialize)]
pub struct WorkflowStatus {
    pub id: String,
    pub version: u32,
    pub title: String,
    pub description: String,
    pub tested: bool,
    pub ready: bool,
    pub problems: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct ServerStatus {
    pub url: String,
    pub reachable: bool,
    pub version: Option<String>,
    pub detail: String,
    pub workflows: Vec<WorkflowStatus>,
}

/// Seuls les serveurs de cette machine sont acceptés : aucun média ni prompt ne quitte le Mac.
pub fn validate_url(url: &str) -> Result<String> {
    let u = url.trim().trim_end_matches('/');
    let rest = u.strip_prefix("http://").ok_or_else(|| AppError::Msg("Adresse ComfyUI invalide : http://127.0.0.1:<port> attendu.".into()))?;
    let host = if rest.starts_with('[') {
        rest.split(']').next().map(|h| format!("{h}]")).unwrap_or_default()
    } else {
        rest.split(':').next().unwrap_or_default().to_string()
    };
    if !matches!(host.as_str(), "127.0.0.1" | "localhost" | "[::1]") {
        return err("ComfyUI doit tourner sur ce Mac (127.0.0.1) : les serveurs distants sont refusés.");
    }
    if rest.contains('/') {
        return err("Adresse ComfyUI invalide : indiquez seulement http://hôte:port.");
    }
    Ok(u.to_string())
}

pub fn load_workflows(dir: &Path) -> Result<Vec<Workflow>> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).map_err(|_| AppError::Msg(format!("Dossier de workflows introuvable : {}", dir.display())))?.flatten() {
        let p = e.path();
        if p.extension().is_some_and(|x| x == "json") {
            let wf: Workflow = serde_json::from_str(&std::fs::read_to_string(&p)?)
                .map_err(|e2| AppError::Msg(format!("Workflow illisible {} : {e2}", p.display())))?;
            out.push(wf);
        }
    }
    out.sort_by(|a, b| (&a.id, a.version).cmp(&(&b.id, b.version)));
    Ok(out)
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout_connect(Duration::from_millis(800)).timeout(Duration::from_secs(30)).build()
}

fn get_json(url: &str) -> Result<Value> {
    agent().get(url).call()
        .map_err(|e| AppError::Msg(format!("ComfyUI ne répond pas ({url}) : {e}")))?
        .into_json().map_err(|e| AppError::Msg(format!("Réponse ComfyUI illisible : {e}")))
}

/// Vérifie un workflow contre le catalogue `/object_info` du serveur.
pub fn check_workflow(wf: &Workflow, object_info: &Value) -> Vec<String> {
    let mut problems = Vec::new();
    if let Some(nodes) = wf.graph.as_object() {
        for (id, node) in nodes {
            let class = node["class_type"].as_str().unwrap_or("");
            match object_info.get(class) {
                None => problems.push(format!("nœud « {class} » ({id}) absent de ce ComfyUI")),
                Some(info) => {
                    let remote = info["api_node"].as_bool().unwrap_or(false)
                        || info["python_module"].as_str().is_some_and(|m| m.starts_with("comfy_api_nodes"));
                    if remote {
                        problems.push(format!("nœud « {class} » : service en ligne, refusé (local uniquement)"));
                    }
                }
            }
        }
    }
    for (key, files) in &wf.requires.models {
        let Some((class, input)) = key.split_once('.') else { continue };
        let available: Vec<String> = object_info[class]["input"]["required"][input][0].as_array()
            .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();
        for f in files {
            if !available.contains(f) {
                problems.push(format!("modèle manquant pour {class} : {f}"));
            }
        }
    }
    problems
}

pub fn status(url: &str, workflows: &[Workflow]) -> ServerStatus {
    let base = match validate_url(url) {
        Ok(b) => b,
        Err(e) => return ServerStatus { url: url.into(), reachable: false, version: None, detail: e.to_string(), workflows: vec![] },
    };
    let stats = get_json(&format!("{base}/system_stats"));
    let info = stats.as_ref().ok().and_then(|_| get_json(&format!("{base}/object_info")).ok());
    let version = stats.as_ref().ok().and_then(|s| s["system"]["comfyui_version"].as_str().map(String::from));
    let wfs = workflows.iter().map(|wf| {
        let problems = match &info {
            Some(i) => check_workflow(wf, i),
            None => vec!["serveur ComfyUI injoignable".into()],
        };
        WorkflowStatus { id: wf.id.clone(), version: wf.version, title: wf.title.clone(), description: wf.description.clone(),
                         tested: wf.tested, ready: problems.is_empty(), problems }
    }).collect();
    ServerStatus {
        url: base.clone(),
        reachable: info.is_some(),
        version,
        detail: match &stats { Ok(_) => "connecté".into(), Err(e) => e.to_string() },
        workflows: wfs,
    }
}

/// Remplit les paramètres exposés du graphe.
pub fn fill(wf: &Workflow, params: &BTreeMap<String, Value>) -> Result<Value> {
    let mut graph = wf.graph.clone();
    for (name, value) in params {
        let Some(targets) = wf.params.get(name) else { continue };
        for [node, input] in targets {
            let slot = graph.get_mut(node).and_then(|n| n.get_mut("inputs")).and_then(|i| i.get_mut(input))
                .ok_or_else(|| AppError::Msg(format!("Workflow {} : entrée {node}.{input} introuvable", wf.id)))?;
            *slot = value.clone();
        }
    }
    Ok(graph)
}

pub struct RunResult {
    pub seconds: f64,
    pub prompt_id: String,
}

/// Soumet le workflow, attend la fin (en interrogeant /history) et écrit l'image dans `dest`.
/// `cancelled` est consulté entre deux interrogations ; en cas d'annulation, /interrupt est appelé.
pub fn run(url: &str, wf: &Workflow, params: &BTreeMap<String, Value>, dest: &PathBuf,
           cancelled: &dyn Fn() -> bool) -> Result<RunResult> {
    let base = validate_url(url)?;
    let info = get_json(&format!("{base}/object_info"))?;
    let problems = check_workflow(wf, &info);
    if !problems.is_empty() {
        return err(format!("Workflow {} v{} non exécutable : {}", wf.id, wf.version, problems.join(" ; ")));
    }
    let graph = fill(wf, params)?;
    let t0 = Instant::now();
    let resp: Value = agent().post(&format!("{base}/prompt"))
        .send_json(json!({"prompt": graph, "client_id": "videeo"}))
        .map_err(|e| match e {
            ureq::Error::Status(_, r) => AppError::Msg(format!("ComfyUI a refusé le workflow : {}", r.into_string().unwrap_or_default())),
            other => AppError::Msg(format!("ComfyUI injoignable : {other}")),
        })?
        .into_json().map_err(|e| AppError::Msg(format!("Réponse ComfyUI illisible : {e}")))?;
    let prompt_id = resp["prompt_id"].as_str().ok_or_else(|| AppError::Msg("ComfyUI n'a pas renvoyé d'identifiant de tâche.".into()))?.to_string();

    let deadline = Instant::now() + Duration::from_secs(1800);
    loop {
        if cancelled() {
            let _ = agent().post(&format!("{base}/interrupt")).call();
            return err("Génération annulée.");
        }
        if Instant::now() > deadline {
            return err("ComfyUI n'a pas terminé en 30 minutes.");
        }
        let h = get_json(&format!("{base}/history/{prompt_id}"))?;
        if let Some(entry) = h.get(&prompt_id) {
            let st = &entry["status"];
            if st["status_str"] == "error" {
                let msg = st["messages"].as_array().and_then(|m| m.iter().find(|x| x[0] == "execution_error"))
                    .map(|x| x[1]["exception_message"].as_str().unwrap_or("").to_string()).unwrap_or_default();
                return err(format!("Erreur ComfyUI : {msg}"));
            }
            if st["completed"].as_bool() == Some(true) {
                let img = &entry["outputs"][&wf.output_node]["images"][0];
                let (Some(name), Some(kind)) = (img["filename"].as_str(), img["type"].as_str()) else {
                    return err("ComfyUI n'a produit aucune image sur le nœud de sortie.");
                };
                let sub = img["subfolder"].as_str().unwrap_or("");
                let resp = agent().get(&format!("{base}/view")).query("filename", name).query("subfolder", sub).query("type", kind).call()
                    .map_err(|e| AppError::Msg(format!("Téléchargement de l'image ComfyUI impossible : {e}")))?;
                let mut bytes = Vec::new();
                std::io::Read::read_to_end(&mut resp.into_reader(), &mut bytes)?;
                let tmp = dest.with_extension("tmp.png");
                std::fs::write(&tmp, &bytes)?;
                std::fs::rename(&tmp, dest)?;
                return Ok(RunResult { seconds: (t0.elapsed().as_secs_f64() * 10.0).round() / 10.0, prompt_id });
            }
        }
        std::thread::sleep(Duration::from_millis(400));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_local_servers() {
        assert!(validate_url("http://127.0.0.1:8188").is_ok());
        assert!(validate_url("http://localhost:8188/").is_ok());
        assert!(validate_url("http://[::1]:8188").is_ok());
        assert!(validate_url("http://192.168.1.10:8188").is_err());
        assert!(validate_url("https://comfy.example.com").is_err());
        assert!(validate_url("http://127.0.0.1.evil.com:8188").is_err());
        assert!(validate_url("http://127.0.0.1:8188/prompt").is_err());
    }

    fn wf() -> Workflow {
        serde_json::from_str(include_str!("../../../workflows/comfyui/flux2-klein-t2i.v1.json")).unwrap()
    }

    #[test]
    fn fills_params_and_detects_problems() {
        let w = wf();
        let mut p = BTreeMap::new();
        p.insert("prompt".to_string(), json!("une forêt"));
        p.insert("width".to_string(), json!(768));
        let g = fill(&w, &p).unwrap();
        assert_eq!(g["4"]["inputs"]["text"], "une forêt");
        assert_eq!(g["5"]["inputs"]["width"], 768);
        assert_eq!(g["6"]["inputs"]["width"], 768);

        let info = json!({
            "UNETLoader": {"input": {"required": {"unet_name": [["autre.safetensors"]]}}},
            "CLIPLoader": {"input": {"required": {"clip_name": [["qwen_3_4b.safetensors"]]}}},
            "VAELoader": {"input": {"required": {"vae_name": [["flux2-vae.safetensors"]]}}},
            "CLIPTextEncode": {}, "EmptyFlux2LatentImage": {}, "Flux2Scheduler": {}, "RandomNoise": {},
            "KSamplerSelect": {}, "BasicGuider": {}, "SamplerCustomAdvanced": {}, "VAEDecode": {},
            "SaveImage": {"api_node": true}
        });
        let problems = check_workflow(&w, &info);
        assert!(problems.iter().any(|p| p.contains("flux-2-klein-4b.safetensors")), "{problems:?}");
        assert!(problems.iter().any(|p| p.contains("service en ligne")), "{problems:?}");
        assert_eq!(problems.len(), 2, "{problems:?}");
    }
}
