<script setup lang="ts">
import { onMounted, ref } from "vue";
import { open, save } from "@tauri-apps/plugin-dialog";
import { api, errorText, type ProjectView, type ToolStatus } from "./api";
import EditorView from "./components/EditorView.vue";

const project = ref<ProjectView | null>(null);
const tools = ref<ToolStatus[]>([]);
const error = ref<string | null>(null);
const working = ref(false);

onMounted(async () => {
  try { tools.value = await api.checkEnvironment(); } catch (e) { error.value = errorText(e); }
  try { project.value = await api.devAutoload(); } catch (e) { error.value = errorText(e); }
});

async function newProject() {
  error.value = null;
  const source = await open({
    title: "Choisir la vidéo à transformer",
    multiple: false,
    filters: [{ name: "Vidéo", extensions: ["mp4", "mov", "m4v", "MP4", "MOV"] }],
  });
  if (!source || Array.isArray(source)) return;
  const dir = await save({ title: "Nom et emplacement du projet", defaultPath: "Projet videeo" });
  if (!dir) return;
  working.value = true;
  try { project.value = await api.createProject(dir, source); } catch (e) { error.value = errorText(e); }
  working.value = false;
}

async function openProject() {
  error.value = null;
  const dir = await open({ title: "Ouvrir un projet videeo (dossier)", directory: true, multiple: false });
  if (!dir || Array.isArray(dir)) return;
  working.value = true;
  try { project.value = await api.openProject(dir); } catch (e) { error.value = errorText(e); }
  working.value = false;
}

async function close() {
  await api.closeProject();
  project.value = null;
}
</script>

<template>
  <EditorView v-if="project" :initial="project" @close="close" />
  <main v-else class="home">
    <section class="intro">
      <h1>videeo</h1>
      <p class="muted">Détourez le sujet d'une courte vidéo, placez-le devant un nouveau décor et exportez un MP4. Tout reste sur ce Mac.</p>
      <div class="actions">
        <button class="primary" :disabled="working" @click="newProject">Nouveau projet depuis une vidéo…</button>
        <button :disabled="working" @click="openProject">Ouvrir un projet…</button>
      </div>
      <p v-if="error" class="error-banner">{{ error }}</p>
    </section>
    <section class="env">
      <h2>Environnement</h2>
      <ul>
        <li v-for="t in tools" :key="t.name">
          <span class="chip" :class="t.ok ? 'ok' : 'warn'">{{ t.ok ? "OK" : "manquant" }}</span>
          <strong>{{ t.name }}</strong>
          <span class="muted mono detail" :title="t.path">{{ t.detail }}</span>
        </li>
        <li v-if="!tools.length" class="muted">Vérification…</li>
      </ul>
    </section>
  </main>
</template>

<style scoped>
.home { height: 100%; display: grid; grid-template-columns: minmax(0, 1.2fr) minmax(0, 1fr); gap: 48px; padding: 64px; align-content: center; max-width: 1200px; margin: 0 auto; }
.intro { display: grid; gap: 18px; align-content: start; }
.intro h1 { font-size: 44px; letter-spacing: -0.02em; }
.intro h1::after { content: ""; display: inline-block; width: 12px; height: 12px; margin-left: 8px; background: var(--accent); border-radius: 50%; }
.intro p { font-size: 15px; max-width: 46ch; margin: 0; }
.actions { display: flex; gap: 10px; flex-wrap: wrap; }
.actions button { padding: 10px 16px; }
.env { background: var(--panel); border: 1px solid var(--line); border-radius: 8px; padding: 18px 20px; display: grid; gap: 12px; align-content: start; }
.env h2 { font-size: 12px; text-transform: uppercase; letter-spacing: 0.08em; color: var(--muted); }
.env ul { list-style: none; margin: 0; padding: 0; display: grid; gap: 10px; }
.env li { display: grid; grid-template-columns: auto 1fr; gap: 2px 10px; align-items: center; }
.detail { grid-column: 2; font-size: 11px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
</style>
