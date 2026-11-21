// Turns (skin + the person's choices + the current cover) into CSS variables on <html>.
//
// Colour modes:  skin   the skin's own palette
//                preset one of the accent presets, on the skin's surfaces
//                custom the person's own accent / background / text
//                art    accent and surface tint taken from the playing cover
// Type, shape and motion settings become variables too, so skin CSS only ever reads variables.

import { emit, on } from './bus.js';
import * as C from './color.js';
import { cfg, getPrefs } from './prefs.js';
import { ACCENTS, fontStack, getSkin } from './skins.js';
import { state } from './state.js';
import { clamp } from './util.js';

export const tokens = { accent: '#ca0032', bg: '#111', fg: '#eee', scheme: 'dark', art2: '#ca0032', art3: '#ca0032' };

const DENSITY = { compact: 0.78, cozy: 1, roomy: 1.28 };

function surfaces(bg, scheme) {
  const dir = scheme === 'dark' ? 1 : -1;
  return { bg2: C.lighten(bg, 0.028 * dir), bg3: C.lighten(bg, 0.07 * dir) };
}

export function computeTokens(c, skin, art) {
  const scheme = c.color.scheme === 'skin' ? skin.scheme : c.color.scheme;
  const base = { ...(skin.palette[scheme] ?? skin.palette[skin.scheme]) };
  const p = { ...base };
  const mode = c.color.mode;

  if (mode === 'preset') {
    p.accent = ACCENTS.find((a) => a.id === c.color.preset)?.hex ?? p.accent;
  } else if (mode === 'custom') {
    if (c.color.accent) p.accent = c.color.accent;
    if (c.color.fg) p.fg = c.color.fg;
    if (c.color.bg) { p.bg = c.color.bg; Object.assign(p, surfaces(p.bg, C.isLight(p.bg) ? 'light' : 'dark')); }
  } else if (mode === 'art' && art?.grey) {
    // No colour to take: a white or grey accent, light enough on dark skins and dark enough on light ones.
    p.accent = C.lchToHex({ L: scheme === 'dark' ? art.neutral : clamp(1.06 - art.neutral, 0.12, 0.45), C: 0, h: 0 });
  } else if (mode === 'art' && art) {
    const mixAmt = clamp(c.color.artMix, 0, 1);
    const tint = art.dominant;
    for (const k of ['bg', 'bg2', 'bg3']) {
      const b = C.hexToLch(p[k]);
      const strength = mixAmt * clamp(tint.C / 0.12, 0.25, 1);
      p[k] = C.lchToHex({ L: b.L, C: b.C + 0.05 * strength, h: tint.h });
    }
    p.accent = art.accent;
    p.paper = p.paper ? C.mixHex(p.paper, art.accent, 0.12 * mixAmt) : p.paper;
    // Skins with a colour sweep (wavelength) take its stops from the cover.
    if (p.g1) {
      p.g1 = C.lchToHex({ L: 0.93, C: 0.015 + 0.03 * mixAmt, h: tint.h });
      p.g2 = C.lchToHex({ L: 0.62, C: 0.05 * mixAmt + 0.02, h: tint.h });
      p.g3 = C.lchToHex({ L: 0.16, C: 0.03 * mixAmt + 0.01, h: (tint.h + 200) % 360 });
      p.g4 = C.lchToHex({ L: 0.3, C: 0.07 * mixAmt, h: art.accent2h });
    }
  }
  // Text must stay readable whatever was chosen above.
  p.fg = C.ensureContrast(p.fg, p.bg, 7);
  const fg2 = C.ensureContrast(C.mixHex(p.fg, p.bg, 0.36), p.bg, 4.5);
  const fg3 = C.ensureContrast(C.mixHex(p.fg, p.bg, 0.6), p.bg, 3);
  const line = C.mixHex(p.fg, p.bg, 0.84);
  const accentText = C.ensureContrast(p.accent, p.bg, 4.5);
  const accent2 = mode === 'art' && art?.grey ? C.lchToHex({ L: clamp(scheme === 'dark' ? art.neutral - 0.2 : clamp(1.06 - art.neutral, 0.12, 0.45) + 0.2, 0.1, 0.95), C: 0, h: 0 })
    : mode === 'art' && art ? art.accent2 : C.rotateHue(p.accent, 32);
  return { ...p, scheme, fg2, fg3, line, accent2, accentText, onAccent: C.onColor(p.accent), mode };
}

let artToken = 0;

/** Reads colours from the playing cover (or clears them when there's none). */
export async function updateArt(url) {
  const mine = ++artToken;
  let art = null;
  if (url) {
    try { art = await extractPalette(url); } catch (e) { console.debug('no cover colours', e); }
  }
  if (mine !== artToken) return;
  state.art = art;
  emit('art');
  applyTheme();
}

function loadImage(url) {
  return new Promise((resolve, reject) => {
    const img = new Image();
    img.crossOrigin = 'anonymous';
    img.onload = () => resolve(img);
    img.onerror = () => reject(new Error('cover failed to load'));
    img.src = url;
  });
}

const artCache = new Map();

export async function extractPalette(url) {
  if (artCache.has(url)) return artCache.get(url);
  const img = await loadImage(url);
  const N = 40;
  const cv = document.createElement('canvas');
  cv.width = cv.height = N;
  const g = cv.getContext('2d', { willReadFrequently: true });
  g.drawImage(img, 0, 0, N, N);
  const data = g.getImageData(0, 0, N, N).data;
  const pts = [];
  let opaque = 0, black = 0, white = 0; // pure black and white are not colours, but they say what the cover is
  for (let i = 0; i < data.length; i += 4) {
    if (data[i + 3] < 200) continue;
    opaque++;
    const lab = C.rgbToOklab([data[i], data[i + 1], data[i + 2]]);
    if (lab[0] < 0.07) { black++; continue; }
    if (lab[0] > 0.97) { white++; continue; }
    pts.push(lab);
  }
  const ex = { opaque, black, white };
  const result = opaque < 20 ? null : pts.length < 20 ? neutral(white > 0 ? 0.96 : 0.9, ex) : summarise(cluster(pts, 6), ex);
  artCache.set(url, result);
  if (artCache.size > 40) artCache.delete(artCache.keys().next().value);
  return result;
}

function cluster(pts, k) {
  const sorted = [...pts].sort((a, b) => a[0] - b[0]);
  let cents = Array.from({ length: k }, (_, i) => [...sorted[Math.floor(((i + 0.5) / k) * sorted.length)]]);
  let counts = new Array(k).fill(0);
  for (let it = 0; it < 8; it++) {
    const sums = cents.map(() => [0, 0, 0]);
    counts = new Array(k).fill(0);
    for (const p of pts) {
      let best = 0, bd = Infinity;
      for (let j = 0; j < k; j++) {
        const d = (p[0] - cents[j][0]) ** 2 + (p[1] - cents[j][1]) ** 2 + (p[2] - cents[j][2]) ** 2;
        if (d < bd) { bd = d; best = j; }
      }
      counts[best]++;
      sums[best][0] += p[0]; sums[best][1] += p[1]; sums[best][2] += p[2];
    }
    cents = cents.map((c, j) => (counts[j] ? sums[j].map((s) => s / counts[j]) : c));
  }
  return cents.map((c, j) => ({ lab: c, n: counts[j] })).filter((c) => c.n > 0);
}

/** A cover with no colour in it (black, white, greys): the accent is a grey or white, not the skin's. */
function neutral(L, ex) {
  return { dominant: { L: ex.white > ex.black ? 0.95 : 0.12, C: 0, h: 0 }, accent: null, accent2: null, accent2h: 20, grey: true, neutral: clamp(L, 0.62, 0.97) };
}

function summarise(cl, ex = { opaque: 1, black: 0, white: 0 }) {
  const total = cl.reduce((s, c) => s + c.n, 0);
  const lch = (lab) => ({ L: lab[0], C: Math.hypot(lab[1], lab[2]), h: ((Math.atan2(lab[2], lab[1]) * 180) / Math.PI + 360) % 360 });
  const items = cl.map((c) => ({ ...lch(c.lab), share: c.n / total }));
  const dominant = [...items].sort((a, b) => b.share - a.share)[0];
  // A good accent is vivid, mid-light and present in the picture, in that order of importance.
  // (Colour that covers only a speck of the picture is not its colour: such a cover counts as grey.)
  const score = (c) => (c.C + 0.02) * c.share ** 0.75 * (1 - Math.abs(c.L - 0.68) * 0.9);
  const ranked = [...items].filter((c) => c.share >= 0.05).sort((a, b) => score(b) - score(a));
  const vivid = !!ranked[0] && ranked[0].C > 0.045;
  const pick = vivid ? ranked[0] : null;
  const hueGap = (a, b) => { const d = Math.abs(a - b) % 360; return d > 180 ? 360 - d : d; };
  const second = pick ? ranked.find((c) => c !== pick && c.C > 0.04 && hueGap(c.h, pick.h) > 35) : null;
  const accentLch = pick ? { L: clamp(pick.L, 0.58, 0.8), C: Math.min(pick.C * 1.15, 0.2), h: pick.h } : null;
  const accent = accentLch ? C.lchToHex(accentLch) : null;
  const acc2 = second ? C.lchToHex({ L: clamp(second.L, 0.55, 0.8), C: Math.min(second.C * 1.1, 0.18), h: second.h }) : accent ? C.rotateHue(accent, 40) : null;
  if (!accent) {
    // A grey or monochrome cover: the accent is as light as the lighter parts of the picture (white for
    // black-and-white art), and nothing is tinted.
    const lights = items.filter((c) => c.share >= 0.06).map((c) => c.L);
    const whiteShare = ex.white / Math.max(1, ex.opaque);
    const lightest = whiteShare >= 0.2 ? 0.96 : Math.max(...lights, dominant.L);
    return { ...neutral(lightest, ex), dominant: { L: dominant.L, C: 0, h: dominant.h } };
  }
  return { dominant, accent, accent2: acc2, accent2h: C.hexToLch(acc2).h, grey: false };
}

/** Applies skin, colours, type and shape to the document. */
export function applyTheme() {
  const prefs = getPrefs();
  const skin = getSkin(prefs.skin);
  const c = cfg();
  let art = state.art;
  const t = computeTokens(c, skin, art);
  Object.assign(tokens, { art2: art?.accent ?? t.accent, art3: art?.accent2 ?? t.accent2, accent: t.accent, bg: t.bg, fg: t.fg, scheme: t.scheme, accent2: t.accent2, fg2: t.fg2, line: t.line, bg2: t.bg2, bg3: t.bg3 });

  const root = document.documentElement;
  root.dataset.skin = skin.cssId ?? skin.id;
  root.dataset.scheme = t.scheme;
  root.dataset.colorMode = t.mode;
  const reduce = c.motion.reduce === 'system' ? matchMedia('(prefers-reduced-motion: reduce)').matches : c.motion.reduce === true || c.motion.reduce === 'on';
  root.dataset.motion = reduce ? 'reduced' : 'full';
  root.dataset.decor = c.decor ? 'on' : 'off';
  root.dataset.density = c.shape.density;
  root.dataset.glitch = c.hero.glitch ? 'on' : 'off';
  root.dataset.flip = c.layout.flip ? 'on' : 'off';

  const vars = {
    '--bg': t.bg, '--bg2': t.bg2, '--bg3': t.bg3, '--fg': t.fg, '--fg2': t.fg2, '--fg3': t.fg3, '--line': t.line,
    '--accent': t.accent, '--accent2': t.accent2, '--accent-text': t.accentText, '--on-accent': t.onAccent,
    '--accent-rgb': C.parseHex(t.accent).join(','), '--fg-rgb': C.parseHex(t.fg).join(','), '--bg-rgb': C.parseHex(t.bg).join(','),
    '--glow': String(c.color.glow),
    '--art1': art ? C.lchToHex({ ...art.dominant, L: clamp(art.dominant.L, 0.25, 0.7), C: art.grey ? 0 : Math.max(art.dominant.C, 0.05) }) : t.accent,
    '--art2': art?.accent ?? t.accent, '--art3': art?.accent2 ?? t.accent2,
    '--font-display': fontStack(c.type.display ?? skin.fonts.display),
    '--font-body': fontStack(c.type.body ?? skin.fonts.body),
    '--font-mono': fontStack(c.type.mono ?? skin.fonts.mono),
    '--type-scale': String(c.type.scale), '--tracking': `${c.type.tracking}em`,
    '--density': String(DENSITY[c.shape.density] ?? 1),
    '--bw': `${c.shape.border}px`, '--shadow-k': String(c.shape.shadow), '--glass': String(c.shape.glass), '--grain': String(c.shape.grain ?? 0),
    '--size-left': `${Math.max(180, c.layout.sizes.left || 0)}px`, '--size-right': `${Math.max(180, c.layout.sizes.right || 0)}px`, // a side column can't be narrower than a panel needs
    '--size-top': `${c.layout.sizes.top}px`, '--size-bottom': `${c.layout.sizes.bottom}px`,
  };
  if (c.shape.radius != null) { vars['--radius'] = `${c.shape.radius}px`; }
  else root.style.removeProperty('--radius');
  for (const [k, v] of Object.entries(t)) if (['paper', 'ink', 'g1', 'g2', 'g3', 'g4'].includes(k)) vars[`--${k}`] = v;
  for (const [k, v] of Object.entries(vars)) root.style.setProperty(k, v);
  root.dataset.fx = getPrefs().perf?.lite ? 'lite' : 'full';
  root.dataset.glass = c.shape.glass > 0 ? 'on' : 'off';
  emit('theme', t);
}

on('ui', applyTheme);
matchMedia('(prefers-reduced-motion: reduce)').addEventListener?.('change', applyTheme);
