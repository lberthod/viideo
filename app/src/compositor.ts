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

/** Décor animé : un élément vidéo par URL, positionné au temps demandé (boucle). */
const videos = new Map<string, HTMLVideoElement>();
export async function videoFrameAt(url: string, seconds: number): Promise<HTMLVideoElement> {
  let v = videos.get(url);
  if (!v) {
    v = document.createElement("video");
    v.crossOrigin = "anonymous";
    v.muted = true;
    v.preload = "auto";
    v.src = url;
    videos.set(url, v);
    await new Promise<void>((resolve, reject) => {
      v!.addEventListener("loadeddata", () => resolve(), { once: true });
      v!.addEventListener("error", () => { videos.delete(url); reject(new Error(`vidéo illisible : ${url}`)); }, { once: true });
    });
  }
  const t = v.duration > 0 ? seconds % v.duration : 0;
  if (Math.abs(v.currentTime - t) > 0.001) {
    await new Promise<void>((resolve) => {
      v!.addEventListener("seeked", () => resolve(), { once: true });
      v!.currentTime = t;
    });
  }
  return v;
}

type Drawable = HTMLImageElement | HTMLVideoElement | HTMLCanvasElement;
const dims = (d: Drawable) => d instanceof HTMLVideoElement ? [d.videoWidth, d.videoHeight]
  : d instanceof HTMLImageElement ? [d.naturalWidth, d.naturalHeight] : [d.width, d.height];

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

export function drawCover(ctx: CanvasRenderingContext2D, img: Drawable, w: number, h: number) {
  const [iw, ih] = dims(img);
  const k = Math.max(w / iw, h / ih);
  const dw = iw * k, dh = ih * k;
  ctx.drawImage(img, (w - dw) / 2, (h - dh) / 2, dw, dh);
}

export function drawComposite(
  ctx: CanvasRenderingContext2D, w: number, h: number,
  frame: HTMLImageElement, mask: HTMLImageElement, bg: Drawable | null, c: Composite,
) {
  if (bg) {
    // Flou du décor exprimé pour 720 px de haut (media.rs : gblur mis à l'échelle de la sortie).
    ctx.filter = c.bg_blur >= 0.5 ? `blur(${(c.bg_blur * h) / 720}px)` : "none";
    drawCover(ctx, bg, w, h);
    ctx.filter = "none";
  } else { ctx.fillStyle = c.background_color; ctx.fillRect(0, 0, w, h); }
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
export function harmonize(frame: HTMLImageElement, mask: HTMLImageElement, bg: Drawable | null, c: Composite): Partial<Composite> {
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

// --- Préréglages de cadrage ------------------------------------------------------

/** Boîte englobante du sujet (fractions 0..1 de l'image), réunie sur plusieurs masques du clip. */
export interface Bounds { x0: number; y0: number; x1: number; y1: number }

const small = document.createElement("canvas");
export function subjectBounds(masks: HTMLImageElement[]): Bounds | null {
  let b: Bounds | null = null;
  for (const m of masks) {
    const w = 160, h = Math.max(1, Math.round((160 * m.naturalHeight) / m.naturalWidth));
    small.width = w; small.height = h;
    const ctx = small.getContext("2d", { willReadFrequently: true })!;
    ctx.drawImage(m, 0, 0, w, h);
    const d = ctx.getImageData(0, 0, w, h).data;
    let x0 = w, y0 = h, x1 = -1, y1 = -1;
    for (let y = 0; y < h; y++) for (let x = 0; x < w; x++) {
      if (d[(y * w + x) * 4] > 127) { if (x < x0) x0 = x; if (x > x1) x1 = x; if (y < y0) y0 = y; if (y > y1) y1 = y; }
    }
    if (x1 < 0) continue;
    const f = { x0: x0 / w, y0: y0 / h, x1: (x1 + 1) / w, y1: (y1 + 1) / h };
    b = b ? { x0: Math.min(b.x0, f.x0), y0: Math.min(b.y0, f.y0), x1: Math.max(b.x1, f.x1), y1: Math.max(b.y1, f.y1) } : f;
  }
  return b;
}

export type FramingPreset = "center" | "ground" | "third-left" | "third-right" | "closeup";

/**
 * Calcule échelle et position pour un cadrage type.
 *
 * Transformation (identique à l'export) : X' = 0,5 + (X − 0,5)·s + x, Y' = 0,5 + (Y − 0,5)·s + y,
 * en fractions de l'image. Un sujet coupé par un bord de la vidéo source (jambes hors champ…) doit
 * garder ce bord hors du cadre de sortie, sinon la coupure devient visible :
 *   coupé en bas → Y'(1) ≥ 1 ; en haut → Y'(0) ≤ 0 ; à gauche → X'(0) ≤ 0 ; à droite → X'(1) ≥ 1.
 */
export function framing(preset: FramingPreset, b: Bounds, current: Composite): Pick<Composite, "subject_scale" | "subject_x" | "subject_y"> {
  const cut = { bottom: b.y1 > 0.985, top: b.y0 < 0.015, left: b.x0 < 0.015, right: b.x1 > 0.985 };
  const height = b.y1 - b.y0, cx = (b.x0 + b.x1) / 2;
  let s = current.subject_scale;
  let targetCx = 0.5;
  let anchor: { edge: "bottom" | "top"; at: number } = { edge: "bottom", at: 0.92 };
  switch (preset) {
    case "center": s = 0.78 / height; break;
    case "ground": s = 0.62 / height; anchor = { edge: "bottom", at: 0.95 }; break;
    case "third-left": targetCx = 1 / 3; s = Math.min(s, 0.8 / height); break;
    case "third-right": targetCx = 2 / 3; s = Math.min(s, 0.8 / height); break;
    case "closeup": s = 1.5 / height; anchor = { edge: "top", at: 0.06 }; break;
  }
  // Coupé des deux côtés opposés : impossible de réduire sans montrer une coupure.
  if ((cut.left && cut.right) || (cut.top && cut.bottom)) s = Math.max(s, 1);
  // Les réglages sont stockés au centième : on arrondit l'échelle d'abord, puis la position du côté
  // qui respecte les contraintes de coupure (sinon 0,5 % de coupure pourrait réapparaître).
  s = Math.round(Math.min(2, Math.max(0.2, s)) * 100) / 100;
  const round = (v: number) => Math.round(v * 100) / 100;
  const up = (v: number) => Math.ceil(v * 100 - 1e-9) / 100;
  const down = (v: number) => Math.floor(v * 100 + 1e-9) / 100;
  let x = round(targetCx - (0.5 + (cx - 0.5) * s));
  let y = round(anchor.edge === "bottom" ? anchor.at - (0.5 + (b.y1 - 0.5) * s) : anchor.at - (0.5 + (b.y0 - 0.5) * s));
  if (cut.bottom) y = Math.max(y, up(0.5 - 0.5 * s));
  if (cut.top) y = Math.min(y, down(0.5 * s - 0.5));
  if (cut.left) x = Math.min(x, down(0.5 * s - 0.5));
  if (cut.right) x = Math.max(x, up(0.5 - 0.5 * s));
  const lim = (v: number) => Math.min(0.8, Math.max(-0.8, v));
  return { subject_scale: s, subject_x: lim(x), subject_y: lim(y) };
}

// --- Préréglages d'éclairage ---------------------------------------------------------

export const LIGHTING: Record<string, { label: string; values: Partial<Composite> }> = {
  neutral: { label: "Neutre", values: { brightness: 0, contrast: 1, saturation: 1, temperature: 0 } },
  golden: { label: "Heure dorée", values: { brightness: 0.02, contrast: 1.03, saturation: 1.1, temperature: 0.2 } },
  overcast: { label: "Ciel couvert", values: { brightness: 0, contrast: 0.9, saturation: 0.85, temperature: -0.06 } },
  studio: { label: "Studio doux", values: { brightness: 0.03, contrast: 0.92, saturation: 0.95, temperature: 0 } },
  night: { label: "Nuit", values: { brightness: -0.12, contrast: 1.08, saturation: 0.75, temperature: -0.22 } },
  dramatic: { label: "Contrasté", values: { brightness: -0.04, contrast: 1.2, saturation: 0.9, temperature: 0 } },
};
