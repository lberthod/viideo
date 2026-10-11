<script setup lang="ts">
// Visionneuse d'une image du proxy : source, masque superposé ou composite, avec saisie des invites.
import { onMounted, ref, watch } from "vue";
import type { Composite, Prompt } from "../api";
import { drawComposite, loadImage } from "../compositor";

export type ViewMode = "source" | "mask" | "composite" | "split";
export type Tool = "positive" | "negative" | "box";

const props = defineProps<{
  frameUrl: string | null;
  maskUrl: string | null;
  backgroundUrl: string | null;
  mode: ViewMode;
  composite: Composite;
  prompt: Prompt | null;
  tool: Tool | null;
  width: number;
  height: number;
}>();
const emit = defineEmits<{
  (e: "point", x: number, y: number, positive: boolean): void;
  (e: "box", box: [number, number, number, number]): void;
}>();

const canvas = ref<HTMLCanvasElement | null>(null);
const missing = ref<string | null>(null);
const drag = ref<{ x0: number; y0: number; x1: number; y1: number } | null>(null);
// Vue avant/après : position du séparateur (fraction de la largeur), déplacée à la souris.
const split = ref(0.5);
const splitting = ref(false);
const isComposite = () => props.mode === "composite" || props.mode === "split";
const tint = document.createElement("canvas");
let token = 0;

async function render() {
  const cv = canvas.value;
  if (!cv || !props.frameUrl) return;
  const my = ++token;
  try {
    const frame = await loadImage(props.frameUrl);
    const mask = props.maskUrl && props.mode !== "source" ? await loadImage(props.maskUrl).catch(() => null) : null;
    const bg = isComposite() && props.backgroundUrl ? await loadImage(props.backgroundUrl).catch(() => null) : null;
    if (my !== token) return; // un rendu plus récent a été demandé
    const ctx = cv.getContext("2d")!;
    const { width: w, height: h } = cv;
    ctx.clearRect(0, 0, w, h);
    missing.value = null;
    if (isComposite()) {
      if (!mask) { missing.value = "Pas de masque pour cette image : propagez le masque."; ctx.drawImage(frame, 0, 0, w, h); }
      else drawComposite(ctx, w, h, frame, mask, bg, props.composite);
      if (props.mode === "split") {
        // À gauche du séparateur : la source d'origine.
        const x = Math.round(split.value * w);
        ctx.save();
        ctx.beginPath(); ctx.rect(0, 0, x, h); ctx.clip();
        ctx.drawImage(frame, 0, 0, w, h);
        ctx.restore();
        ctx.fillStyle = "#fff"; ctx.fillRect(x - 1, 0, 2, h);
        ctx.font = `${Math.round(h / 36)}px -apple-system, sans-serif`;
        ctx.fillStyle = "rgba(0,0,0,.55)";
        ctx.fillRect(x - 96, 12, 84, h / 24); ctx.fillRect(x + 12, 12, 84, h / 24);
        ctx.fillStyle = "#fff"; ctx.textAlign = "center"; ctx.textBaseline = "middle";
        ctx.fillText("avant", x - 54, 12 + h / 48); ctx.fillText("après", x + 54, 12 + h / 48);
      }
    } else {
      ctx.drawImage(frame, 0, 0, w, h);
      if (mask) {
        tint.width = w; tint.height = h;
        const t = tint.getContext("2d")!;
        t.clearRect(0, 0, w, h);
        t.drawImage(mask, 0, 0, w, h);
        // Le masque est en niveaux de gris : on le convertit en alpha avant de le teinter.
        const img = t.getImageData(0, 0, w, h);
        for (let i = 0; i < img.data.length; i += 4) {
          img.data[i + 3] = img.data[i];
          img.data[i] = 214; img.data[i + 1] = 40; img.data[i + 2] = 52;
        }
        t.putImageData(img, 0, 0);
        ctx.globalAlpha = 0.45;
        ctx.drawImage(tint, 0, 0);
        ctx.globalAlpha = 1;
      }
    }
    drawPrompt(ctx);
  } catch (e) {
    missing.value = String(e);
  }
}

function drawPrompt(ctx: CanvasRenderingContext2D) {
  if (isComposite()) return;
  const s = props.width / 1280 + 0.6;
  const b = props.prompt?.box;
  const live = drag.value;
  const boxes: [number, number, number, number][] = [];
  if (b) boxes.push(b);
  if (live) boxes.push([Math.min(live.x0, live.x1), Math.min(live.y0, live.y1), Math.max(live.x0, live.x1), Math.max(live.y0, live.y1)]);
  for (const [x0, y0, x1, y1] of boxes) {
    ctx.save();
    ctx.setLineDash([10 * s, 6 * s]);
    ctx.lineWidth = 2.5 * s;
    ctx.strokeStyle = "#ffd23f";
    ctx.strokeRect(x0, y0, x1 - x0, y1 - y0);
    ctx.restore();
  }
  for (const p of props.prompt?.points ?? []) {
    ctx.beginPath();
    ctx.arc(p.x, p.y, 9 * s, 0, Math.PI * 2);
    ctx.fillStyle = p.positive ? "#2fbf71" : "#e5484d";
    ctx.fill();
    ctx.lineWidth = 2.5 * s;
    ctx.strokeStyle = "#fff";
    ctx.stroke();
    ctx.beginPath();
    ctx.moveTo(p.x - 4.5 * s, p.y); ctx.lineTo(p.x + 4.5 * s, p.y);
    if (p.positive) { ctx.moveTo(p.x, p.y - 4.5 * s); ctx.lineTo(p.x, p.y + 4.5 * s); }
    ctx.stroke();
  }
}

function toImage(ev: MouseEvent): [number, number] {
  const cv = canvas.value!;
  const r = cv.getBoundingClientRect();
  const x = ((ev.clientX - r.left) / r.width) * cv.width;
  const y = ((ev.clientY - r.top) / r.height) * cv.height;
  return [Math.min(Math.max(x, 0), cv.width), Math.min(Math.max(y, 0), cv.height)];
}

function onDown(ev: MouseEvent) {
  if (props.mode === "split") { splitting.value = true; onMove(ev); return; }
  if (!props.tool || isComposite()) return;
  const [x, y] = toImage(ev);
  if (props.tool === "box") {
    drag.value = { x0: x, y0: y, x1: x, y1: y };
  } else {
    // Alt+clic inverse le type de point.
    const positive = (props.tool === "positive") !== ev.altKey;
    emit("point", Math.round(x), Math.round(y), positive);
  }
}

function onMove(ev: MouseEvent) {
  if (splitting.value) {
    split.value = toImage(ev)[0] / canvas.value!.width;
    render();
    return;
  }
  if (!drag.value) return;
  const [x, y] = toImage(ev);
  drag.value = { ...drag.value, x1: x, y1: y };
  render();
}

function onUp() {
  splitting.value = false;
  const d = drag.value;
  drag.value = null;
  if (!d) return;
  const box: [number, number, number, number] = [
    Math.round(Math.min(d.x0, d.x1)), Math.round(Math.min(d.y0, d.y1)),
    Math.round(Math.max(d.x0, d.x1)), Math.round(Math.max(d.y0, d.y1)),
  ];
  if (box[2] - box[0] >= 6 && box[3] - box[1] >= 6) emit("box", box);
  else render();
}

watch(() => [props.frameUrl, props.maskUrl, props.backgroundUrl, props.mode, props.prompt, props.composite, props.width, props.height], render, { deep: true });
onMounted(render);
</script>

<template>
  <div class="viewer" :class="{ armed: tool && (mode === 'mask' || mode === 'source'), splitter: mode === 'split' }">
    <canvas
      v-if="frameUrl"
      ref="canvas"
      :width="width"
      :height="height"
      :style="{ aspectRatio: `${width} / ${height}` }"
      @mousedown="onDown"
      @mousemove="onMove"
      @mouseup="onUp"
      @mouseleave="onUp"
    />
    <div v-else class="empty">Préparez l'extrait pour afficher les images.</div>
    <p v-if="missing" class="missing">{{ missing }}</p>
  </div>
</template>

<style scoped>
.viewer { position: relative; display: grid; place-items: center; min-height: 0; height: 100%; }
canvas { max-width: 100%; max-height: 100%; background: #000; box-shadow: 0 0 0 1px var(--line); }
.armed canvas { cursor: crosshair; }
.splitter canvas { cursor: ew-resize; }
.empty { color: var(--muted); }
.missing { position: absolute; bottom: 8px; left: 50%; transform: translateX(-50%); margin: 0; padding: 6px 10px;
  background: var(--panel); border: 1px solid var(--line); font-size: 12px; color: var(--warn); }
</style>
