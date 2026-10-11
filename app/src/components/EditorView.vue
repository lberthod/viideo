<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { open } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import type { UnlistenFn } from "@tauri-apps/api/event";
import {
  api, errorText, fileUrl, formatTime, frameName, onExportProgress, onMaskProgress,
  type Composite, type ExportRecord, type Prompt, type ProjectView,
} from "../api";
import FrameViewer, { type Tool, type ViewMode } from "./FrameViewer.vue";

const props = defineProps<{ initial: ProjectView }>();
const emit = defineEmits<{ (e: "close"): void }>();

type Step = "clip" | "subject" | "background" | "export";
const pv = ref<ProjectView>(props.initial);
const step = ref<Step>(props.initial.manifest.proxy ? "subject" : "clip");
const frame = ref(0);
const mode = ref<ViewMode>("mask");
const tool = ref<Tool>("positive");
const error = ref<string | null>(null);
const job = ref<null | { label: string; done: number; total: number; cancellable: boolean }>(null);
const previews = ref<Record<number, string>>({});
const lastExport = ref<ExportRecord | null>(null);
const playing = ref(false);

const m = computed(() => pv.value.manifest);
const probe = computed(() => m.value.source.probe);
const proxy = computed(() => m.value.proxy);
const fps = computed(() => (proxy.value ? proxy.value.fps_num / proxy.value.fps_den : probe.value.fps_num / probe.value.fps_den));
const frames = computed(() => proxy.value?.frames ?? 0);
const status = computed(() => m.value.segmentation.status);
const prompts = computed(() => m.value.segmentation.prompts);
const currentPrompt = computed<Prompt | null>(() => prompts.value.find((p) => p.frame === frame.value) ?? null);
const projectName = computed(() => pv.value.dir.split("/").pop());

// Réglages de l'extrait (saisis, appliqués par « Préparer »).
const clipStart = ref(m.value.clip.start);
const clipDuration = ref(m.value.clip.duration);
const composite = ref<Composite>({ ...m.value.composite });

const frameUrl = computed(() => (pv.value.proxy_dir && frames.value ? fileUrl(`${pv.value.proxy_dir}/${frameName(frame.value)}.jpg`) : null));
const maskUrl = computed(() => {
  if (pv.value.mask_dir) return fileUrl(`${pv.value.mask_dir}/${frameName(frame.value)}.png`);
  const p = previews.value[frame.value];
  return p ? fileUrl(p) : null;
});
const backgroundUrl = computed(() => (pv.value.background_path ? fileUrl(pv.value.background_path) : null));

function apply(v: ProjectView) {
  pv.value = v;
  if (frame.value >= frames.value) frame.value = Math.max(0, frames.value - 1);
}

async function run<T>(fn: () => Promise<T>): Promise<T | undefined> {
  error.value = null;
  try { return await fn(); } catch (e) { error.value = errorText(e); return undefined; }
}

// --- Extrait ---------------------------------------------------------------
async function prepareClip() {
  job.value = { label: "Extraction des images de l'extrait", done: 0, total: 0, cancellable: false };
  await run(async () => {
    apply(await api.setClip(clipStart.value, clipDuration.value));
    apply(await api.prepareProxy());
    previews.value = {};
    frame.value = 0;
    step.value = "subject";
  });
  job.value = null;
}

// --- Sujet -------------------------------------------------------------------
async function savePrompts(next: Prompt[]) {
  const r = await run(() => api.setPrompts(next));
  if (r) apply(r);
}

function editCurrent(edit: (p: Prompt) => void) {
  const next = prompts.value.map((p) => ({ ...p, points: [...p.points] }));
  let cur = next.find((p) => p.frame === frame.value);
  if (!cur) { cur = { frame: frame.value, points: [], box: null }; next.push(cur); }
  edit(cur);
  next.sort((a, b) => a.frame - b.frame);
  return savePrompts(next.filter((p) => p.points.length || p.box));
}

async function onPoint(x: number, y: number, positive: boolean) {
  await editCurrent((p) => p.points.push({ x, y, positive }));
  await preview();
}

async function onBox(box: [number, number, number, number]) {
  await editCurrent((p) => { p.box = box; });
  await preview();
}

async function clearCurrent() {
  delete previews.value[frame.value];
  await savePrompts(prompts.value.filter((p) => p.frame !== frame.value));
}

async function undoPoint() {
  await editCurrent((p) => { if (p.points.length) p.points.pop(); else p.box = null; });
  if (currentPrompt.value) await preview();
  else delete previews.value[frame.value];
}

async function preview() {
  if (!currentPrompt.value) return;
  mode.value = "mask";
  job.value = { label: "Aperçu du masque (chargement du modèle au premier appel)", done: 0, total: 0, cancellable: false };
  const r = await run(() => api.previewMask(frame.value));
  if (r) previews.value = { ...previews.value, [r.frame]: r.png };
  job.value = null;
}

async function propagate() {
  job.value = { label: "Propagation du masque", done: 0, total: frames.value, cancellable: true };
  const r = await run(() => api.propagateMask());
  if (r) { apply(r); previews.value = {}; }
  job.value = null;
}

// --- Décor ---------------------------------------------------------------------
async function importBackground() {
  const path = await open({ title: "Choisir une image de décor", multiple: false,
    filters: [{ name: "Image", extensions: ["png", "jpg", "jpeg", "webp", "PNG", "JPG", "JPEG"] }] });
  if (!path || Array.isArray(path)) return;
  const r = await run(() => api.setBackground(path));
  if (r) { apply(r); composite.value.background = r.manifest.composite.background; }
  mode.value = "composite";
}

async function removeBackground() {
  const r = await run(() => api.setBackground(null));
  if (r) { apply(r); composite.value.background = null; }
}

let compositeTimer: number | undefined;
watch(composite, (c) => {
  window.clearTimeout(compositeTimer);
  compositeTimer = window.setTimeout(async () => {
    const r = await run(() => api.setComposite({ ...c }));
    if (r) pv.value = r;
  }, 250);
}, { deep: true });

function resetLook() {
  composite.value = { ...composite.value, subject_scale: 1, subject_x: 0, subject_y: 0, feather: 1.5, brightness: 0, contrast: 1, saturation: 1 };
}

// --- Export ----------------------------------------------------------------------
async function exportVideo() {
  job.value = { label: "Export MP4", done: 0, total: 100, cancellable: true };
  const r = await run(() => api.exportVideo());
  if (r) {
    lastExport.value = r;
    const fresh = await run(() => api.openProject(pv.value.dir));
    if (fresh) pv.value = fresh;
  }
  job.value = null;
}

async function reveal(rel: string) {
  await run(async () => revealItemInDir(await api.projectPath(rel)));
}

async function cancel() {
  await run(() => api.cancelJob());
}

// --- Lecture ---------------------------------------------------------------------
let playTimer: number | undefined;
function togglePlay() {
  playing.value = !playing.value;
  window.clearInterval(playTimer);
  if (playing.value) {
    playTimer = window.setInterval(() => { frame.value = (frame.value + 1) % Math.max(frames.value, 1); }, 1000 / fps.value);
  }
}

function onKey(ev: KeyboardEvent) {
  if ((ev.target as HTMLElement)?.tagName === "INPUT") return;
  if (ev.key === "ArrowRight") frame.value = Math.min(frame.value + 1, frames.value - 1);
  else if (ev.key === "ArrowLeft") frame.value = Math.max(frame.value - 1, 0);
  else if (ev.key === " ") { ev.preventDefault(); togglePlay(); }
  else if ((ev.metaKey || ev.ctrlKey) && ev.key === "z") { ev.preventDefault(); undoPoint(); }
}

// --- Démonstration automatique (dev uniquement, VIDEEO_DEV_DEMO=1) -----------------
const pause = (ms: number) => new Promise((r) => setTimeout(r, ms));
async function runDemo(background: string) {
  const mark = async (s: string) => { await api.devLog(`DEMO ${s}`); await pause(2500); };
  step.value = "clip"; clipDuration.value = 3; await prepareClip(); await mark("1-proxy");
  tool.value = "box"; await onBox([420, 0, 915, 719]); await onPoint(646, 140, true); await mark("2-preview");
  frame.value = 60; await onPoint(640, 450, true); await mark("3-keyframe");
  await propagate(); frame.value = 30; await mark("4-propagated");
  step.value = "background"; await api.setBackground(background).then(apply);
  composite.value = { ...composite.value, background: m.value.composite.background, subject_scale: 0.85, subject_y: 0.08, brightness: 0.03 };
  mode.value = "composite"; await mark("5-composite");
  step.value = "export"; await exportVideo(); await mark("6-exported");
  if (error.value) await api.devLog(`DEMO error ${error.value}`);
}

const unlisten: UnlistenFn[] = [];
onMounted(async () => {
  window.addEventListener("keydown", onKey);
  unlisten.push(await onMaskProgress((done, total) => { if (job.value) job.value = { ...job.value, done, total }; }));
  unlisten.push(await onExportProgress((f) => { if (job.value) job.value = { ...job.value, done: Math.round(f * 100), total: 100 }; }));
  const demo = await api.devDemo();
  if (demo) runDemo(demo).catch((e) => api.devLog(`DEMO error ${errorText(e)}`));
});
onBeforeUnmount(() => {
  window.removeEventListener("keydown", onKey);
  window.clearInterval(playTimer);
  unlisten.forEach((u) => u());
});

const statusLabel = computed(() => ({ none: "aucun masque", stale: "à recalculer", ready: "prêt" })[status.value]);
const outSize = computed(() => {
  const w = probe.value.display_width, h = probe.value.display_height;
  const k = Math.max(w, h) > 3840 ? 3840 / Math.max(w, h) : 1;
  return `${Math.round((w * k) / 2) * 2}×${Math.round((h * k) / 2) * 2}`;
});
const steps: { id: Step; label: string }[] = [
  { id: "clip", label: "Extrait" }, { id: "subject", label: "Sujet" },
  { id: "background", label: "Décor" }, { id: "export", label: "Export" },
];
function stepState(id: Step) {
  if (id === "clip") return proxy.value ? "ok" : "none";
  if (id === "subject") return status.value === "ready" ? "ok" : status.value === "stale" ? "warn" : "none";
  if (id === "background") return m.value.composite.background ? "ok" : "none";
  return m.value.exports.length ? "ok" : "none";
}
</script>

<template>
  <div class="editor">
    <header class="top">
      <button @click="emit('close')" title="Fermer le projet">←</button>
      <div class="title">
        <strong>{{ projectName }}</strong>
        <span class="muted mono">{{ m.source.path.split("/").pop() }} · {{ probe.display_width }}×{{ probe.display_height }}
          · {{ fps.toFixed(2) }} i/s · {{ formatTime(probe.duration) }}{{ probe.has_audio ? " · audio" : "" }}{{ probe.rotation ? ` · rotation ${probe.rotation}°` : "" }}</span>
      </div>
      <nav class="steps">
        <button v-for="s in steps" :key="s.id" :class="{ active: step === s.id }" :disabled="s.id !== 'clip' && !proxy" @click="step = s.id">
          <i class="dot" :class="stepState(s.id)"></i>{{ s.label }}
        </button>
      </nav>
    </header>

    <p v-if="pv.source_missing" class="error-banner">La vidéo source est introuvable : {{ m.source.path }}. Les éléments déjà calculés restent consultables.</p>
    <p v-if="error" class="error-banner">{{ error }}</p>

    <div class="body">
      <section class="stage">
        <div class="modes">
          <button :class="{ active: mode === 'source' }" @click="mode = 'source'">Source</button>
          <button :class="{ active: mode === 'mask' }" @click="mode = 'mask'">Masque</button>
          <button :class="{ active: mode === 'composite' }" :disabled="!pv.mask_dir" @click="mode = 'composite'">Composite</button>
        </div>
        <FrameViewer
          :frame-url="frameUrl" :mask-url="maskUrl" :background-url="backgroundUrl" :mode="mode"
          :composite="composite" :prompt="currentPrompt" :tool="step === 'subject' ? tool : null"
          :width="proxy?.width ?? 1280" :height="proxy?.height ?? 720"
          @point="onPoint" @box="onBox"
        />
        <div v-if="frames" class="timeline">
          <button @click="togglePlay" :title="playing ? 'Pause (espace)' : 'Lecture (espace)'">{{ playing ? "❚❚" : "▶" }}</button>
          <div class="track">
            <input id="frame" type="range" min="0" :max="frames - 1" v-model.number="frame" aria-label="Image courante" />
            <span v-for="p in prompts" :key="p.frame" class="kf" :style="{ left: `${(p.frame / Math.max(frames - 1, 1)) * 100}%` }" :title="`Image clé ${p.frame}`" @click="frame = p.frame"></span>
          </div>
          <span class="mono muted">{{ frame }} / {{ frames - 1 }} · {{ formatTime(m.clip.start + frame / fps) }}</span>
        </div>
      </section>

      <aside class="panel">
        <!-- Extrait -->
        <template v-if="step === 'clip'">
          <h2>Extrait à traiter</h2>
          <p class="muted">Commencez court : quelques secondes suffisent pour régler le détourage. Le proxy est extrait en 720p à cadence constante.</p>
          <label>Début (s)<input id="clip-start" type="number" min="0" :max="probe.duration" step="0.1" v-model.number="clipStart" /></label>
          <label>Durée (s)<input id="clip-duration" type="number" min="0.2" max="120" step="0.5" v-model.number="clipDuration" /></label>
          <p class="mono muted">≈ {{ Math.round(Math.min(clipDuration, probe.duration - clipStart) * fps) }} images · source {{ formatTime(probe.duration) }}</p>
          <button class="primary" :disabled="!!job" @click="prepareClip">{{ proxy ? "Mettre à jour l'extrait" : "Préparer l'extrait" }}</button>
          <p v-if="proxy" class="muted">Proxy prêt : {{ proxy.frames }} images {{ proxy.width }}×{{ proxy.height }}. Changer l'extrait efface les images clés.</p>
        </template>

        <!-- Sujet -->
        <template v-else-if="step === 'subject'">
          <h2>Sujet à conserver <span class="chip" :class="status === 'ready' ? 'ok' : status === 'stale' ? 'warn' : 'none'">{{ statusLabel }}</span></h2>
          <p class="muted">Cliquez sur le sujet (point vert), excluez une zone (point rouge, ou Alt+clic), ou tracez un rectangle. Corrigez sur n'importe quelle image, puis propagez.</p>
          <div class="tools">
            <button :class="{ active: tool === 'positive' }" @click="tool = 'positive'">+ Inclure</button>
            <button :class="{ active: tool === 'negative' }" @click="tool = 'negative'">− Exclure</button>
            <button :class="{ active: tool === 'box' }" @click="tool = 'box'">▭ Rectangle</button>
          </div>
          <div class="row">
            <button :disabled="!currentPrompt || !!job" @click="undoPoint">Annuler le dernier (⌘Z)</button>
            <button :disabled="!currentPrompt || !!job" @click="clearCurrent">Effacer cette image</button>
          </div>
          <div class="keyframes">
            <span class="muted">Images clés :</span>
            <button v-for="p in prompts" :key="p.frame" class="mono" :class="{ active: p.frame === frame }" @click="frame = p.frame">
              {{ p.frame }} <small class="muted">{{ p.points.length }} pt{{ p.box ? " + ▭" : "" }}</small>
            </button>
            <span v-if="!prompts.length" class="muted">aucune</span>
          </div>
          <button class="primary" :disabled="!prompts.length || !!job" @click="propagate">Propager sur les {{ frames }} images</button>
          <dl v-if="m.segmentation.mask && status === 'ready'" class="stats">
            <dt>Calcul</dt><dd class="mono">{{ m.segmentation.mask.seconds.toFixed(1) }} s ({{ (m.segmentation.mask.frames / Math.max(m.segmentation.mask.seconds, 0.01)).toFixed(1) }} i/s)</dd>
            <dt>Images vides</dt><dd class="mono">{{ m.segmentation.mask.empty_frames }}</dd>
            <dt>Modèle</dt><dd class="mono">SAM 2.1 {{ m.segmentation.model }} · {{ m.segmentation.dtype }}</dd>
          </dl>
          <p class="muted small">Parcourez la timeline : si le masque déborde ou perd le sujet, ajoutez des points sur cette image et propagez à nouveau.</p>
        </template>

        <!-- Décor -->
        <template v-else-if="step === 'background'">
          <h2>Décor et intégration</h2>
          <div class="row">
            <button class="primary" @click="importBackground">Importer une image…</button>
            <button v-if="m.composite.background" @click="removeBackground">Retirer</button>
          </div>
          <label v-if="!m.composite.background">Couleur de fond<input id="bg-color" type="text" v-model="composite.background_color" /></label>
          <fieldset>
            <legend>Sujet</legend>
            <label>Échelle <span class="mono">{{ composite.subject_scale.toFixed(2) }}</span><input id="scale" type="range" min="0.2" max="2" step="0.01" v-model.number="composite.subject_scale" /></label>
            <label>Position X <span class="mono">{{ composite.subject_x.toFixed(2) }}</span><input id="pos-x" type="range" min="-0.8" max="0.8" step="0.01" v-model.number="composite.subject_x" /></label>
            <label>Position Y <span class="mono">{{ composite.subject_y.toFixed(2) }}</span><input id="pos-y" type="range" min="-0.8" max="0.8" step="0.01" v-model.number="composite.subject_y" /></label>
            <label>Adoucissement du bord <span class="mono">{{ composite.feather.toFixed(1) }} px</span><input id="feather" type="range" min="0" max="8" step="0.5" v-model.number="composite.feather" /></label>
          </fieldset>
          <fieldset>
            <legend>Couleur du sujet</legend>
            <label>Luminosité <span class="mono">{{ composite.brightness.toFixed(2) }}</span><input id="brightness" type="range" min="-0.3" max="0.3" step="0.01" v-model.number="composite.brightness" /></label>
            <label>Contraste <span class="mono">{{ composite.contrast.toFixed(2) }}</span><input id="contrast" type="range" min="0.6" max="1.6" step="0.01" v-model.number="composite.contrast" /></label>
            <label>Saturation <span class="mono">{{ composite.saturation.toFixed(2) }}</span><input id="saturation" type="range" min="0" max="2" step="0.01" v-model.number="composite.saturation" /></label>
          </fieldset>
          <button @click="resetLook">Réinitialiser les réglages</button>
        </template>

        <!-- Export -->
        <template v-else>
          <h2>Export MP4</h2>
          <dl class="stats">
            <dt>Sortie</dt><dd class="mono">{{ outSize }} · {{ fps.toFixed(2) }} i/s · H.264</dd>
            <dt>Durée</dt><dd class="mono">{{ m.clip.duration.toFixed(2) }} s depuis {{ formatTime(m.clip.start) }}</dd>
            <dt>Audio</dt><dd class="mono">{{ probe.has_audio ? "conservé (AAC)" : "aucun dans la source" }}</dd>
            <dt>Masque</dt><dd><span class="chip" :class="status === 'ready' ? 'ok' : 'warn'">{{ statusLabel }}</span></dd>
          </dl>
          <p class="muted">Le rendu repart de la vidéo source à pleine résolution ; le masque calculé sur le proxy est mis à l'échelle.</p>
          <button class="primary" :disabled="status !== 'ready' || !!job || pv.source_missing" @click="exportVideo">Exporter</button>
          <div v-if="lastExport" class="done">Export terminé en {{ lastExport.seconds.toFixed(1) }} s ({{ (lastExport.size_bytes / 1e6).toFixed(1) }} Mo).</div>
          <h3 v-if="m.exports.length">Exports du projet</h3>
          <ul class="exports">
            <li v-for="e in [...m.exports].reverse()" :key="e.path">
              <span class="mono">{{ e.path.replace("exports/", "") }}</span>
              <span class="muted mono">{{ e.width }}×{{ e.height }} · {{ (e.size_bytes / 1e6).toFixed(1) }} Mo</span>
              <button @click="reveal(e.path)">Afficher dans le Finder</button>
            </li>
          </ul>
        </template>

        <div v-if="job" class="job">
          <div class="job-head"><span>{{ job.label }}</span><span v-if="job.total" class="mono">{{ Math.round((job.done / job.total) * 100) }} %</span></div>
          <progress :value="job.total ? job.done : undefined" :max="job.total || 1"></progress>
          <button v-if="job.cancellable" @click="cancel">Annuler</button>
        </div>
      </aside>
    </div>
  </div>
</template>

<style scoped>
.editor { height: 100%; display: flex; flex-direction: column; }
.top { display: flex; align-items: center; gap: 14px; padding: 10px 14px; border-bottom: 1px solid var(--line); background: var(--panel); }
.title { display: grid; min-width: 0; flex: 1; }
.title span { font-size: 11px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.steps { display: flex; gap: 6px; }
.dot { display: inline-block; width: 7px; height: 7px; border-radius: 50%; margin-right: 7px; background: var(--line); vertical-align: 1px; }
.dot.ok { background: var(--ok); } .dot.warn { background: var(--warn); }
.error-banner { margin: 8px 14px 0; }
.body { flex: 1; display: grid; grid-template-columns: minmax(0, 1fr) 320px; min-height: 0; }
.stage { display: grid; grid-template-rows: auto minmax(0, 1fr) auto; gap: 10px; padding: 12px 14px; min-height: 0; }
.modes { display: flex; gap: 6px; }
.timeline { display: grid; grid-template-columns: auto 1fr auto; align-items: center; gap: 12px; }
.track { position: relative; }
.kf { position: absolute; top: -6px; width: 8px; height: 8px; margin-left: -4px; background: #ffd23f; transform: rotate(45deg); cursor: pointer; }
.panel { border-left: 1px solid var(--line); background: var(--panel); padding: 16px; display: flex; flex-direction: column; gap: 12px; overflow-y: auto; }
.panel h2 { font-size: 15px; display: flex; gap: 8px; align-items: center; }
.panel h3 { font-size: 12px; text-transform: uppercase; letter-spacing: 0.06em; color: var(--muted); margin-top: 8px; }
.panel p { margin: 0; }
label { display: grid; gap: 5px; font-size: 12px; color: var(--muted); }
label span { color: var(--fg); }
fieldset { border: 1px solid var(--line); border-radius: 6px; padding: 10px 12px; display: grid; gap: 10px; margin: 0; }
legend { font-size: 11px; text-transform: uppercase; letter-spacing: 0.06em; color: var(--muted); padding: 0 4px; }
.tools, .row { display: flex; gap: 6px; flex-wrap: wrap; }
.keyframes { display: flex; gap: 6px; flex-wrap: wrap; align-items: center; }
.keyframes button { padding: 3px 8px; }
.stats { display: grid; grid-template-columns: auto 1fr; gap: 4px 12px; margin: 0; }
.stats dt { color: var(--muted); } .stats dd { margin: 0; }
.small { font-size: 12px; }
.job { margin-top: auto; border-top: 1px solid var(--line); padding-top: 12px; display: grid; gap: 8px; }
.job-head { display: flex; justify-content: space-between; }
progress { width: 100%; accent-color: var(--accent); }
.done { color: var(--ok); }
.exports { list-style: none; padding: 0; margin: 0; display: grid; gap: 10px; }
.exports li { display: grid; gap: 4px; padding-bottom: 10px; border-bottom: 1px solid var(--line); }
.exports button { justify-self: start; }
</style>
