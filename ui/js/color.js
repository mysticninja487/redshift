// Colour maths in OKLab/OKLCH, so "a bit lighter" or "more vivid" looks the same on every hue.
// Colours are hex strings outside this module; inside, {L, C, h} (h in degrees).

import { clamp } from './util.js';

const toLin = (c) => (c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4);
const toSrgb = (c) => (c <= 0.0031308 ? 12.92 * c : 1.055 * c ** (1 / 2.4) - 0.055);

export function parseHex(hex) {
  let s = String(hex).trim().replace('#', '');
  if (s.length === 3) s = [...s].map((c) => c + c).join('');
  const n = parseInt(s.slice(0, 6), 16);
  if (!Number.isFinite(n) || s.length < 6) return [0, 0, 0];
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}
export const toHex = ([r, g, b]) => '#' + [r, g, b].map((v) => clamp(Math.round(v), 0, 255).toString(16).padStart(2, '0')).join('');

export function rgbToOklab([r, g, b]) {
  const [lr, lg, lb] = [r / 255, g / 255, b / 255].map(toLin);
  const l = Math.cbrt(0.4122214708 * lr + 0.5363325363 * lg + 0.0514459929 * lb);
  const m = Math.cbrt(0.2119034982 * lr + 0.6806995451 * lg + 0.1073969566 * lb);
  const s = Math.cbrt(0.0883024619 * lr + 0.2817188376 * lg + 0.6299787005 * lb);
  return [0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s, 1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s, 0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s];
}
export function oklabToRgbRaw([L, a, b]) {
  const l = (L + 0.3963377774 * a + 0.2158037573 * b) ** 3;
  const m = (L - 0.1055613458 * a - 0.0638541728 * b) ** 3;
  const s = (L - 0.0894841775 * a - 1.291485548 * b) ** 3;
  return [4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s, -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s, -0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s];
}

export function hexToLch(hex) {
  const [L, a, b] = rgbToOklab(parseHex(hex));
  return { L, C: Math.hypot(a, b), h: ((Math.atan2(b, a) * 180) / Math.PI + 360) % 360 };
}
export function rgbToLch(rgb) {
  const [L, a, b] = rgbToOklab(rgb);
  return { L, C: Math.hypot(a, b), h: ((Math.atan2(b, a) * 180) / Math.PI + 360) % 360 };
}

/** Converts to hex, lowering chroma until the colour fits the sRGB gamut. */
export function lchToHex({ L, C, h }) {
  L = clamp(L, 0, 1);
  const hr = (h * Math.PI) / 180;
  let c = Math.max(0, C);
  for (let i = 0; i < 24; i++) {
    const lin = oklabToRgbRaw([L, c * Math.cos(hr), c * Math.sin(hr)]);
    if (lin.every((v) => v >= -0.0005 && v <= 1.0005)) return toHex(lin.map((v) => toSrgb(clamp(v, 0, 1)) * 255));
    c *= 0.93;
  }
  return toHex(oklabToRgbRaw([L, 0, 0]).map((v) => toSrgb(clamp(v, 0, 1)) * 255));
}

export const lighten = (hex, dL) => { const c = hexToLch(hex); return lchToHex({ ...c, L: c.L + dL }); };
export const withL = (hex, L) => { const c = hexToLch(hex); return lchToHex({ ...c, L }); };
export const withC = (hex, C) => { const c = hexToLch(hex); return lchToHex({ ...c, C }); };
export const rotateHue = (hex, deg) => { const c = hexToLch(hex); return lchToHex({ ...c, h: (c.h + deg + 360) % 360 }); };

/** Mixes two hex colours in OKLab. t=0 gives a, t=1 gives b. */
export function mixHex(a, b, t) {
  const x = rgbToOklab(parseHex(a)), y = rgbToOklab(parseHex(b));
  const lab = x.map((v, i) => v + (y[i] - v) * t);
  return toHex(oklabToRgbRaw(lab).map((v) => toSrgb(clamp(v, 0, 1)) * 255));
}

export function luminance(hex) {
  const [r, g, b] = parseHex(hex).map((v) => toLin(v / 255));
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}
export function contrast(a, b) {
  const [x, y] = [luminance(a), luminance(b)].sort((p, q) => q - p);
  return (x + 0.05) / (y + 0.05);
}
/** Moves `fg` lighter or darker (whichever helps) until it has at least `min` contrast on `bg`. */
export function ensureContrast(fg, bg, min = 4.5) {
  if (contrast(fg, bg) >= min) return fg;
  const bgDark = luminance(bg) < 0.4;
  const c = hexToLch(fg);
  for (let i = 0; i < 40; i++) {
    c.L = clamp(c.L + (bgDark ? 0.02 : -0.02), 0, 1);
    const out = lchToHex(c);
    if (contrast(out, bg) >= min) return out;
  }
  return bgDark ? '#ffffff' : '#000000';
}
/** Black or white, whichever reads better on `bg`. */
export const onColor = (bg) => (contrast('#ffffff', bg) >= contrast('#000000', bg) ? '#ffffff' : '#000000');
export const isLight = (hex) => luminance(hex) > 0.4;

/** Hex plus alpha as an rgba() string. */
export function alpha(hex, a) { const [r, g, b] = parseHex(hex); return `rgba(${r},${g},${b},${a})`; }
