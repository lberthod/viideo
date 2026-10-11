<script setup lang="ts">
// Assistant : une demande en langage naturel devient un plan d'opérations de la liste blanche,
// validé et chiffré par Rust, modifiable, puis exécuté étape par étape par l'éditeur.
import { computed, onMounted, ref } from "vue";
import { api, errorText, type Plan, type PlanMode, type PlanStep, type PlannerStatus } from "../api";

export type StepState = "pending" | "running" | "done" | "skipped" | "waiting" | "error";

const props = defineProps<{ busy: boolean; run: { plan: Plan; states: StepState[]; index: number; waiting: string | null } | null }>();
const emit = defineEmits<{ (e: "execute", plan: Plan): void; (e: "resume"): void; (e: "stop"): void; (e: "error", msg: string | null): void }>();

const status = ref<PlannerStatus | null>(null);
const instruction = ref("");
const mode = ref<PlanMode>("balanced");
const plan = ref<Plan | null>(null);
const meta = ref<{ model: string; seconds: number; attempts: number } | null>(null);
const thinking = ref(false);

const MODES: { id: PlanMode; label: string }[] = [
  { id: "fast", label: "Rapide" }, { id: "balanced", label: "Équilibré" }, { id: "quality", label: "Qualité" },
];
const EXAMPLES = [
  "Mets-moi sur une plage au coucher du soleil.",
  "Place-moi à gauche, dans un bureau moderne et lumineux, avec l'arrière-plan un peu flou.",
  "Ambiance nuit en ville, plan serré, la meilleure qualité possible.",
];
const LABELS: Record<string, string> = {
  set_clip: "Choisir l'extrait", segment_subject: "Détourer le sujet", generate_background: "Générer le décor",
  use_background: "Utiliser un décor existant", solid_background: "Fond uni", framing: "Cadrage", lighting: "Lumière",
  adjust: "Réglages", harmonize: "Harmoniser avec le décor", export: "Exporter le MP4", note: "Remarque",
};
const PARAM_LABELS: Record<string, string> = {
  brightness: "luminosité", contrast: "contraste", saturation: "saturation", temperature: "température", bg_blur: "flou du décor",
  edge: "bord", feather: "adoucissement", subject_scale: "échelle", subject_x: "position X", subject_y: "position Y",
};
const FRAMINGS = { center: "Centré", ground: "En pied, au sol", "third-left": "Tiers gauche", "third-right": "Tiers droit", closeup: "Plan serré" };
const LIGHTINGS = { neutral: "Neutre", golden: "Heure dorée", overcast: "Ciel couvert", studio: "Studio doux", night: "Nuit", dramatic: "Contrasté" };
const QUALITIES = { fast: "Rapide (VideoToolbox)", balanced: "Équilibré", high: "Qualité" };

const running = computed(() => !!props.run);
const shown = computed(() => props.run?.plan ?? plan.value);
const executable = computed(() => !!plan.value?.steps.some((s) => s.enabled && !s.blocked && s.op !== "note"));

function fmtTime(s: number) {
  if (s < 60) return `${Math.max(1, Math.round(s))} s`;
  return `${Math.floor(s / 60)} min ${String(Math.round(s % 60)).padStart(2, "0")}`;
}

async function refreshStatus() {
  try { status.value = await api.plannerStatus(); } catch (e) { emit("error", errorText(e)); }
}

async function setModel(m: string) {
  try { await api.setPlannerModel(m); await refreshStatus(); } catch (e) { emit("error", errorText(e)); }
}

async function propose() {
  emit("error", null);
  thinking.value = true;
  try {
    const r = await api.proposePlan(instruction.value, mode.value);
    plan.value = r.plan;
    meta.value = { model: r.model, seconds: r.seconds, attempts: r.attempts };
  } catch (e) { emit("error", errorText(e)); }
  thinking.value = false;
}

async function revalidate(modeChanged = false) {
  if (!plan.value) return;
  try { plan.value = await api.revalidatePlan(plan.value, modeChanged); } catch (e) { emit("error", errorText(e)); }
}

async function switchMode(m: PlanMode) {
  mode.value = m;
  if (plan.value) { plan.value.mode = m; await revalidate(true); }
}

function edit(step: PlanStep, key: string, value: unknown) {
  step.params = { ...step.params, [key]: value };
  revalidate();
}

function stateOf(i: number): StepState | null {
  return props.run ? props.run.states[i] : null;
}

onMounted(refreshStatus);

/** Démonstration (dev) : affiche un plan déjà obtenu, comme s'il venait d'être proposé. */
function setPlan(r: { plan: Plan; model: string; seconds: number; attempts: number }, text: string) {
  instruction.value = text;
  mode.value = r.plan.mode;
  plan.value = r.plan;
  meta.value = { model: r.model, seconds: r.seconds, attempts: r.attempts };
}
defineExpose({ setPlan });
</script>

<template>
  <section class="assistant">
    <h2>Assistant</h2>
    <p class="muted">Décrivez le résultat voulu. Un modèle de langage local propose un plan ; l'application le vérifie, en estime le coût, et n'exécute rien avant votre accord.</p>

    <div v-if="status" class="llm">
      <span class="chip" :class="status.llm.reachable && status.llm.models.length ? 'ok' : 'warn'">{{ status.llm.reachable ? `Ollama ${status.llm.version}` : "Ollama absent" }}</span>
      <select v-if="status.llm.models.length" id="planner-model" :value="status.model" :disabled="running" @change="setModel(($event.target as HTMLSelectElement).value)">
        <option v-for="m in status.llm.models" :key="m" :value="m">{{ m }}</option>
      </select>
      <span v-else class="muted small">{{ status.llm.detail }}</span>
    </div>

    <label>Votre demande
      <textarea id="instruction" v-model="instruction" rows="3" maxlength="2000" :disabled="running"
        placeholder="Ex. : mets-moi sur une terrasse face à la mer, en fin de journée"></textarea>
    </label>
    <div class="examples">
      <button v-for="e in EXAMPLES" :key="e" class="chipbtn" :disabled="running" @click="instruction = e">{{ e }}</button>
    </div>
    <div class="modes" role="radiogroup" aria-label="Mode">
      <button v-for="m in MODES" :key="m.id" role="radio" :aria-checked="mode === m.id" :class="{ active: mode === m.id }" :disabled="running" @click="switchMode(m.id)">
        {{ m.label }}<small v-if="plan" class="mono"> · {{ fmtTime(plan.alternatives.find((a) => a[0] === m.id)?.[1] ?? 0) }}</small>
      </button>
    </div>
    <button class="primary" :disabled="!instruction.trim() || thinking || running || busy || !status?.llm.models.length" @click="propose">
      {{ thinking ? "Réflexion…" : plan ? "Proposer un nouveau plan" : "Proposer un plan" }}
    </button>

    <div v-if="shown" class="plan">
      <p class="summary">{{ shown.summary }}</p>
      <p v-if="meta && !run" class="muted small mono">{{ meta.model }} · {{ meta.seconds }} s<template v-if="meta.attempts > 1"> · corrigé après validation</template></p>
      <p v-for="w in shown.warnings" :key="w" class="warn-note small">⚠ {{ w }}</p>

      <ol class="steps">
        <li v-for="(s, i) in shown.steps" :key="i" :class="[stateOf(i) ?? '', { off: !s.enabled || s.blocked }]">
          <div class="shead">
            <input v-if="!run" :id="`step-${i}`" type="checkbox" v-model="s.enabled" :disabled="!!s.blocked || s.op === 'note'" @change="revalidate()" />
            <span v-else class="state" :title="stateOf(i) ?? ''">{{ { pending: "○", running: "◐", done: "●", skipped: "–", waiting: "⏸", error: "✕" }[stateOf(i) ?? "pending"] }}</span>
            <strong>{{ LABELS[s.op] ?? s.op }}</strong>
            <span v-if="s.estimate.seconds" class="mono muted small">{{ fmtTime(s.estimate.seconds) }}</span>
          </div>
          <div class="params" v-if="!run">
            <template v-if="s.op === 'set_clip'">
              <label class="inline">début <input type="number" step="0.1" min="0" :value="s.params.start" @change="edit(s, 'start', Number(($event.target as HTMLInputElement).value))" /> s</label>
              <label class="inline">durée <input type="number" step="0.5" min="0.2" :value="s.params.duration" @change="edit(s, 'duration', Number(($event.target as HTMLInputElement).value))" /> s</label>
            </template>
            <template v-else-if="s.op === 'generate_background'">
              <textarea rows="2" :value="String(s.params.prompt)" @change="edit(s, 'prompt', ($event.target as HTMLTextAreaElement).value)"></textarea>
              <label class="inline">variantes
                <select :value="s.params.variants" @change="edit(s, 'variants', Number(($event.target as HTMLSelectElement).value))"><option v-for="n in 4" :key="n" :value="n">{{ n }}</option></select>
              </label>
            </template>
            <template v-else-if="s.op === 'framing'">
              <select :value="s.params.preset" @change="edit(s, 'preset', ($event.target as HTMLSelectElement).value)"><option v-for="(l, k) in FRAMINGS" :key="k" :value="k">{{ l }}</option></select>
            </template>
            <template v-else-if="s.op === 'lighting'">
              <select :value="s.params.preset" @change="edit(s, 'preset', ($event.target as HTMLSelectElement).value)"><option v-for="(l, k) in LIGHTINGS" :key="k" :value="k">{{ l }}</option></select>
            </template>
            <template v-else-if="s.op === 'export'">
              <select :value="s.params.quality" @change="edit(s, 'quality', ($event.target as HTMLSelectElement).value)"><option v-for="(l, k) in QUALITIES" :key="k" :value="k">{{ l }}</option></select>
            </template>
            <template v-else-if="s.op === 'adjust'">
              <span v-for="(v, k) in s.params" :key="k" class="mono small kv">{{ PARAM_LABELS[k] ?? k }} {{ Number(v).toFixed(2) }}</span>
            </template>
            <template v-else-if="s.op === 'solid_background'"><span class="swatch" :style="{ background: String(s.params.color) }"></span><span class="mono small">{{ s.params.color }}</span></template>
            <template v-else-if="s.op === 'use_background'"><span class="mono small">{{ s.params.id }}</span></template>
            <template v-else-if="s.op === 'note'"><span class="small">{{ s.params.text }}</span></template>
          </div>
          <p v-if="s.why" class="why muted small">{{ s.why }}</p>
          <p v-if="s.blocked" class="warn-note small">Bloquée : {{ s.blocked }}</p>
          <p v-else-if="s.needs_user" class="note small">Action requise : {{ s.needs_user }}</p>
          <p v-if="s.estimate.models.length" class="muted small mono">{{ s.estimate.models.join(", ") }} · pic ≈ {{ s.estimate.peak_gb.toFixed(1) }} Go</p>
        </li>
      </ol>

      <details v-if="shown.rejected.length" class="rejected">
        <summary class="small">{{ shown.rejected.length }} proposition(s) refusée(s) par la validation</summary>
        <ul><li v-for="(r, i) in shown.rejected" :key="i" class="small mono">{{ r.op }} : {{ r.reason }}</li></ul>
      </details>

      <dl class="totals">
        <dt>Durée estimée</dt><dd class="mono">{{ fmtTime(shown.totals.seconds) }}</dd>
        <dt>Mémoire (pic)</dt><dd class="mono">≈ {{ shown.totals.peak_gb.toFixed(1) }} Go</dd>
        <dt>Disque</dt><dd class="mono">≈ {{ shown.totals.disk_mb.toFixed(0) }} Mo</dd>
        <dt>Modèles</dt><dd class="small">{{ shown.totals.models.length ? shown.totals.models.join(", ") : "aucun" }}</dd>
      </dl>

      <div v-if="!run" class="row">
        <button class="primary" :disabled="!executable || busy" @click="emit('execute', plan!)">Exécuter le plan</button>
      </div>
      <div v-else class="row">
        <button v-if="run.waiting" class="primary" @click="emit('resume')">Reprendre le plan</button>
        <button @click="emit('stop')">Arrêter</button>
      </div>
      <p v-if="run?.waiting" class="note small">{{ run.waiting }}</p>
    </div>
  </section>
</template>

<style scoped>
.assistant { display: grid; gap: 10px; }
h2 { font-size: 15px; }
p { margin: 0; }
.llm { display: flex; gap: 8px; align-items: center; }
.llm select { width: auto; flex: 1; }
textarea { font: 12px/1.4 var(--font); color: var(--fg); background: var(--bg); border: 1px solid var(--line); border-radius: 4px; padding: 6px 8px; resize: vertical; width: 100%; user-select: text; }
label { display: grid; gap: 5px; font-size: 12px; color: var(--muted); }
.examples { display: flex; flex-direction: column; gap: 4px; }
.chipbtn { text-align: left; padding: 3px 8px; font-size: 11px; color: var(--muted); }
.modes { display: grid; grid-template-columns: repeat(3, 1fr); gap: 4px; }
.modes button { padding: 5px 4px; font-size: 12px; }
.plan { display: grid; gap: 8px; border-top: 1px solid var(--line); padding-top: 10px; }
.summary { font-weight: 600; }
.steps { list-style: none; margin: 0; padding: 0; display: grid; gap: 6px; }
.steps li { border: 1px solid var(--line); border-radius: 6px; padding: 8px 10px; display: grid; gap: 4px; }
.steps li.off { opacity: 0.5; }
.steps li.running { border-color: var(--accent); }
.steps li.done { border-color: var(--ok); }
.steps li.error { border-color: #b2404a; }
.steps li.waiting { border-color: var(--warn); }
.shead { display: flex; align-items: center; gap: 8px; }
.shead strong { flex: 1; }
.state { width: 14px; text-align: center; }
.params { display: flex; flex-wrap: wrap; gap: 6px; align-items: center; }
.params select { width: auto; }
.inline { display: flex; align-items: center; gap: 4px; }
.inline input { width: 64px; }
.kv { background: var(--panel-2); padding: 2px 6px; border-radius: 3px; }
.swatch { width: 14px; height: 14px; border-radius: 3px; border: 1px solid var(--line); }
.why { font-style: italic; }
.note { color: var(--warn); }
.warn-note { color: var(--warn); }
.totals { display: grid; grid-template-columns: auto 1fr; gap: 3px 12px; margin: 0; font-size: 12px; }
.totals dt { color: var(--muted); }
.totals dd { margin: 0; }
.rejected ul { margin: 4px 0 0; padding-left: 16px; color: var(--muted); }
.row { display: flex; gap: 6px; }
.small { font-size: 12px; }
</style>
