<script setup lang="ts">
// Bibliothèque de décors du projet : import, génération locale (moteur intégré ou ComfyUI), choix.
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { open } from "@tauri-apps/plugin-dialog";
import type { UnlistenFn } from "@tauri-apps/api/event";
import {
  api, errorText, fileUrl, onImageProgress,
  type BackgroundItem, type ImageBackends, type ProjectView,
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
const PLATE = "Photographie réaliste, plan large à hauteur d'yeux, scène vide sans personne, sol visible au premier plan, faible profondeur de champ.";

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
    const text = empty.value ? `${prompt.value.trim()}. ${PLATE}` : prompt.value.trim();
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
onMounted(async () => {
  await refresh();
  unlisten = await onImageProgress((p) => {
    if (!generating.value) return;
    generating.value = { ...generating.value, ...(p.index !== undefined ? { index: p.index, count: p.count ?? generating.value.count } : {}), done: p.done, total: p.total };
  });
});
onBeforeUnmount(() => unlisten?.());

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
        <img :src="fileUrl(it.abs_path)" alt="" @click="choose(it)" />
        <figcaption>
          <span class="chip" :class="it.source === 'generated' ? 'ok' : 'none'">{{ it.source === "generated" ? "généré" : "importé" }}</span>
          <button v-if="it.path !== current" class="x" title="Supprimer ce décor" @click="remove(it)">×</button>
        </figcaption>
      </figure>
    </div>
    <p v-else class="muted small">Aucun décor : importez une image ou générez-en un ci-dessous.</p>
    <p v-if="currentItem?.meta.prompt" class="meta muted small mono">
      {{ currentItem.meta.backend === "comfyui" ? `ComfyUI · ${currentItem.meta.workflow} v${currentItem.meta.workflow_version}` : currentItem.meta.model }}
      · graine {{ currentItem.meta.seed }} · {{ currentItem.meta.width }}×{{ currentItem.meta.height }}<template v-if="currentItem.meta.seconds"> · {{ currentItem.meta.seconds }} s</template>
    </p>

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
figure img { display: block; width: 100%; aspect-ratio: 16 / 9; object-fit: cover; }
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
