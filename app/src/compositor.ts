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
    if (cache.size > 400) cache.delete(cache.keys().next().value as string);
  }
  return p;
}

/** Équivalent du filtre FFmpeg `eq` : contraste et luminosité sur la luma, saturation sur la chroma. */
function applyEq(data: Uint8ClampedArray, c: Composite) {
  if (c.brightness === 0 && c.contrast === 1 && c.saturation === 1) return;
  const b = c.brightness * 255;
  for (let i = 0; i < data.length; i += 4) {
    const r = data[i], g = data[i + 1], bl = data[i + 2];
    const y = 0.299 * r + 0.587 * g + 0.114 * bl;
    const y2 = (y - 128) * c.contrast + 128 + b;
    data[i] = y2 + (r - y) * c.saturation;
    data[i + 1] = y2 + (g - y) * c.saturation;
    data[i + 2] = y2 + (bl - y) * c.saturation;
  }
}

const work = document.createElement("canvas");
const maskCanvas = document.createElement("canvas");

/** Sujet détouré (RGBA) à la taille du proxy. */
export function cutout(frame: HTMLImageElement, mask: HTMLImageElement, c: Composite): HTMLCanvasElement {
  const w = frame.naturalWidth, h = frame.naturalHeight;
  maskCanvas.width = w; maskCanvas.height = h;
  const mctx = maskCanvas.getContext("2d", { willReadFrequently: true })!;
  mctx.filter = c.feather >= 0.5 ? `blur(${c.feather}px)` : "none";
  mctx.drawImage(mask, 0, 0, w, h);
  const m = mctx.getImageData(0, 0, w, h).data;

  work.width = w; work.height = h;
  const ctx = work.getContext("2d", { willReadFrequently: true })!;
  ctx.drawImage(frame, 0, 0);
  const img = ctx.getImageData(0, 0, w, h);
  applyEq(img.data, c);
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
