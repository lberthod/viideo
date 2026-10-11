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

// --- Relighting directionnel (mêmes formules que media.rs::relight_coeffs) -----------------------
const RELIGHT_LZ = 0.6;
const RELIGHT_AMP = 0.8; // media.rs::RELIGHT_AMP

export function lightKey(hex: string): [number, number, number] {
  const ch = (i: number) => { const v = parseInt(hex.slice(i, i + 2), 16); return Number.isFinite(v) ? v : 255; };
  const r = ch(1), g = ch(3), b = ch(5);
  const sum = Math.max(r + g + b, 1);
  return [r, g, b].map((v) => Math.min(2, (3 * v) / sum)) as [number, number, number];
}

export function relightCoeffs(c: Composite) {
  const n = Math.hypot(c.light_x, c.light_y, RELIGHT_LZ);
  const l = [c.light_x / n, c.light_y / n, RELIGHT_LZ / n];
  const s = c.relight;
  return {
    shade: [2 * l[0], 2 * l[1], 2 * l[2], -(l[0] + l[1] + l[2])],
    scale: lightKey(c.light_color).map((k) => RELIGHT_AMP * s * k),
    offset: 0.5 * (1 - RELIGHT_AMP * s),
  };
}

const clamp8 = (v: number) => (v < 0 ? 0 : v > 255 ? 255 : Math.round(v));

/** Sujet × gain (normales encodées → ombrage → gain codé 127,5 = ×1), comme colorchannelmixer + blend. */
function applyRelight(data: Uint8ClampedArray, normals: Uint8ClampedArray, c: Composite) {
  const { shade: [a, b, z, d], scale: [kr, kg, kb], offset } = relightCoeffs(c);
  const o = offset * 255;
  for (let i = 0; i < data.length; i += 4) {
    const sh = clamp8(a * normals[i] + b * normals[i + 1] + z * normals[i + 2] + d * 255);
    data[i] = (data[i] * clamp8(kr * sh + o)) / 127.5;
    data[i + 1] = (data[i + 1] * clamp8(kg * sh + o)) / 127.5;
    data[i + 2] = (data[i + 2] * clamp8(kb * sh + o)) / 127.5;
  }
}

/**
 * Lumière du décor : direction = barycentre des 3 % de pixels les plus lumineux (après flou), par
 * rapport au centre ; couleur = moyenne de ces pixels. Volontairement simple et prévisible ;
 * l'utilisateur corrige ensuite la direction s'il le faut.
 */
export function estimateLight(bg: Drawable | null): Pick<Composite, "light_x" | "light_y" | "light_color"> | null {
  if (!bg) return null;
  const [iw, ih] = dims(bg);
  const w = 160, h = Math.max(1, Math.round((160 * ih) / iw));
  small.width = w; small.height = h;
  const ctx = small.getContext("2d", { willReadFrequently: true })!;
  ctx.filter = `blur(${0.02 * h}px)`;
  ctx.drawImage(bg, 0, 0, w, h);
  ctx.filter = "none";
  const d = ctx.getImageData(0, 0, w, h).data;
  const lum = new Float32Array(w * h);
  for (let i = 0; i < lum.length; i++) lum[i] = 0.299 * d[i * 4] + 0.587 * d[i * 4 + 1] + 0.114 * d[i * 4 + 2];
  const thr = Array.from(lum).sort((p, q) => p - q)[Math.floor(lum.length * 0.97)];
  let n = 0, sx = 0, sy = 0, r = 0, g = 0, b = 0;
  for (let i = 0; i < lum.length; i++) {
    if (lum[i] < thr) continue;
    n++; sx += i % w; sy += Math.floor(i / w); r += d[i * 4]; g += d[i * 4 + 1]; b += d[i * 4 + 2];
  }
  if (!n) return null;
  const hex = (v: number) => Math.round(v / n).toString(16).padStart(2, "0");
  return {
    light_x: round2(clamp((sx / n / w - 0.5) * 2.5, -1, 1)),
    // Lumière jamais franchement par en dessous : un décor clair en bas (neige, eau) ne doit pas
    // éclairer le visage par le menton.
    light_y: round2(clamp((sy / n / h - 0.5) * 2.5, -1, 0.3)),
    light_color: `#${hex(r)}${hex(g)}${hex(b)}`,
  };
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

const normalsCanvas = document.createElement("canvas");

/** Sujet détouré (RGBA) à la taille du proxy. */
export function cutout(frame: HTMLImageElement, mask: HTMLImageElement, c: Composite, normals?: HTMLImageElement | null): HTMLCanvasElement {
  const w = frame.naturalWidth, h = frame.naturalHeight;
  const m = maskAlpha(mask, w, h, c);
  work.width = w; work.height = h;
  const ctx = work.getContext("2d", { willReadFrequently: true })!;
  ctx.drawImage(frame, 0, 0);
  const img = ctx.getImageData(0, 0, w, h);
  applyColor(img.data, c);
  if (normals && c.relight > 0.01) {
    normalsCanvas.width = w; normalsCanvas.height = h;
    const nctx = normalsCanvas.getContext("2d", { willReadFrequently: true })!;
    nctx.drawImage(normals, 0, 0, w, h);
    applyRelight(img.data, nctx.getImageData(0, 0, w, h).data, c);
  }
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

const layer = document.createElement("canvas");
const layer2 = document.createElement("canvas");
// media.rs : WRAP_MASK_BLUR_720, WRAP_BG_BLUR_720, wrap_gain
const WRAP_MASK_BLUR_720 = 10, WRAP_BG_BLUR_720 = 24;

export interface Extras { shadow?: HTMLImageElement | null; element?: HTMLImageElement | null; normals?: HTMLImageElement | null }

/** Ordre (identique à l'export) : décor, ombre, sujet (couleur puis relighting), light wrap, élément. */
export function drawComposite(
  ctx: CanvasRenderingContext2D, w: number, h: number,
  frame: HTMLImageElement, mask: HTMLImageElement, bg: Drawable | null, c: Composite, extras: Extras = {},
) {
  const drawBg = (target: CanvasRenderingContext2D, extraBlur = 0) => {
    const b = (c.bg_blur >= 0.5 ? (c.bg_blur * h) / 720 : 0) + extraBlur;
    if (bg) {
      // Flou du décor exprimé pour 720 px de haut (media.rs : gblur mis à l'échelle de la sortie).
      target.filter = b > 0 ? `blur(${b}px)` : "none";
      drawCover(target, bg, w, h);
      target.filter = "none";
    } else { target.fillStyle = c.background_color; target.fillRect(0, 0, w, h); }
  };
  drawBg(ctx);
  const sw = w * c.subject_scale, sh = h * c.subject_scale;
  const sx = (w - sw) / 2 + c.subject_x * w, sy = (h - sh) / 2 + c.subject_y * h;

  if (extras.shadow && c.shadow > 0.01) {
    const sc = layer;
    sc.width = extras.shadow.naturalWidth; sc.height = extras.shadow.naturalHeight;
    const s = sc.getContext("2d", { willReadFrequently: true })!;
    s.drawImage(extras.shadow, 0, 0);
    const d = s.getImageData(0, 0, sc.width, sc.height);
    for (let i = 0; i < d.data.length; i += 4) { d.data[i + 3] = d.data[i] * c.shadow; d.data[i] = d.data[i + 1] = d.data[i + 2] = 0; }
    s.putImageData(d, 0, 0);
    ctx.drawImage(sc, sx, sy, sw, sh);
  }

  const fg = cutout(frame, mask, c, extras.normals);
  ctx.drawImage(fg, sx, sy, sw, sh);

  if (c.light_wrap > 0.01) {
    // Bande de bord = masque × (1 − flou(masque)), dans l'espace de sortie ; mélange vers le décor flouté.
    const mc = layer2;
    mc.width = w; mc.height = h;
    const m = mc.getContext("2d", { willReadFrequently: true })!;
    m.fillStyle = "#000"; m.fillRect(0, 0, w, h);
    const alpha = maskAlpha(mask, frame.naturalWidth, frame.naturalHeight, c);
    const mcv = document.createElement("canvas");
    mcv.width = frame.naturalWidth; mcv.height = frame.naturalHeight;
    const mctx = mcv.getContext("2d")!;
    const md = mctx.createImageData(mcv.width, mcv.height);
    for (let i = 0; i < alpha.length; i += 4) { md.data[i] = md.data[i + 1] = md.data[i + 2] = alpha[i]; md.data[i + 3] = 255; }
    mctx.putImageData(md, 0, 0);
    m.drawImage(mcv, sx, sy, sw, sh);
    const sharp = m.getImageData(0, 0, w, h).data;
    m.filter = `blur(${(WRAP_MASK_BLUR_720 * h) / 720}px)`;
    m.drawImage(mc, 0, 0);
    m.filter = "none";
    const soft = m.getImageData(0, 0, w, h).data;
    const bc = layer;
    bc.width = w; bc.height = h;
    const bctx = bc.getContext("2d", { willReadFrequently: true })!;
    drawBg(bctx, (WRAP_BG_BLUR_720 * h) / 720);
    const bgb = bctx.getImageData(0, 0, w, h).data;
    const out = ctx.getImageData(0, 0, w, h);
    const k = 2 * c.light_wrap;
    for (let i = 0; i < out.data.length; i += 4) {
      const a = Math.min(1, (sharp[i] / 255) * (1 - soft[i] / 255) * k);
      if (a <= 0) continue;
      out.data[i] = out.data[i] * (1 - a) + bgb[i] * a;
      out.data[i + 1] = out.data[i + 1] * (1 - a) + bgb[i + 1] * a;
      out.data[i + 2] = out.data[i + 2] * (1 - a) + bgb[i + 2] * a;
    }
    ctx.putImageData(out, 0, 0);
  }

  const f = c.foreground;
  if (f && extras.element) {
    const ew = f.width * w, eh = (ew * extras.element.naturalHeight) / extras.element.naturalWidth;
    ctx.drawImage(extras.element, f.x * w - ew / 2, f.y * h - eh, ew, eh);
  }
}

/**
 * Place un élément de premier plan pour masquer le bas du sujet.
 * - Son bord haut est à la hauteur `cover` (fraction de la hauteur du sujet, depuis le bas).
 * - Il est au moins 10 % plus large que le sujet (sinon ses côtés laisseraient voir la coupure).
 * - Sa hauteur suit son rapport d'aspect : il peut déborder sous le bord de l'image, ce qui est
 *   naturel pour un objet au premier plan (il ne remonte jamais au-dessus de la ligne demandée).
 * - Si le sujet n'est pas coupé et que l'élément est plus court, il est posé au niveau des pieds.
 */
export function placeElement(b: Bounds, c: Composite, aspect: number, outAspect: number, cover: number) {
  const s = c.subject_scale;
  const tx = (X: number) => 0.5 + (X - 0.5) * s + c.subject_x;
  const ty = (Y: number) => 0.5 + (Y - 0.5) * s + c.subject_y;
  const x0 = tx(b.x0), x1 = tx(b.x1), y0 = ty(b.y0), y1 = Math.min(ty(b.y1), 1.0);
  const cut = b.y1 > 0.985 || ty(b.y1) >= 0.999;
  const top = y1 - cover * (y1 - y0);
  const minBottom = cut ? 1.03 : y1 + 0.03;
  // Largeur : celle qui couvre de `top` à `minBottom`, mais au moins 1,1 × la largeur du sujet.
  let width = Math.max((minBottom - top) * aspect / outAspect, (x1 - x0) * 1.1);
  // Priorité à la ligne de couverture : le bas ne peut pas dépasser 1,6 (limite validée par Rust) ;
  // un élément très haut (panneau) devient alors plus étroit que le sujet.
  width = Math.min(width, (1.6 - top) * aspect / outAspect);
  const bottom = top + (width * outAspect) / aspect;
  const r = (v: number) => Math.round(v * 1000) / 1000;
  return { width: r(Math.min(3, Math.max(0.03, width))), x: r((x0 + x1) / 2), y: r(Math.min(1.6, bottom)) };
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
