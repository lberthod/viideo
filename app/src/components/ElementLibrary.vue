<script setup lang="ts">
// Éléments de premier plan : objets générés (FLUX.2) ou importés, détourés automatiquement (SAM 2),
// placés devant le sujet pour lui donner un appui et masquer une coupure (bas du corps hors champ,
// pieds cachés par des herbes…).
import { onMounted, ref } from "vue";
import { open } from "@tauri-apps/plugin-dialog";
import { api, errorText, fileUrl, type ElementItem, type Foreground } from "../api";

const props = defineProps<{ current: Foreground | null; busy: boolean }>();
const emit = defineEmits<{
  (e: "choose", item: ElementItem): void;
  (e: "update", f: Foreground | null): void;
  (e: "autoplace", cover: number): void;
  (e: "error", msg: string | null): void;
}>();

const items = ref<ElementItem[]>([]);
const prompt = ref("");
const working = ref<string | null>(null);
const cover = ref(0.3);

const IDEAS = [
  "bureau en bois clair, vue de face",
  "pupitre moderne en verre et métal",
  "rambarde en bois de terrasse",
  "touffe d'herbes hautes et de fleurs sauvages",
  "rochers moussus",
  "plante verte en pot",
];

async function refresh() {
  try { items.value = await api.listElements(); } catch (e) { emit("error", errorText(e)); }
}

async function generate() {
  emit("error", null);
  working.value = "Génération et détourage de l'élément…";
  try {
    items.value = await api.createElement(prompt.value, null);
    if (items.value.length) emit("choose", items.value[0]);
  } catch (e) { emit("error", errorText(e)); }
  working.value = null;
}

async function importFile() {
  const path = await open({ title: "Choisir une image d'élément (PNG transparent ou photo à détourer)", multiple: false,
    filters: [{ name: "Image", extensions: ["png", "jpg", "jpeg", "webp", "PNG", "JPG", "JPEG"] }] });
  if (!path || Array.isArray(path)) return;
  working.value = "Import et détourage…";
  try {
    items.value = await api.importElement(path);
    if (items.value.length) emit("choose", items.value[0]);
  } catch (e) { emit("error", errorText(e)); }
  working.value = null;
}

function set(key: "width" | "x" | "y", v: number) {
  if (props.current) emit("update", { ...props.current, [key]: v });
}

onMounted(refresh);
defineExpose({ refresh });
</script>

<template>
  <fieldset class="elements">
    <legend>Élément de premier plan</legend>
    <p class="muted small">Un objet devant le sujet (bureau, rambarde, herbes…) lui donne un appui et cache une coupure, par exemple un corps coupé en bas de l'image.</p>
    <div v-if="items.length" class="grid">
      <figure v-for="it in items" :key="it.path" :class="{ active: current?.path === it.path }" :title="it.meta.prompt ?? it.path" @click="emit('choose', it)">
        <img :src="fileUrl(it.abs_path)" alt="" />
      </figure>
    </div>
    <label>Décrire un élément
      <textarea id="el-prompt" v-model="prompt" rows="2" maxlength="500" placeholder="Ex. : bureau en bois clair, vue de face"></textarea>
    </label>
    <div class="ideas">
      <button v-for="i in IDEAS" :key="i" class="chipbtn" @click="prompt = i">{{ i }}</button>
    </div>
    <div class="row2">
      <button class="primary" :disabled="!prompt.trim() || !!working || busy" @click="generate">Générer et détourer</button>
      <button :disabled="!!working || busy" @click="importFile">Importer…</button>
    </div>
    <p v-if="working" class="small">{{ working }}</p>
    <template v-if="current">
      <label>Hauteur du sujet masquée <span class="mono">{{ Math.round(cover * 100) }} %</span>
        <input id="el-cover" type="range" min="0.05" max="0.7" step="0.01" v-model.number="cover" />
      </label>
      <button @click="emit('autoplace', cover)">Placer devant le bas du sujet</button>
      <label>Largeur <span class="mono">{{ current.width.toFixed(2) }}</span>
        <input id="el-w" type="range" min="0.05" max="2" step="0.01" :value="current.width" @input="set('width', Number(($event.target as HTMLInputElement).value))" />
      </label>
      <label>Position X <span class="mono">{{ current.x.toFixed(2) }}</span>
        <input id="el-x" type="range" min="-0.3" max="1.3" step="0.005" :value="current.x" @input="set('x', Number(($event.target as HTMLInputElement).value))" />
      </label>
      <label>Bord bas <span class="mono">{{ current.y.toFixed(2) }}</span>
        <input id="el-y" type="range" min="0.2" max="1.5" step="0.005" :value="current.y" @input="set('y', Number(($event.target as HTMLInputElement).value))" />
      </label>
      <button @click="emit('update', null)">Retirer l'élément</button>
    </template>
  </fieldset>
</template>

<style scoped>
.elements { display: grid; gap: 8px; }
.grid { display: grid; grid-template-columns: repeat(4, minmax(0, 1fr)); gap: 6px; }
figure { margin: 0; border: 1px solid var(--line); border-radius: 4px; cursor: pointer; padding: 4px;
  background: repeating-conic-gradient(#3a3f45 0% 25%, #2a2e33 0% 50%) 50% / 12px 12px; }
figure.active { border-color: var(--accent); box-shadow: 0 0 0 1px var(--accent); }
figure img { display: block; width: 100%; aspect-ratio: 1; object-fit: contain; }
textarea { font: 12px/1.4 var(--font); color: var(--fg); background: var(--bg); border: 1px solid var(--line); border-radius: 4px; padding: 6px 8px; resize: vertical; width: 100%; user-select: text; }
.ideas { display: flex; flex-wrap: wrap; gap: 4px; }
.chipbtn { padding: 2px 7px; font-size: 11px; color: var(--muted); }
.row2 { display: flex; gap: 8px; }
.row2 > * { flex: 1; }
.small { font-size: 12px; }
</style>
