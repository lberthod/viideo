//! Planificateur LLM local (Ollama). Transforme une demande en langage naturel en plan JSON
//! contraint par un schéma (liste blanche d'opérations), puis le confie à `plan::validate`.
//!
//! Serveur local uniquement ; aucune donnée média n'est envoyée, seulement l'état du projet
//! (durées, formats, décors disponibles) et la demande de l'utilisateur.

use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};

use crate::error::{err, AppError, Result};
use crate::plan::{self, Context, Mode, Plan, RawPlan, OPS};

pub const DEFAULT_URL: &str = "http://127.0.0.1:11434";
/// Modèle par défaut : qwen3:30b-a3b (MoE, 3 milliards de paramètres actifs) est à la fois plus juste
/// et plus rapide que qwen3:8b sur le banc de l'étape 4 ; repli sur qwen3:8b s'il n'est pas installé.
pub const DEFAULT_MODEL: &str = "qwen3:30b-a3b";
pub const FALLBACK_MODEL: &str = "qwen3:8b";

/// Modèles proposés : généralistes, sans variante « débridée ».
pub const ALLOWED_MODELS: [&str; 4] = ["qwen3:8b", "qwen3:30b-a3b", "qwen3.5:35b-a3b", "qwen3.8:27b"];

pub fn validate_url(url: &str) -> Result<String> {
    let u = url.trim().trim_end_matches('/');
    let rest = u.strip_prefix("http://").ok_or_else(|| AppError::Msg("Adresse Ollama invalide : http://127.0.0.1:11434 attendu.".into()))?;
    let host = rest.split(':').next().unwrap_or_default();
    if !matches!(host, "127.0.0.1" | "localhost") || rest.contains('/') {
        return err("Le LLM doit tourner sur ce Mac (Ollama sur 127.0.0.1) : aucun service distant.");
    }
    Ok(u.to_string())
}

fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "summary": {"type": "string"},
            "steps": {"type": "array", "items": {
                "type": "object",
                "properties": {
                    "op": {"type": "string", "enum": OPS},
                    "params": {"type": "object"},
                    "why": {"type": "string"}
                },
                "required": ["op", "params", "why"]
            }},
            "warnings": {"type": "array", "items": {"type": "string"}}
        },
        "required": ["summary", "steps", "warnings"]
    })
}

fn system_prompt(mode: Mode) -> String {
    let mode_txt = match mode {
        Mode::Fast => "Rapide : le moins d'étapes et de calcul possible, une seule variante de décor, export rapide.",
        Mode::Balanced => "Équilibré : bon compromis entre qualité et temps.",
        Mode::Quality => "Qualité : privilégie le rendu (plusieurs variantes de décor, harmonisation, export haute qualité).",
    };
    format!(
        "Tu es le planificateur de videeo, une application locale qui remplace le décor derrière une personne filmée.\n\
Tu reçois l'état du projet et une demande en français. Tu réponds UNIQUEMENT par un objet JSON {{summary, steps, warnings}}.\n\
- summary : une phrase en français qui résume le plan.\n\
- steps : la liste ordonnée des opérations, chacune avec op, params et why (une courte justification en français).\n\
- warnings : ce que l'application ne peut pas faire dans la demande, ou les risques (vide si rien).\n\
N'utilise que les opérations ci-dessous, avec des paramètres dans les bornes. N'invente ni fichier, ni identifiant de décor absent de l'état du projet.\n\
Règles :\n\
1. Décor : utilise use_background UNIQUEMENT si la demande correspond clairement à la description d'un décor disponible. Pour tout autre lieu demandé (plage, bureau, autre planète…), utilise generate_background avec une description précise du lieu, de la lumière et de l'ambiance. Pour une couleur unie (fond vert, noir, blanc…), utilise solid_background avec la couleur en #rrggbb.\n\
2. Si la demande ne parle pas de décor (lumière, flou, cadrage), ne change pas le décor.\n\
3. Inclus segment_subject si le masque n'est pas « prêt ».\n\
4. N'inclus set_clip que si l'utilisateur demande une autre partie de la vidéo.\n\
5. Termine par export si l'utilisateur veut un résultat final ou ne précise rien.\n\
6. L'export garde la résolution et la cadence de la vidéo source : si la demande en exige d'autres, dis-le dans warnings.\n\
Mode demandé — {mode_txt}\n\n{}",
        plan::catalogue()
    )
}

#[derive(Debug, Serialize)]
pub struct PlannerResult {
    pub plan: Plan,
    pub model: String,
    pub seconds: f64,
    pub attempts: u32,
}

fn chat(base: &str, model: &str, messages: &[Value]) -> Result<String> {
    let agent = ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(2)).timeout(Duration::from_secs(300)).build();
    let body = json!({
        "model": model,
        "messages": messages,
        "format": schema(),
        "stream": false,
        "think": false,
        "keep_alive": "120s",
        "options": {"temperature": 0.0, "seed": 7, "num_ctx": 8192}
    });
    let resp: Value = agent.post(&format!("{base}/api/chat")).send_json(body)
        .map_err(|e| match e {
            ureq::Error::Status(404, _) => AppError::Msg(format!("Modèle « {model} » absent d'Ollama (ollama pull {model}).")),
            ureq::Error::Status(_, r) => AppError::Msg(format!("Ollama a refusé la requête : {}", r.into_string().unwrap_or_default())),
            other => AppError::Msg(format!("Ollama injoignable sur {base} : {other}")),
        })?
        .into_json().map_err(|e| AppError::Msg(format!("Réponse Ollama illisible : {e}")))?;
    Ok(resp["message"]["content"].as_str().unwrap_or("").to_string())
}

/// Demande un plan au LLM. Une seconde tentative est faite si la réponse n'est pas un JSON valide
/// ou si des étapes ont été rejetées par la validation, en renvoyant les erreurs au modèle.
pub fn propose(url: &str, model: &str, instruction: &str, project_state: &Value, ctx: &Context, mode: Mode) -> Result<PlannerResult> {
    let base = validate_url(url)?;
    if !ALLOWED_MODELS.contains(&model) {
        return err(format!("Modèle « {model} » non proposé pour la planification."));
    }
    let instruction = instruction.trim();
    if instruction.is_empty() || instruction.chars().count() > 2000 {
        return err("Décrivez le résultat voulu (2 000 caractères au maximum).");
    }
    let t0 = Instant::now();
    let mut messages = vec![
        json!({"role": "system", "content": system_prompt(mode)}),
        json!({"role": "user", "content": format!("État du projet :\n{}\n\nDemande :\n{instruction}",
                                                    serde_json::to_string_pretty(project_state)?)}),
    ];
    let mut last: Option<Plan> = None;
    for attempt in 1..=2u32 {
        let content = chat(&base, model, &messages)?;
        let parsed: std::result::Result<RawPlan, _> = serde_json::from_str(&content);
        let feedback = match parsed {
            Ok(raw) => {
                let p = plan::validate(&raw, ctx, mode);
                if p.rejected.is_empty() || attempt == 2 {
                    return Ok(PlannerResult { plan: p, model: model.into(), seconds: round1(t0.elapsed().as_secs_f64()), attempts: attempt });
                }
                let msg = p.rejected.iter().map(|r| format!("- {} : {}", r.op, r.reason)).collect::<Vec<_>>().join("\n");
                last = Some(p);
                format!("Certaines étapes ont été refusées par la validation :\n{msg}\nRenvoie le plan complet corrigé (même format JSON).")
            }
            Err(e) => format!("Ta réponse n'est pas un JSON valide ({e}). Renvoie uniquement l'objet JSON demandé."),
        };
        messages.push(json!({"role": "assistant", "content": content}));
        messages.push(json!({"role": "user", "content": feedback}));
    }
    match last {
        Some(plan) => Ok(PlannerResult { plan, model: model.into(), seconds: round1(t0.elapsed().as_secs_f64()), attempts: 2 }),
        None => err("Le LLM n'a pas produit de plan exploitable. Reformulez la demande ou choisissez un autre modèle."),
    }
}

/// Décharge le modèle d'Ollama (keep_alive = 0) avant une tâche lourde : un seul gros modèle en mémoire.
pub fn release(url: &str, model: &str) {
    if let Ok(base) = validate_url(url) {
        let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(5)).build();
        let _ = agent.post(&format!("{base}/api/generate")).send_json(json!({"model": model, "keep_alive": 0}));
    }
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

#[derive(Debug, Serialize)]
pub struct LlmStatus {
    pub url: String,
    pub reachable: bool,
    pub version: Option<String>,
    /// Modèles autorisés et installés.
    pub models: Vec<String>,
    pub detail: String,
}

pub fn status(url: &str) -> LlmStatus {
    let base = match validate_url(url) {
        Ok(b) => b,
        Err(e) => return LlmStatus { url: url.into(), reachable: false, version: None, models: vec![], detail: e.to_string() },
    };
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(3)).build();
    let version = agent.get(&format!("{base}/api/version")).call().ok()
        .and_then(|r| r.into_json::<Value>().ok()).and_then(|v| v["version"].as_str().map(String::from));
    let tags: Option<Value> = agent.get(&format!("{base}/api/tags")).call().ok().and_then(|r| r.into_json().ok());
    let installed: Vec<String> = tags.as_ref().and_then(|t| t["models"].as_array())
        .map(|a| a.iter().filter_map(|m| m["name"].as_str().map(String::from)).collect()).unwrap_or_default();
    let models: Vec<String> = ALLOWED_MODELS.iter().filter(|m| installed.iter().any(|i| i == *m)).map(|m| m.to_string()).collect();
    LlmStatus {
        url: base,
        reachable: version.is_some(),
        version,
        detail: if tags.is_none() { "Ollama ne répond pas : lancez l'application Ollama.".into() }
                else if models.is_empty() { format!("Aucun modèle proposé n'est installé (ex. ollama pull {DEFAULT_MODEL}).") }
                else { "prêt".into() },
        models,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_only_and_allowed_models() {
        assert!(validate_url("http://127.0.0.1:11434").is_ok());
        assert!(validate_url("http://localhost:11434/").is_ok());
        assert!(validate_url("https://api.example.com").is_err());
        assert!(validate_url("http://10.0.0.2:11434").is_err());
        assert!(!ALLOWED_MODELS.iter().any(|m| m.to_lowercase().contains("obliterat") || m.contains("uncensored")));
        let s = schema();
        assert_eq!(s["properties"]["steps"]["items"]["properties"]["op"]["enum"].as_array().unwrap().len(), OPS.len());
    }
}
