// Aperçu du composite dans un canvas, avec les mêmes formules que l'export FFmpeg
// (media.rs::filter_graph) pour que l'aperçu reflète le rendu final.
import type { Composite } from "./api";

const cache = new Map<string, Promise<HTMLImageElement>>();

export function loadImage(url: string): Promise<HTMLImageElement> {
  let p = cache.get(url);
  if (!p) {
    p = new Promise((resolve, reject) => {
      const img = new Image();
      // Le protocole asset de Tauri renvoie les en-têtes CORS : sans ce mode, le canvas serait
      // « contaminé » et getImageData lèverait une SecurityError.
      img.crossOrigin = "anonymous";
      img.onload = () => resolve(img);
      img.onerror = () => { cache.delete(url); reject(new Error(`image illisible : ${url}`)); };
      img.src = url;
    });
    cache.set(url, p);
    if (cache.size > 600) cache.delete(cache.keys().next().value as string);
  }
  return p;
}

/** Précharge des images sans attendre (lecture fluide). */
export function preload(urls: string[]) {
  for (const u of urls) loadImage(u).catch(() => undefined);
}

/** Équivalent de `eq` puis `lutrgb` (température) de l'export. */
function applyColor(data: Uint8ClampedArray, c: Composite) {
  const t = c.temperature * 60; // media.rs::temperature_offset
  if (c.brightness === 0 && c.contrast === 1 && c.saturation === 1 && t === 0) return;
  const b = c.brightness * 255;
  for (let i = 0; i < data.length; i += 4) {
    const r = data[i], g = data[i + 1], bl = data[i + 2];
    const y = 0.299 * r + 0.587 * g + 0.114 * bl;
    const y2 = (y - 128) * c.contrast + 128 + b;
    data[i] = y2 + (r - y) * c.saturation + t;
    data[i + 1] = y2 + (g - y) * c.saturation;
    data[i + 2] = y2 + (bl - y) * c.saturation - t;
  }
}

const work = document.createElement("canvas");
const maskCanvas = document.createElement("canvas");

/** Masque adouci puis courbe de bord (media.rs::edge_threshold), en niveaux 0..255 par pixel RGBA. */
function maskAlpha(mask: HTMLImageElement, w: number, h: number, c: Composite): Uint8ClampedArray {
  maskCanvas.width = w; maskCanvas.height = h;
  const mctx = maskCanvas.getContext("2d", { willReadFrequently: true })!;
  mctx.filter = c.feather >= 0.5 ? `blur(${c.feather}px)` : "none";
  mctx.drawImage(mask, 0, 0, w, h);
  const m = mctx.getImageData(0, 0, w, h).data;
  if (Math.abs(c.edge) > 1e-3) {
    const thr = 128 + c.edge * 120;
    for (let i = 0; i < m.length; i += 4) m[i] = Math.min(255, Math.max(0, (m[i] - thr) * 2 + 128));
  }
  return m;
}

/** Sujet détouré (RGBA) à la taille du proxy. */
export function cutout(frame: HTMLImageElement, mask: HTMLImageElement, c: Composite): HTMLCanvasElement {
  const w = frame.naturalWidth, h = frame.naturalHeight;
  const m = maskAlpha(mask, w, h, c);
  work.width = w; work.height = h;
  const ctx = work.getContext("2d", { willReadFrequently: true })!;
  ctx.drawImage(frame, 0, 0);
  const img = ctx.getImageData(0, 0, w, h);
  applyColor(img.data, c);
  for (let i = 0; i < img.data.length; i += 4) img.data[i + 3] = m[i];
  ctx.putImageData(img, 0, 0);
  return work;
}

export function drawCover(ctx: CanvasRenderingContext2D, img: CanvasImageSource & { width: number; height: number }, w: number, h: number) {
  const iw = (img as HTMLImageElement).naturalWidth || img.width;
  const ih = (img as HTMLImageElement).naturalHeight || img.height;
  const k = Math.max(w / iw, h / ih);
  const dw = iw * k, dh = ih * k;
  ctx.drawImage(img, (w - dw) / 2, (h - dh) / 2, dw, dh);
}

export function drawComposite(
  ctx: CanvasRenderingContext2D, w: number, h: number,
  frame: HTMLImageElement, mask: HTMLImageElement, bg: HTMLImageElement | null, c: Composite,
) {
  if (bg) drawCover(ctx, bg, w, h);
  else { ctx.fillStyle = c.background_color; ctx.fillRect(0, 0, w, h); }
  const fg = cutout(frame, mask, c);
  const sw = w * c.subject_scale, sh = h * c.subject_scale;
  ctx.drawImage(fg, (w - sw) / 2 + c.subject_x * w, (h - sh) / 2 + c.subject_y * h, sw, sh);
}

interface Stats { luma: number; lumaStd: number; warmth: number; chroma: number; n: number }

function stats(data: Uint8ClampedArray, weight?: Uint8ClampedArray): Stats {
  let n = 0, sy = 0, sy2 = 0, sw = 0, sc = 0;
  for (let i = 0; i < data.length; i += 16) { // une sur quatre : largement suffisant
    if (weight && weight[i] < 160) continue;
    const r = data[i], g = data[i + 1], b = data[i + 2];
    const y = 0.299 * r + 0.587 * g + 0.114 * b;
    sy += y; sy2 += y * y; sw += r - b; sc += Math.abs(r - y) + Math.abs(b - y); n++;
  }
  n = Math.max(n, 1);
  const luma = sy / n;
  return { luma, lumaStd: Math.sqrt(Math.max(sy2 / n - luma * luma, 1)), warmth: sw / n, chroma: sc / n, n };
}

const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));
const round2 = (v: number) => Math.round(v * 100) / 100;

/**
 * Harmonisation automatique, volontairement prudente : elle parcourt environ un tiers de l'écart
 * de lumière et de couleur entre le sujet et le décor, avec des bornes serrées. Un décor très clair
 * ou très saturé ne doit pas délaver ni brûler le sujet ; l'utilisateur affine ensuite à la main.
 */
export function harmonize(frame: HTMLImageElement, mask: HTMLImageElement, bg: HTMLImageElement | null, c: Composite): Partial<Composite> {
  const w = frame.naturalWidth, h = frame.naturalHeight;
  const neutral = { ...c, brightness: 0, contrast: 1, saturation: 1, temperature: 0, edge: 0, feather: 0 };
  const m = maskAlpha(mask, w, h, neutral);
  work.width = w; work.height = h;
  const ctx = work.getContext("2d", { willReadFrequently: true })!;
  ctx.drawImage(frame, 0, 0);
  const subj = stats(ctx.getImageData(0, 0, w, h).data, m);
  if (bg) drawCover(ctx, bg, w, h);
  else { ctx.fillStyle = c.background_color; ctx.fillRect(0, 0, w, h); }
  const back = stats(ctx.getImageData(0, 0, w, h).data);
  if (subj.n < 50) return {};
  const k = 0.33;
  return {
    brightness: round2(clamp(((back.luma - subj.luma) / 255) * k, -0.12, 0.12)),
    contrast: round2(clamp(1 + (back.lumaStd / subj.lumaStd - 1) * k, 0.9, 1.15)),
    saturation: round2(clamp(1 + (back.chroma / Math.max(subj.chroma, 1) - 1) * k, 0.8, 1.2)),
    // La température décale R−B de 120 × t.
    temperature: round2(clamp(((back.warmth - subj.warmth) * k) / 120, -0.2, 0.2)),
  };
}
