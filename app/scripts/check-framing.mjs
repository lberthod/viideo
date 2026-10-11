// Vérifie la géométrie du cadrage (copie de la fonction pure, sans DOM).
import fs from "node:fs";
const src = fs.readFileSync(new URL("../src/compositor.ts", import.meta.url), "utf8");
const body = src.slice(src.indexOf("export function framing"), src.indexOf("// --- Préréglages d'éclairage"))
  .replace("export function framing(preset: FramingPreset, b: Bounds, current: Composite): Pick<Composite, \"subject_scale\" | \"subject_x\" | \"subject_y\">", "function framing(preset, b, current)")
  .replace("let anchor: { edge: \"bottom\" | \"top\"; at: number } =", "let anchor =")
  .replaceAll(": number", "");
const framing = new Function(body + "; return framing;")();
const tf = (c, X, Y) => [0.5 + (X - 0.5) * c.subject_scale + c.subject_x, 0.5 + (Y - 0.5) * c.subject_scale + c.subject_y];
const cur = { subject_scale: 1 };
let fails = 0;
const check = (name, cond) => { console.log((cond ? "OK  " : "ÉCHEC ") + name); if (!cond) fails++; };
// Sujet en pied, entier, décentré à droite
const full = { x0: 0.55, y0: 0.1, x1: 0.75, y1: 0.9 };
let c = framing("center", full, cur);
let [, bot] = tf(c, 0.65, 0.9); let [cx] = tf(c, 0.65, 0.5);
check("centre : sujet centré", Math.abs(cx - 0.5) < 0.01);
check("centre : hauteur 78 %", Math.abs((0.8 * c.subject_scale) - 0.78) < 0.01);
check("centre : pieds à 92 %", Math.abs(bot - 0.92) < 0.01);
c = framing("third-left", full, cur); [cx] = tf(c, 0.65, 0.5);
check("tiers gauche", Math.abs(cx - 1 / 3) < 0.01);
// Présentatrice coupée en bas (buste) : le bord bas source doit rester hors cadre
const bust = { x0: 0.35, y0: 0.08, x1: 0.7, y1: 1.0 };
for (const p of ["center", "ground", "third-left", "third-right", "closeup"]) {
  c = framing(p, bust, cur);
  const [, y1] = tf(c, 0.5, 1.0);
  check(`${p} : coupure basse invisible (Y'(1)=${y1.toFixed(3)})`, y1 >= 0.999);
}
// Sujet coupé à gauche
const leftCut = { x0: 0.0, y0: 0.2, x1: 0.3, y1: 0.9 };
c = framing("third-right", leftCut, cur); const [x0] = tf(c, 0.0, 0.5);
check(`coupure gauche invisible (X'(0)=${x0.toFixed(3)})`, x0 <= 0.001);
// Plan serré : la tête reste dans le cadre
c = framing("closeup", full, cur); const [, top] = tf(c, 0.65, 0.1);
check(`plan serré : haut du sujet à 6 % (${top.toFixed(3)})`, Math.abs(top - 0.06) < 0.01);


// --- Placement d'un élément de premier plan ---
const pbody = src.slice(src.indexOf("export function placeElement"), src.indexOf("interface Stats {"))
  .replace("export function placeElement(b: Bounds, c: Composite, aspect: number, outAspect: number, cover: number)", "function placeElement(b, c, aspect, outAspect, cover)")
  .replaceAll(": number", "");
const placeElement = new Function(pbody + "; return placeElement;")();
const presenter = { x0: 0.35, y0: 0.08, x1: 0.7, y1: 1.0 };
const comp = { subject_scale: 0.85, subject_x: 0, subject_y: 0.07 };
for (const [name, aspect] of [["comptoir large", 2.5], ["pupitre carré", 1.0], ["panneau vertical", 0.5]]) {
  const e = placeElement(presenter, comp, aspect, 16 / 9, 0.36);
  const h = (e.width * 16 / 9) / aspect;
  const top = e.y - h;
  const y0t = 0.5 + (0.08 - 0.5) * 0.85 + 0.07, y1t = 1.0;
  const want = y1t - 0.36 * (y1t - y0t);
  const subjW = (0.7 - 0.35) * 0.85;
  check(`${name} : bord haut sur la ligne (${top.toFixed(3)} ≈ ${want.toFixed(3)})`, Math.abs(top - want) < 0.01);
  if (aspect >= 1) check(`${name} : au moins aussi large que le sujet`, e.width >= subjW * 1.09);
  check(`${name} : bas sous le cadre (coupure cachée)`, e.y >= 1.0 || e.y === 1.6);
}
// --- Relighting : mêmes coefficients que media.rs (test relight dans graph_contains_expected_stages) ---
const rbody = src.slice(src.indexOf("const RELIGHT_LZ"), src.indexOf("const clamp8"))
  .replace("export function lightKey(hex: string): [number, number, number]", "function lightKey(hex)")
  .replace("export function relightCoeffs(c: Composite)", "function relightCoeffs(c)")
  .replace("const ch = (i: number) =>", "const ch = (i) =>")
  .replace(" as [number, number, number]", "");
const { relightCoeffs, lightKey } = new Function(rbody + "; return { relightCoeffs, lightKey };")();
const rc = relightCoeffs({ relight: 0.5, light_x: 1, light_y: 0, light_color: "#ffffff" });
check(`relighting : ombrage identique à l'export (${rc.shade.map((v) => v.toFixed(4)).join(", ")})`,
  [1.715, 0, 1.029, -1.372].every((v, i) => Math.abs(rc.shade[i] - v) < 1e-3));
const r1 = relightCoeffs({ relight: 1, light_x: -0.3, light_y: -0.5, light_color: "#ffffff" });
check("relighting : ombrage moyen = exposition inchangée", Math.abs(r1.offset * 255 + r1.scale[0] * 127.5 - 127.5) < 1e-9);
const k = lightKey("#ff8000");
check("relighting : couleur normalisée (moyenne 1)", k[0] > 1.5 && k[2] === 0 && Math.abs(lightKey("#808080")[1] - 1) < 1e-9);

process.exit(fails ? 1 : 0);
