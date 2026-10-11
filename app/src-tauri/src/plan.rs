//! Plan de transformation proposé par le LLM : liste blanche d'opérations, validation stricte,
//! ordre canonique, prérequis et estimation des coûts. **Le LLM propose, Rust valide.**
//!
//! Le plan ne contient que des opérations de [`OPS`] avec des paramètres bornés ; il n'y a ni chemin,
//! ni commande, ni texte exécuté. Chaque étape est ensuite exécutée par la commande Tauri
//! correspondante, qui revalide ses propres paramètres.

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::project::{MaskStatus, Project};

pub const PLAN_SCHEMA_VERSION: u32 = 1;

/// Opérations autorisées, dans l'ordre canonique d'exécution.
pub const OPS: [&str; 11] = [
    "set_clip", "segment_subject", "generate_background", "use_background", "solid_background",
    "framing", "lighting", "adjust", "harmonize", "export", "note",
];

const FRAMINGS: [&str; 5] = ["center", "ground", "third-left", "third-right", "closeup"];
const LIGHTINGS: [&str; 6] = ["neutral", "golden", "overcast", "studio", "night", "dramatic"];
const QUALITIES: [&str; 3] = ["fast", "balanced", "high"];

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Fast,
    Balanced,
    Quality,
}

/// Plan brut tel que produit par le LLM (format imposé par le schéma JSON envoyé à Ollama).
#[derive(Debug, Clone, Deserialize)]
pub struct RawPlan {
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub steps: Vec<RawStep>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RawStep {
    pub op: String,
    #[serde(default)]
    pub params: Value,
    #[serde(default)]
    pub why: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Estimate {
    pub seconds: f64,
    /// Modèles chargés par l'étape.
    pub models: Vec<String>,
    /// Pic mémoire approximatif (Go) pendant l'étape.
    pub peak_gb: f64,
    /// Espace disque ajouté (Mo).
    pub disk_mb: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Step {
    pub op: String,
    pub params: Value,
    pub why: String,
    /// Action requise de l'utilisateur avant d'exécuter cette étape (ex. sélectionner le sujet).
    pub needs_user: Option<String>,
    /// Étape bloquée (prérequis manquant) : elle ne sera pas exécutée telle quelle.
    pub blocked: Option<String>,
    pub enabled: bool,
    pub estimate: Estimate,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rejected {
    pub op: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Totals {
    pub seconds: f64,
    pub peak_gb: f64,
    pub disk_mb: f64,
    pub models: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub schema_version: u32,
    pub summary: String,
    pub mode: Mode,
    pub steps: Vec<Step>,
    pub warnings: Vec<String>,
    pub rejected: Vec<Rejected>,
    pub totals: Totals,
    /// Durée estimée du même plan dans chaque mode (alternatives).
    pub alternatives: Vec<(Mode, f64)>,
}

/// Mesures des étapes 0 à 3 (M5 Pro 64 Go) utilisées pour les estimations.
mod cost {
    pub const SAM_FPS: f64 = 5.5;
    pub const SAM_LOAD_S: f64 = 3.0;
    pub const SAM_BASE_GB: f64 = 2.4;
    pub const SAM_GB_PER_FRAME: f64 = 0.0126;
    pub const SAM_CHUNK: f64 = 400.0;
    pub const MASK_MB_PER_FRAME: f64 = 0.006;
    pub const PROXY_S_PER_FRAME: f64 = 0.004;
    pub const PROXY_MB_PER_FRAME: f64 = 0.25;
    pub const FLUX_LOAD_S: f64 = 8.0;
    pub const FLUX_S_PER_IMAGE: f64 = 9.5;
    pub const FLUX_GB: f64 = 24.0;
    pub const BG_MB: f64 = 1.5;
    /// Secondes d'export par seconde de vidéo, pour 720p à 24 i/s.
    pub fn export_factor(q: &str) -> f64 {
        match q { "fast" => 0.11, "high" => 0.6, _ => 0.3 }
    }
}

pub struct Context {
    pub clip_start: f64,
    pub frames: f64,
    pub fps: f64,
    pub duration: f64,
    pub source_duration: f64,
    pub out_pixels: f64,
    pub has_proxy: bool,
    pub has_prompts: bool,
    pub mask_ready: bool,
    pub background_ids: Vec<String>,
    pub export_quality: String,
}

impl Context {
    pub fn from_project(p: &Project, background_ids: Vec<String>) -> Context {
        let m = &p.manifest;
        let pr = &m.source.probe;
        let fps = pr.fps_num as f64 / pr.fps_den.max(1) as f64;
        let (w, h) = crate::media::output_size(pr.display_width, pr.display_height);
        Context {
            clip_start: m.clip.start,
            frames: m.proxy.as_ref().map(|x| x.frames as f64).unwrap_or(m.clip.duration * fps),
            fps,
            duration: m.clip.duration,
            source_duration: pr.duration,
            out_pixels: w as f64 * h as f64,
            has_proxy: m.proxy.is_some(),
            has_prompts: !m.segmentation.prompts.is_empty(),
            mask_ready: m.segmentation.status == MaskStatus::Ready,
            background_ids,
            export_quality: match m.export_settings.quality {
                crate::project::ExportQuality::Fast => "fast",
                crate::project::ExportQuality::Balanced => "balanced",
                crate::project::ExportQuality::High => "high",
            }.into(),
        }
    }
}

fn num(params: &Value, key: &str) -> Option<f64> {
    params.get(key).and_then(|v| v.as_f64())
}

fn in_range(name: &str, v: f64, lo: f64, hi: f64) -> Result<f64, String> {
    if v.is_finite() && v >= lo && v <= hi { Ok(v) } else { Err(format!("{name} = {v} hors limites [{lo} ; {hi}]")) }
}

fn one_of(name: &str, v: Option<&str>, allowed: &[&str]) -> Result<String, String> {
    match v {
        Some(s) if allowed.contains(&s) => Ok(s.to_string()),
        Some(s) => Err(format!("{name} « {s} » inconnu (attendu : {})", allowed.join(", "))),
        None => Err(format!("{name} manquant")),
    }
}

/// Valide et normalise les paramètres d'une opération. Les clés inconnues sont ignorées.
fn validate_step(op: &str, p: &Value, ctx: &Context, mode: Mode) -> Result<Value, String> {
    let p = if p.is_object() { p.clone() } else { json!({}) };
    Ok(match op {
        "set_clip" => {
            let start = in_range("début", num(&p, "start").unwrap_or(0.0), 0.0, (ctx.source_duration - 0.2).max(0.0))?;
            let max = (ctx.source_duration - start).min(crate::project::MAX_CLIP_SECONDS);
            let duration = num(&p, "duration").unwrap_or(max.min(10.0));
            json!({"start": start, "duration": in_range("durée", duration, 0.2, max)?})
        }
        "segment_subject" => json!({"cleanup": p.get("cleanup").and_then(|v| v.as_bool()).unwrap_or(true)}),
        "generate_background" => {
            let prompt = p.get("prompt").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
            if prompt.is_empty() || prompt.chars().count() > 1200 {
                return Err("description du décor vide ou trop longue (1 200 caractères max)".into());
            }
            let default_variants = match mode { Mode::Fast => 1.0, Mode::Balanced => 2.0, Mode::Quality => 3.0 };
            let variants = in_range("variantes", num(&p, "variants").unwrap_or(default_variants).round(), 1.0, 4.0)?;
            json!({"prompt": prompt, "variants": variants as u32,
                   "empty_scene": p.get("empty_scene").and_then(|v| v.as_bool()).unwrap_or(true)})
        }
        "use_background" => {
            let id = p.get("id").and_then(|v| v.as_str()).unwrap_or("");
            if !ctx.background_ids.iter().any(|b| b == id) {
                return Err(format!("décor « {id} » absent de la bibliothèque du projet"));
            }
            json!({"id": id})
        }
        "solid_background" => {
            let c = p.get("color").and_then(|v| v.as_str()).unwrap_or("");
            if !(c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|ch| ch.is_ascii_hexdigit())) {
                return Err(format!("couleur « {c} » invalide (#rrggbb)"));
            }
            json!({"color": c})
        }
        "framing" => json!({"preset": one_of("cadrage", p.get("preset").and_then(|v| v.as_str()), &FRAMINGS)?}),
        "lighting" => json!({"preset": one_of("lumière", p.get("preset").and_then(|v| v.as_str()), &LIGHTINGS)?}),
        "adjust" => {
            // Mêmes bornes que Composite::validate : un réglage hors bornes est refusé, pas tronqué.
            let bounds = [("brightness", -0.5, 0.5), ("contrast", 0.5, 2.0), ("saturation", 0.0, 2.0), ("temperature", -0.5, 0.5),
                          ("bg_blur", 0.0, 30.0), ("edge", -0.5, 0.5), ("feather", 0.0, 20.0), ("subject_scale", 0.1, 4.0),
                          ("subject_x", -1.0, 1.0), ("subject_y", -1.0, 1.0)];
            let mut out = Map::new();
            for (k, lo, hi) in bounds {
                if let Some(v) = num(&p, k) {
                    out.insert(k.into(), json!(in_range(k, v, lo, hi)?));
                }
            }
            if out.is_empty() {
                return Err("aucun réglage reconnu".into());
            }
            Value::Object(out)
        }
        "harmonize" => json!({}),
        "export" => {
            let default = match mode { Mode::Fast => "fast", Mode::Balanced => ctx.export_quality.as_str(), Mode::Quality => "high" };
            json!({"quality": one_of("qualité", Some(p.get("quality").and_then(|v| v.as_str()).unwrap_or(default)), &QUALITIES)?})
        }
        "note" => json!({"text": p.get("text").and_then(|v| v.as_str()).unwrap_or("").chars().take(300).collect::<String>()}),
        other => return Err(format!("opération « {other} » non autorisée")),
    })
}

fn estimate(op: &str, params: &Value, frames: f64, ctx: &Context) -> Estimate {
    let duration = frames / ctx.fps.max(1.0);
    match op {
        "set_clip" => Estimate { seconds: (frames * cost::PROXY_S_PER_FRAME).max(0.5), models: vec![], peak_gb: 0.3,
                                 disk_mb: frames * cost::PROXY_MB_PER_FRAME },
        "segment_subject" => Estimate {
            seconds: cost::SAM_LOAD_S + frames / cost::SAM_FPS,
            models: vec!["SAM 2.1 tiny (bf16)".into()],
            peak_gb: cost::SAM_BASE_GB + frames.min(cost::SAM_CHUNK) * cost::SAM_GB_PER_FRAME,
            disk_mb: frames * cost::MASK_MB_PER_FRAME,
        },
        "generate_background" => {
            let n = params["variants"].as_f64().unwrap_or(1.0);
            Estimate { seconds: cost::FLUX_LOAD_S + n * cost::FLUX_S_PER_IMAGE, models: vec!["FLUX.2 klein 4B".into()],
                       peak_gb: cost::FLUX_GB, disk_mb: n * cost::BG_MB }
        }
        "export" => {
            let q = params["quality"].as_str().unwrap_or("balanced");
            let k = cost::export_factor(q) * (ctx.fps / 24.0) * (ctx.out_pixels / 921_600.0);
            Estimate { seconds: (duration * k).max(1.0), models: vec![], peak_gb: 1.0,
                       disk_mb: duration * (ctx.out_pixels / 921_600.0) * 0.5 }
        }
        _ => Estimate::default(),
    }
}

fn op_rank(op: &str) -> usize {
    OPS.iter().position(|o| *o == op).unwrap_or(OPS.len())
}

/// Transforme le plan brut en plan validé pour un mode donné.
pub fn validate(raw: &RawPlan, ctx: &Context, mode: Mode) -> Plan {
    let (steps, rejected) = build_steps(raw, ctx, mode);
    let mut warnings: Vec<String> = raw.warnings.iter().take(6).map(|w| w.chars().take(300).collect()).collect();
    if steps.is_empty() {
        warnings.push("Le plan ne contient aucune étape exécutable.".into());
    }
    let totals = totals(&steps);
    let alternatives = [Mode::Fast, Mode::Balanced, Mode::Quality].iter().map(|m| {
        let secs = if *m == mode { totals.seconds } else { totals_for_mode(raw, ctx, *m) };
        (*m, (secs * 10.0).round() / 10.0)
    }).collect();
    Plan { schema_version: PLAN_SCHEMA_VERSION, summary: raw.summary.chars().take(500).collect(), mode, steps,
           warnings, rejected, totals, alternatives }
}

/// Validation, dédoublonnage, ordre canonique, prérequis et estimations : une seule logique,
/// utilisée pour le plan affiché comme pour le chiffrage des autres modes.
fn build_steps(raw: &RawPlan, ctx: &Context, mode: Mode) -> (Vec<Step>, Vec<Rejected>) {
    let mut rejected = Vec::new();
    let mut steps: Vec<Step> = Vec::new();
    for s in raw.steps.iter().take(24) {
        let op = s.op.trim();
        match validate_step(op, &s.params, ctx, mode) {
            Ok(params) => {
                // Une seule occurrence par opération (la dernière l'emporte), sauf `adjust` (fusionné) et `note`.
                if op == "adjust" {
                    if let Some(prev) = steps.iter_mut().find(|x| x.op == "adjust") {
                        for (k, v) in params.as_object().unwrap() {
                            prev.params[k] = v.clone();
                        }
                        continue;
                    }
                } else if op != "note" {
                    steps.retain(|x| x.op != op);
                }
                // Un seul type de décor.
                if ["generate_background", "use_background", "solid_background"].contains(&op) {
                    steps.retain(|x| !["generate_background", "use_background", "solid_background"].contains(&x.op.as_str()));
                }
                steps.push(Step { op: op.into(), params, why: s.why.chars().take(400).collect(), needs_user: None,
                                  blocked: None, enabled: true, estimate: Estimate::default() });
            }
            Err(reason) => rejected.push(Rejected { op: op.chars().take(40).collect(), reason }),
        }
    }
    steps.sort_by_key(|s| op_rank(&s.op));

    // Un changement d'extrait identique à l'extrait actuel n'est pas un changement : il effacerait
    // pour rien la sélection du sujet.
    steps.retain(|s| !(s.op == "set_clip" && ctx.has_proxy
        && (s.params["start"].as_f64().unwrap_or(-1.0) - ctx.clip_start).abs() < 0.05
        && (s.params["duration"].as_f64().unwrap_or(-1.0) - ctx.duration).abs() < 0.05));

    // Prérequis complétés par l'application (pas par le LLM) : sans masque prêt, toute étape qui en
    // dépend impose un détourage. L'ajout est visible et justifié dans le plan.
    let clip_changes = steps.iter().any(|s| s.op == "set_clip");
    let needs_mask = steps.iter().any(|s| ["framing", "harmonize", "export"].contains(&s.op.as_str()));
    if needs_mask && !steps.iter().any(|s| s.op == "segment_subject") && (!ctx.mask_ready || clip_changes) {
        steps.push(Step { op: "segment_subject".into(), params: json!({"cleanup": true}),
                          why: "Ajouté par l'application : le masque du sujet est nécessaire pour la suite du plan.".into(),
                          needs_user: None, blocked: None, enabled: true, estimate: Estimate::default() });
        steps.sort_by_key(|s| op_rank(&s.op));
    }

    let frames = steps.iter().find(|s| s.op == "set_clip")
        .map(|s| (s.params["duration"].as_f64().unwrap_or(ctx.duration) * ctx.fps).round())
        .unwrap_or(ctx.frames);
    let will_have_proxy = ctx.has_proxy || clip_changes;
    // Changer d'extrait efface les images clés : il faudra resélectionner le sujet.
    let prompts_ok = ctx.has_prompts && !clip_changes;
    let mask_ok = (ctx.mask_ready && !clip_changes) || steps.iter().any(|s| s.op == "segment_subject");
    for s in steps.iter_mut() {
        s.estimate = estimate(&s.op, &s.params, frames, ctx);
        match s.op.as_str() {
            "segment_subject" => {
                if !will_have_proxy {
                    s.blocked = Some("aucun extrait préparé".into());
                } else if !prompts_ok {
                    s.needs_user = Some("Sélectionnez le sujet (clic ou rectangle) sur une image, puis l'étape reprendra.".into());
                }
            }
            "framing" | "harmonize" | "export" if !mask_ok => {
                s.blocked = Some("aucun masque du sujet : ajoutez l'étape « détourage du sujet »".into());
            }
            _ => {}
        }
    }
    (steps, rejected)
}

fn totals(steps: &[Step]) -> Totals {
    let on: Vec<&Step> = steps.iter().filter(|s| s.enabled && s.blocked.is_none()).collect();
    let mut models: Vec<String> = on.iter().flat_map(|s| s.estimate.models.clone()).collect();
    models.dedup();
    Totals {
        seconds: (on.iter().map(|s| s.estimate.seconds).sum::<f64>() * 10.0).round() / 10.0,
        // Les modèles sont chargés l'un après l'autre : le pic est le maximum, pas la somme.
        peak_gb: on.iter().map(|s| s.estimate.peak_gb).fold(0.0, f64::max),
        disk_mb: (on.iter().map(|s| s.estimate.disk_mb).sum::<f64>() * 10.0).round() / 10.0,
        models,
    }
}

/// Durée du même plan dans un autre mode : le mode fixe le nombre de variantes de décor et le
/// préréglage d'export (paramètres du LLM ignorés pour ces deux réglages).
fn totals_for_mode(raw: &RawPlan, ctx: &Context, mode: Mode) -> f64 {
    let mut r = raw.clone();
    for s in r.steps.iter_mut() {
        if let Some(o) = s.params.as_object_mut() {
            if s.op == "generate_background" { o.remove("variants"); }
            if s.op == "export" { o.remove("quality"); }
        }
    }
    totals(&build_steps(&r, ctx, mode).0).seconds
}

/// Recalcule les totaux après que l'utilisateur a activé/désactivé ou modifié des étapes.
pub fn revalidate(mut plan: Plan, ctx: &Context) -> Plan {
    let enabled: Vec<bool> = plan.steps.iter().map(|s| s.enabled).collect();
    let raw = RawPlan {
        summary: plan.summary.clone(),
        steps: plan.steps.iter().map(|s| RawStep { op: s.op.clone(), params: s.params.clone(), why: s.why.clone() }).collect(),
        warnings: plan.warnings.clone(),
    };
    let mut fresh = validate(&raw, ctx, plan.mode);
    for (s, on) in fresh.steps.iter_mut().zip(enabled) {
        s.enabled = on;
    }
    fresh.totals = totals(&fresh.steps);
    fresh.rejected.append(&mut plan.rejected);
    fresh
}

/// Description des opérations pour le prompt système du LLM.
pub fn catalogue() -> &'static str {
    r##"Opérations autorisées (champ "op") et leurs paramètres ("params") :
- set_clip {start: secondes ≥ 0, duration: secondes 0,2–300} : choisir l'extrait à traiter.
- segment_subject {cleanup: booléen} : détourer et suivre le sujet sur tout l'extrait (SAM 2). Le sujet doit avoir été désigné par l'utilisateur (clic) ; sinon l'application le lui demandera.
- generate_background {prompt: description du décor en français, variants: 1–4, empty_scene: booléen} : générer un décor fixe (FLUX.2, local). Décrire seulement le lieu, la lumière et l'ambiance, sans personnage.
- use_background {id: identifiant d'un décor existant de la bibliothèque}
- solid_background {color: "#rrggbb"}
- framing {preset: "center" | "ground" (en pied, au sol) | "third-left" | "third-right" | "closeup" (plan serré)}
- lighting {preset: "neutral" | "golden" (heure dorée) | "overcast" (ciel couvert) | "studio" | "night" | "dramatic"}
- adjust {brightness −0,5–0,5, contrast 0,5–2, saturation 0–2, temperature −0,5–0,5, bg_blur 0–30, edge −0,5–0,5, feather 0–20, subject_scale 0,1–4, subject_x −1–1, subject_y −1–1} : réglages fins du sujet et du décor (seulement les clés utiles).
- harmonize {} : accorder automatiquement la lumière et les couleurs du sujet au décor.
- export {quality: "fast" | "balanced" | "high"} : produire le MP4 final.
- note {text} : remarque pour l'utilisateur, sans action.
Limites de l'application : elle remplace le décor derrière un sujet filmé. Elle ne modifie pas le visage, les vêtements ni le mouvement, ne génère pas de vidéo complète, n'ajoute ni musique ni texte, et ne supprime ni n'envoie aucun fichier. Si la demande sort de ces limites, ne l'invente pas : explique-le dans "warnings"."##
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> Context {
        Context { clip_start: 0.0, frames: 125.0, fps: 25.0, duration: 5.0, source_duration: 13.0, out_pixels: 1280.0 * 720.0,
                  has_proxy: true, has_prompts: true, mask_ready: false, background_ids: vec!["gen_1".into()],
                  export_quality: "balanced".into() }
    }

    fn raw(steps: Value) -> RawPlan {
        serde_json::from_value(json!({"summary": "s", "steps": steps, "warnings": []})).unwrap()
    }

    #[test]
    fn whitelist_bounds_and_order() {
        let r = raw(json!([
            {"op": "export", "params": {}},
            {"op": "rm -rf", "params": {"path": "/"}},
            {"op": "lighting", "params": {"preset": "golden"}},
            {"op": "adjust", "params": {"brightness": 3.0}},
            {"op": "segment_subject", "params": {}},
            {"op": "generate_background", "params": {"prompt": "un lac", "variants": 9}},
            {"op": "use_background", "params": {"id": "../../etc"}},
            {"op": "framing", "params": {"preset": "dutch-angle"}},
        ]));
        let p = validate(&r, &ctx(), Mode::Balanced);
        let ops: Vec<&str> = p.steps.iter().map(|s| s.op.as_str()).collect();
        assert_eq!(ops, ["segment_subject", "lighting", "export"]);
        assert_eq!(p.rejected.len(), 5, "{:?}", p.rejected);
        assert!(p.rejected.iter().any(|r| r.reason.contains("non autorisée")));
        assert!(p.rejected.iter().any(|r| r.reason.contains("variantes")));
        assert!(p.rejected.iter().any(|r| r.reason.contains("absent de la bibliothèque")));
        assert!(p.steps.iter().all(|s| s.blocked.is_none()), "le détourage du plan rend l'export possible");
    }

    #[test]
    fn prerequisites_and_modes() {
        // Sans masque : le détourage est ajouté par l'application, visiblement.
        let r = raw(json!([{"op": "framing", "params": {"preset": "center"}}, {"op": "export", "params": {}}]));
        let p = validate(&r, &ctx(), Mode::Balanced);
        assert_eq!(p.steps[0].op, "segment_subject");
        assert!(p.steps[0].why.starts_with("Ajouté par l'application"));
        assert!(p.steps.iter().all(|s| s.blocked.is_none()));
        // Sans extrait préparé : le détourage est bloqué.
        let mut c = ctx();
        c.has_proxy = false;
        let p = validate(&r, &c, Mode::Balanced);
        assert!(p.steps[0].blocked.is_some());
        // Extrait identique à l'actuel : retiré (sinon il effacerait la sélection du sujet).
        let r = raw(json!([{"op": "set_clip", "params": {"start": 0, "duration": 5}}, {"op": "lighting", "params": {"preset": "night"}}]));
        let p = validate(&r, &ctx(), Mode::Balanced);
        assert_eq!(p.steps.iter().map(|s| s.op.as_str()).collect::<Vec<_>>(), ["lighting"]);
        // Changer d'extrait efface les images clés : le détourage demandera une sélection.
        let r = raw(json!([{"op": "set_clip", "params": {"start": 2, "duration": 4}}, {"op": "segment_subject", "params": {}},
                           {"op": "generate_background", "params": {"prompt": "plage"}}, {"op": "export", "params": {}}]));
        let p = validate(&r, &ctx(), Mode::Fast);
        assert!(p.steps[1].needs_user.is_some());
        assert_eq!(p.steps[2].params["variants"], 1, "mode rapide : une variante");
        assert_eq!(p.steps[3].params["quality"], "fast");
        let t = |mode: Mode| p.alternatives.iter().find(|(m, _)| *m == mode).unwrap().1;
        assert!(t(Mode::Fast) < t(Mode::Balanced) && t(Mode::Balanced) < t(Mode::Quality),
                "coûts croissants : {:?}", p.alternatives);
        // Les alternatives incluent le détourage ajouté automatiquement (régression corrigée).
        let r2 = raw(json!([{"op": "generate_background", "params": {"prompt": "studio", "variants": 3}},
                            {"op": "export", "params": {"quality": "balanced"}}]));
        let p2 = validate(&r2, &ctx(), Mode::Balanced);
        let seg = p2.steps.iter().find(|s| s.op == "segment_subject").unwrap().estimate.seconds;
        let q = p2.alternatives.iter().find(|(m, _)| *m == Mode::Quality).unwrap().1;
        assert!(q >= p2.totals.seconds - 0.1 && q > seg, "{:?} vs {}", p2.alternatives, p2.totals.seconds);
        assert_eq!(p.totals.peak_gb, cost::FLUX_GB, "pic = max des étapes, pas la somme");
        // Durée hors source refusée.
        let r = raw(json!([{"op": "set_clip", "params": {"start": 12, "duration": 30}}]));
        assert_eq!(validate(&r, &ctx(), Mode::Balanced).rejected.len(), 1);
    }

    #[test]
    fn adjust_merges_and_revalidate_keeps_toggles() {
        let r = raw(json!([{"op": "segment_subject"}, {"op": "adjust", "params": {"brightness": 0.1}},
                           {"op": "adjust", "params": {"contrast": 1.2}}, {"op": "generate_background", "params": {"prompt": "forêt"}}]));
        let mut p = validate(&r, &ctx(), Mode::Balanced);
        let adj = p.steps.iter().find(|s| s.op == "adjust").unwrap();
        assert_eq!(adj.params, json!({"brightness": 0.1, "contrast": 1.2}));
        let before = p.totals.seconds;
        p.steps.iter_mut().find(|s| s.op == "generate_background").unwrap().enabled = false;
        let p2 = revalidate(p, &ctx());
        assert!(p2.totals.seconds < before);
        assert!(!p2.steps.iter().find(|s| s.op == "generate_background").unwrap().enabled);
    }
}
