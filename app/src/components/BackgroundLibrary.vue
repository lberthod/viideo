<script setup lang="ts">
// Bibliothèque de décors du projet : import, génération locale (moteur intégré ou ComfyUI), choix.
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { open } from "@tauri-apps/plugin-dialog";
import type { UnlistenFn } from "@tauri-apps/api/event";
import {
  api, errorText, fileUrl, onImageProgress, onVideoProgress, PLATE_SUFFIX,
  type BackgroundItem, type ImageBackends, type ProjectView, type VideoEngines,
} from "../api";

const props = defineProps<{ pv: ProjectView; busy: boolean }>();
const emit = defineEmits<{ (e: "applied", v: ProjectView): void; (e: "error", msg: string | null): void }>();

const items = ref<BackgroundItem[]>([]);
const backends = ref<ImageBackends | null>(null);
const prompt = ref("");
const empty = ref(true);
const count = ref(2);
const seed = ref<number | null>(null);
const comfyUrl = ref(props.pv.manifest.generation.comfy_url);
const generating = ref<null | { index: number; count: number; done: number; total: number }>(null);

// --- Décor animé (expérimental) ---
const video = ref<VideoEngines | null>(null);
const animEngine = ref("sulphur2");
const animPrompt = ref("");
const animSeconds = ref(5);
const animating = ref<null | { fraction: number; started: number }>(null);
// Cinémagraphe : seules les zones désignées (eau, ciel…) reçoivent le mouvement généré ; le reste du
// décor reste strictement fixe (les moteurs ne respectent pas la caméra fixe sur 5 s, cf. étape 5).
const zoneMode = ref(true);
const zones = ref<[number, number][]>([]);
function addZone(ev: MouseEvent) {
  const r = (ev.currentTarget as HTMLElement).getBoundingClientRect();
  const x = Math.min(1, Math.max(0, (ev.clientX - r.left) / r.width));
  const y = Math.min(1, Math.max(0, (ev.clientY - r.top) / r.height));
  if (zones.value.length < 8) zones.value = [...zones.value, [Math.round(x * 1000) / 1000, Math.round(y * 1000) / 1000]];
}
const currentIsImage = computed(() => !!currentItem.value && currentItem.value.kind === "image");
const engine = computed(() => video.value?.engines.find((e) => e.id === animEngine.value) ?? null);
function engineFrames(e: { frames_rule: string; max_frames: number; fps: number }) {
  const want = Math.max(9, Math.round(animSeconds.value * e.fps));
  const step = e.frames_rule === "8k+1" ? 8 : e.frames_rule === "4k+1" ? 4 : 1;
  if (step === 1) return Math.min(want, e.max_frames);
  const n = Math.round((want - 1) / step) * step + 1;
  return n <= e.max_frames ? n : Math.floor((e.max_frames - 1) / step) * step + 1;
}
const estimate = computed(() => {
  const e = engine.value;
  if (!e) return null;
  const frames = engineFrames(e);
  return { frames, seconds: e.cost.load_s + frames * e.cost.s_per_frame, covers: frames / e.fps };
});
// Formulation recommandée par le guide creation_video.md (§2) : caméra verrouillée, mouvement lent et local.
function defaultMotion() {
  return "Locked camera, no camera movement. Water, clouds and vegetation move gently and naturally. Preserve every detail, material, and lighting exactly, motion slow and continuous.";
}
async function animate() {
  if (!engine.value) return;
  emit("error", null);
  animating.value = { fraction: 0, started: Date.now() };
  try {
    const item = await api.animateBackground(engine.value.id, animPrompt.value || defaultMotion(), animSeconds.value, null,
      zoneMode.value && zones.value.length ? zones.value : null);
    await refresh();
    await choose(item);
  } catch (e) { emit("error", errorText(e)); }
  animating.value = null;
}
function fmtDur(s: number) { return s < 90 ? `${Math.round(s)} s` : `${Math.round(s / 60)} min`; }

const current = computed(() => props.pv.manifest.composite.background);
const currentItem = computed(() => items.value.find((i) => i.path === current.value) ?? null);
const backend = computed(() => props.pv.manifest.generation.backend);
const readyWorkflows = computed(() => backends.value?.comfy.workflows ?? []);
const canGenerate = computed(() => {
  if (!prompt.value.trim() || props.busy || generating.value) return false;
  if (backend.value === "builtin") return !!backends.value?.builtin_ok;
  const wf = readyWorkflows.value.find((w) => `comfyui:${w.id}` === backend.value);
  return !!wf?.ready;
});

// Suggestions : la scène se décrit en langage courant ; ces ajouts orientent vers une « plaque » de décor.
const STYLES = [
  { label: "Intérieur lumineux", text: "intérieur lumineux et moderne, grandes fenêtres, lumière du jour douce" },
  { label: "Extérieur nature", text: "paysage extérieur naturel, végétation, lumière naturelle" },
  { label: "Ville", text: "rue de ville contemporaine, façades, trottoir" },
  { label: "Studio", text: "fond de studio photo uni avec éclairage doux et dégradé subtil" },
  { label: "Coucher de soleil", text: "à l'heure dorée, lumière chaude rasante" },
  { label: "Nuit", text: "de nuit, éclairage urbain, ambiance bleutée" },
];

function addStyle(t: string) {
  prompt.value = prompt.value.trim() ? `${prompt.value.trim()}, ${t}` : t;
}

async function refresh() {
  try {
    items.value = await api.listBackgrounds();
    backends.value = await api.imageBackends();
  } catch (e) { emit("error", errorText(e)); }
}

async function choose(item: BackgroundItem) {
  try { emit("applied", await api.selectBackground(item.path)); } catch (e) { emit("error", errorText(e)); }
}

async function remove(item: BackgroundItem) {
  try { items.value = await api.deleteBackground(item.path); } catch (e) { emit("error", errorText(e)); }
}

async function importImage() {
  const path = await open({ title: "Choisir une image de décor", multiple: false,
    filters: [{ name: "Image", extensions: ["png", "jpg", "jpeg", "webp", "PNG", "JPG", "JPEG"] }] });
  if (!path || Array.isArray(path)) return;
  try { emit("applied", await api.setBackground(path)); await refresh(); } catch (e) { emit("error", errorText(e)); }
}

async function setBackend(value: string) {
  try { emit("applied", await api.setGeneration({ backend: value, comfy_url: comfyUrl.value })); } catch (e) { emit("error", errorText(e)); }
}

async function checkComfy() {
  try {
    emit("applied", await api.setGeneration({ backend: backend.value, comfy_url: comfyUrl.value }));
    backends.value = await api.imageBackends();
  } catch (e) { emit("error", errorText(e)); }
}

async function generate() {
  emit("error", null);
  generating.value = { index: 0, count: count.value, done: 0, total: 0 };
  try {
    const text = empty.value ? `${prompt.value.trim()}. ${PLATE_SUFFIX}` : prompt.value.trim();
    const made = await api.generateBackgrounds(text, count.value, seed.value);
    await refresh();
    if (made.length) await choose(made[0]);
  } catch (e) {
    emit("error", errorText(e));
  }
  generating.value = null;
}

async function cancel() {
  try { await api.cancelJob(); } catch (e) { emit("error", errorText(e)); }
}

let unlisten: UnlistenFn | undefined;
let unlistenVideo: UnlistenFn | undefined;
onMounted(async () => {
  await refresh();
  try {
    video.value = await api.videoEngines();
    animSeconds.value = Math.min(5, Math.max(1, Math.round(video.value.clip_seconds)));
    const firstOk = video.value.engines.find((e) => e.available && e.kind !== "builtin");
    if (firstOk) animEngine.value = firstOk.id;
  } catch { /* registre absent : fonction masquée */ }
  unlistenVideo = await onVideoProgress((f) => { if (animating.value) animating.value = { ...animating.value, fraction: f }; });
  unlisten = await onImageProgress((p) => {
    if (!generating.value) return;
    generating.value = { ...generating.value, ...(p.index !== undefined ? { index: p.index, count: p.count ?? generating.value.count } : {}), done: p.done, total: p.total };
  });
});
onBeforeUnmount(() => { unlisten?.(); unlistenVideo?.(); });

defineExpose({ refresh });
</script>

<template>
  <section class="library">
    <div class="head">
      <h3>Décors du projet</h3>
      <button @click="importImage">Importer…</button>
    </div>
    <div v-if="items.length" class="grid">
      <figure v-for="it in items" :key="it.path" :class="{ active: it.path === current }" :title="it.meta.prompt ?? it.meta.original_name ?? it.path">
        <video v-if="it.kind === 'video'" :src="fileUrl(it.abs_path)" muted loop autoplay playsinline @click="choose(it)"></video>
        <img v-else :src="fileUrl(it.abs_path)" alt="" @click="choose(it)" />
        <figcaption>
          <span class="chip" :class="it.source === 'imported' ? 'none' : 'ok'">{{ { generated: "généré", animated: "animé", imported: "importé" }[it.source] }}</span>
          <button v-if="it.path !== current" class="x" title="Supprimer ce décor" @click="remove(it)">×</button>
        </figcaption>
      </figure>
    </div>
    <p v-else class="muted small">Aucun décor : importez une image ou générez-en un ci-dessous.</p>
    <p v-if="currentItem?.meta.prompt" class="meta muted small mono">
      {{ currentItem.meta.backend === "comfyui" ? `ComfyUI · ${currentItem.meta.workflow} v${currentItem.meta.workflow_version}` : currentItem.meta.model }}
      · graine {{ currentItem.meta.seed }} · {{ currentItem.meta.width }}×{{ currentItem.meta.height }}<template v-if="currentItem.meta.seconds"> · {{ currentItem.meta.seconds }} s</template>
    </p>

    <p v-if="currentItem?.kind === 'video'" class="meta muted small mono">
      {{ currentItem.meta.engine_title }} · {{ currentItem.meta.frames }} images, boucle aller-retour · {{ currentItem.meta.seconds }} s de calcul
    </p>

    <details v-if="video && currentIsImage" class="gen anim">
      <summary>Animer ce décor <span class="chip warn">expérimental</span></summary>
      <p class="muted small">Le décor fixe devient un plan court où l'eau, les nuages ou la végétation bougent, caméra fixe. Le plan est bouclé en aller-retour pour couvrir l'extrait. Calcul long et gourmand : les autres modèles sont déchargés pendant ce temps.</p>
      <label>Moteur
        <select id="anim-engine" v-model="animEngine">
          <option v-for="e in video.engines" :key="e.id" :value="e.id" :disabled="!e.available">
            {{ e.title }}{{ e.tested ? "" : " — non validé" }}{{ e.available ? "" : " — indisponible" }}
          </option>
        </select>
      </label>
      <template v-if="engine">
        <p class="muted small">{{ engine.notes }}</p>
        <p class="small mono">Licence : {{ engine.license }}</p>
        <ul v-if="engine.problems.length" class="problems"><li v-for="p in engine.problems" :key="p">{{ p }}</li></ul>
      </template>
      <label class="check"><input id="anim-zones" type="checkbox" v-model="zoneMode" /> Animer seulement des zones (cinémagraphe, recommandé)</label>
      <div v-if="zoneMode && currentItem" class="zonepick">
        <p class="muted small">Cliquez sur chaque zone à animer (eau, ciel, feuillage…). Tout le reste du décor restera fixe.</p>
        <div class="zoneimg" @click="addZone">
          <img :src="fileUrl(currentItem.abs_path)" alt="Décor : cliquez sur les zones à animer" />
          <span v-for="(z, i) in zones" :key="i" class="zdot" :style="{ left: `${z[0] * 100}%`, top: `${z[1] * 100}%` }">{{ i + 1 }}</span>
        </div>
        <button v-if="zones.length" class="chipbtn" @click="zones = []">Effacer les zones</button>
      </div>
      <label>Mouvement (en anglais, caméra fixe)
        <textarea id="anim-prompt" v-model="animPrompt" rows="3" maxlength="1000" :placeholder="defaultMotion()"></textarea>
      </label>
      <label>Durée du plan généré (s)
        <input id="anim-seconds" type="number" min="1" max="10" step="1" v-model.number="animSeconds" />
      </label>
      <p v-if="estimate && engine" class="small mono">
        {{ estimate.frames }} images ({{ estimate.covers.toFixed(1) }} s, puis boucle) · ≈ {{ fmtDur(estimate.seconds) }} · pic ≈ {{ engine.cost.peak_gb }} Go
      </p>
      <div class="row2">
        <button class="primary" :disabled="!engine?.available || !!animating || busy || !!generating || (zoneMode && !zones.length)" @click="animate">Animer</button>
        <button v-if="animating" @click="cancel">Annuler</button>
      </div>
      <div v-if="animating" class="progress">
        <span class="small">Animation en cours · {{ Math.round(animating.fraction * 100) }} % · {{ fmtDur((Date.now() - animating.started) / 1000) }}</span>
        <progress :value="animating.fraction" max="1"></progress>
      </div>
    </details>

    <details class="gen" open>
      <summary>Générer un décor</summary>
      <label>Description
        <textarea id="bg-prompt" v-model="prompt" rows="3" maxlength="1200" placeholder="Ex. : terrasse en bois face à un lac de montagne, fin d'après-midi"></textarea>
      </label>
      <div class="chips">
        <button v-for="s in STYLES" :key="s.label" class="chipbtn" @click="addStyle(s.text)">+ {{ s.label }}</button>
      </div>
      <label class="check"><input id="bg-empty" type="checkbox" v-model="empty" /> Décor vide, plan large à hauteur d'yeux, sol visible</label>
      <div class="row2">
        <label>Variantes
          <select id="bg-count" v-model.number="count"><option :value="1">1</option><option :value="2">2</option><option :value="3">3</option><option :value="4">4</option></select>
        </label>
        <label>Graine (optionnelle)
          <input id="bg-seed" type="number" min="0" max="16777215" :value="seed ?? ''" placeholder="aléatoire"
            @input="seed = ($event.target as HTMLInputElement).value === '' ? null : Number(($event.target as HTMLInputElement).value)" />
        </label>
      </div>
      <label>Moteur
        <select id="bg-backend" :value="backend" @change="setBackend(($event.target as HTMLSelectElement).value)">
          <option value="builtin" :disabled="!backends?.builtin_ok">Intégré — FLUX.2 klein 4B (local)</option>
          <option v-for="w in readyWorkflows" :key="w.id" :value="`comfyui:${w.id}`">
            ComfyUI — {{ w.title }} (v{{ w.version }}){{ w.ready ? "" : " — indisponible" }}
          </option>
        </select>
      </label>
      <p class="muted small" v-if="backend === 'builtin'">{{ backends?.builtin_detail }}. Le modèle de détourage est déchargé pendant la génération (≈ 24 Go).</p>
      <template v-else>
        <div class="row2">
          <input id="comfy-url" type="text" v-model="comfyUrl" aria-label="Adresse du serveur ComfyUI" />
          <button @click="checkComfy">Vérifier</button>
        </div>
        <p class="muted small">{{ backends?.comfy.reachable ? `ComfyUI ${backends.comfy.version ?? ""} connecté` : backends?.comfy.detail }}</p>
        <ul v-for="w in readyWorkflows.filter((x) => `comfyui:${x.id}` === backend)" :key="w.id" class="problems">
          <li v-for="pb in w.problems" :key="pb">{{ pb }}</li>
          <li v-if="!w.tested && w.ready" class="muted">Workflow non encore validé de bout en bout sur ce Mac.</li>
        </ul>
      </template>
      <div class="row2">
        <button class="primary" :disabled="!canGenerate" @click="generate">Générer {{ count > 1 ? `${count} variantes` : "" }}</button>
        <button v-if="generating" @click="cancel">Annuler</button>
      </div>
      <div v-if="generating" class="progress">
        <span class="small">Variante {{ generating.index + 1 }} / {{ generating.count }}<template v-if="generating.total"> · étape {{ generating.done }} / {{ generating.total }}</template><template v-else> · chargement du modèle…</template></span>
        <progress :value="generating.total ? generating.index * generating.total + generating.done : undefined" :max="generating.total ? generating.count * generating.total : 1"></progress>
      </div>
    </details>
  </section>
</template>

<style scoped>
.library { display: grid; gap: 10px; }
.head { display: flex; justify-content: space-between; align-items: center; }
h3 { font-size: 12px; text-transform: uppercase; letter-spacing: 0.06em; color: var(--muted); margin: 0; }
.grid { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 6px; }
figure { margin: 0; position: relative; border: 1px solid var(--line); border-radius: 4px; overflow: hidden; cursor: pointer; }
figure.active { border-color: var(--accent); box-shadow: 0 0 0 1px var(--accent); }
figure img, figure video { display: block; width: 100%; aspect-ratio: 16 / 9; object-fit: cover; }
.anim summary .chip { margin-left: 6px; font-size: 10px; }
.zonepick { display: grid; gap: 6px; }
.zoneimg { position: relative; cursor: crosshair; border: 1px solid var(--line); border-radius: 4px; overflow: hidden; }
.zoneimg img { display: block; width: 100%; }
.zdot { position: absolute; transform: translate(-50%, -50%); width: 18px; height: 18px; border-radius: 50%;
  background: var(--accent); color: #fff; font-size: 11px; display: grid; place-items: center; border: 2px solid #fff; }
figcaption { position: absolute; inset: auto 3px 3px 3px; display: flex; justify-content: space-between; align-items: center; }
figcaption .chip { background: rgba(0, 0, 0, 0.6); font-size: 9px; padding: 2px 4px; }
.x { padding: 0 6px; line-height: 16px; background: rgba(0, 0, 0, 0.6); border: none; }
.meta { margin: 0; overflow-wrap: anywhere; }
.gen { border: 1px solid var(--line); border-radius: 6px; padding: 8px 12px; display: grid; gap: 10px; }
.gen[open] { padding-bottom: 12px; }
summary { cursor: pointer; font-weight: 600; }
textarea { font: 12px/1.4 var(--font); color: var(--fg); background: var(--bg); border: 1px solid var(--line); border-radius: 4px; padding: 6px 8px; resize: vertical; width: 100%; user-select: text; }
.chips { display: flex; flex-wrap: wrap; gap: 4px; }
.chipbtn { padding: 2px 7px; font-size: 11px; }
.row2 { display: flex; gap: 8px; align-items: end; }
.row2 > * { flex: 1; }
.check { display: flex; align-items: center; gap: 8px; color: var(--fg); }
.problems { margin: 0; padding-left: 16px; color: var(--warn); font-size: 12px; }
.progress { display: grid; gap: 4px; }
progress { width: 100%; accent-color: var(--accent); }
.small { font-size: 12px; }
</style>
