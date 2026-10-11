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
process.exit(fails ? 1 : 0);
