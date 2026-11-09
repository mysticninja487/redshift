// Drives every visualiser canvas on screen from one stream of analyser frames.
//
// One loop asks the backend for the latest frame (never more than one request in flight), keeps
// short histories (spectra, levels) and a beat detector, then lets each mounted canvas draw. When
// nothing visible needs it, the window is hidden, or playback is paused and the picture has settled,
// the loop stops completely; anything that could change what's drawn wakes it again.

import { api } from '../api.js';
import { on } from '../bus.js';
import { cfg, getPrefs } from '../prefs.js';
import { duration, position, isPlaying, state } from '../state.js';
import { tokens } from '../theme.js';
import { clamp } from '../util.js';
import { getViz, vizOptions } from './index.js';

const EMPTY = { bars: new Array(64).fill(0), peaks: new Array(64).fill(0), scope_left: new Array(256).fill(0), scope_right: new Array(256).fill(0), rms_left: 0, rms_right: 0, peak_left: 0, peak_right: 0, bass: 0 };

const HIST = 160;
const shared = {
  frame: EMPTY, spec: [], levels: [], bass: 0, bassAvg: 0, beat: 0, energy: 0, t: 0, dt: 1 / 60,
  coverImg: null,
};
const targets = new Set();
const watchers = new Set();
let running = false, inFlight = false, last = 0, lastFrameAt = 0, calmFrames = 0;

on('track', () => {
  shared.coverImg = null;
  if (!state.coverUrl) return;
  const url = state.coverUrl, img = new Image();
  img.crossOrigin = 'anonymous';
  img.onload = () => { if (state.coverUrl === url) shared.coverImg = img; };
  img.src = url;
});

function ingest(f) {
  shared.frame = f;
  shared.spec.push(Float32Array.from(f.bars));
  if (shared.spec.length > HIST) shared.spec.shift();
  const lvl = (f.rms_left + f.rms_right) / 2;
  shared.levels.push(lvl);
  if (shared.levels.length > 600) shared.levels.shift();
  // Smoothed bass and a simple onset detector: a jump well above the recent average is a beat.
  shared.bass += (f.bass - shared.bass) * 0.5;
  if (f.bass > shared.bassAvg * 1.35 + 0.06 && shared.beat < 0.4) shared.beat = 1;
  shared.bassAvg += (f.bass - shared.bassAvg) * 0.06;
  shared.energy += (lvl - shared.energy) * 0.2;
}

function paintFor(ctx, mode) {
  return (x0, y0, x1, y1) => {
    switch (mode) {
      case 'fg': return tokens.fg;
      case 'gradient': { const g = ctx.createLinearGradient(x0, y0, x1, y1); g.addColorStop(0, tokens.accent); g.addColorStop(1, tokens.accent2); return g; }
      case 'art': { const g = ctx.createLinearGradient(x0, y0, x1, y1); g.addColorStop(0, tokens.art2); g.addColorStop(1, tokens.art3); return g; }
      case 'rainbow': { const g = ctx.createLinearGradient(x0, y0, x1, y1); for (let i = 0; i <= 6; i++) g.addColorStop(i / 6, `hsl(${i * 55}, 85%, 62%)`); return g; }
      default: return tokens.accent;
    }
  };
}

function draw(t) {
  const { canvas, ctx } = t;
  if (!t.visible) return;
  const viz = getViz(typeof t.id === 'function' ? t.id() : t.id);
  if (!viz) return;
  const rect = canvas.getBoundingClientRect();
  const dpr = Math.min(window.devicePixelRatio || 1, getPrefs().perf?.lite ? 1 : 2); // Reduce effects draws at 1x: a quarter of the pixels on a 2x screen
  const w = Math.max(1, Math.round(rect.width)), h = Math.max(1, Math.round(rect.height));
  if (canvas.width !== Math.round(w * dpr) || canvas.height !== Math.round(h * dpr)) { canvas.width = Math.round(w * dpr); canvas.height = Math.round(h * dpr); }
  canvas.style.mixBlendMode = viz.blend ?? '';
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  if (!viz.persist) ctx.clearRect(0, 0, w, h);
  const c = cfg();
  const e = {
    ...shared, w, h, tokens, ctx, dpr, playing: isPlaying(), position: position(), duration: duration(), waveform: state.waveform,
    colorMode: c.viz.color, paint: paintFor(ctx, c.viz.color), store: t.store, scheme: tokens.scheme,
  };
  ctx.save();
  try { viz.draw(ctx, e, vizOptions(viz.id, (typeof t.opts === 'function' ? t.opts() : t.opts) ?? {})); } catch (err) { if (!t.warned) { console.error(`visualiser "${viz.id}" failed`, err); t.warned = true; } }
  ctx.restore();
}

/** Frame rate to aim for: the chosen one, less when nobody is looking at the window closely. */
function targetFps() {
  const perf = getPrefs().perf ?? {};
  let fps = clamp(cfg().viz.fps || 60, 15, 144);
  if (perf.lite) fps = Math.min(fps, 30);
  if (perf.background !== false && !document.hasFocus()) fps = Math.min(fps, 20);
  return fps;
}

/** True when something visible is drawing, or a watcher wants frames. */
const wanted = () => watchers.size > 0 || [...targets].some((t) => t.visible);

function loop(now) {
  if (!wanted() || document.hidden) { running = false; return; }
  // Paused with every bar back at zero: the last frame drawn is the final one.
  if (!isPlaying() && calmFrames > 20) { running = false; return; }
  const fps = targetFps();
  if (now - last < 1000 / fps - 3) { requestAnimationFrame(loop); return; }
  shared.dt = clamp((now - last) / 1000, 0.001, 0.1);
  last = now;
  shared.t += shared.dt;
  shared.beat *= Math.pow(0.001, shared.dt / 0.35); // fades out over roughly a third of a second
  if (!inFlight && isPlaying()) {
    inFlight = true;
    api.vizFrame().then((f) => { lastFrameAt = performance.now(); ingest(f); }).catch(() => {}).finally(() => { inFlight = false; });
  }
  // Without a fresh frame for a while (paused, or the backend is busy) let everything settle to zero.
  if (performance.now() - lastFrameAt > 400) ingest({ ...EMPTY, bars: shared.frame.bars.map((v) => v * 0.9) });
  for (const t of targets) draw(t);
  for (const w of watchers) w(shared);
  const still = !isPlaying() && shared.beat < 0.01 && shared.frame.bars.every((v) => v < 0.004);
  calmFrames = still ? calmFrames + 1 : 0;
  requestAnimationFrame(loop);
}

function start() {
  calmFrames = 0;
  if (running) return;
  running = true;
  last = performance.now();
  requestAnimationFrame(loop);
}
document.addEventListener('visibilitychange', () => { if (!document.hidden) start(); });
// Whatever could change the picture wakes a stopped loop: play, a new track, a changed setting or
// theme, a resize, the window coming back into focus.
for (const topic of ['status', 'track', 'ui', 'theme', 'art']) on(topic, () => { if (wanted()) start(); });
addEventListener('resize', () => { if (wanted()) start(); });
addEventListener('focus', () => { if (wanted()) start(); });

/** Starts drawing a visualiser into `canvas`. `id` and `opts` are functions, read every frame. */
export function mountViz(canvas, { id, opts }) {
  const t = { canvas, ctx: canvas.getContext('2d'), id, opts, store: {}, visible: true };
  const io = new IntersectionObserver((entries) => { t.visible = entries.some((en) => en.isIntersecting); if (t.visible) start(); });
  io.observe(canvas);
  targets.add(t);
  start();
  return { destroy() { targets.delete(t); io.disconnect(); }, reset() { t.store = {}; } };
}

/**
 * Calls `fn(live)` every tick with the shared analysis (`bass`, `beat`, `energy`, `t`, `frame`)
 * without drawing anything; for effects built from CSS. Returns a stop function.
 */
export function watchLive(fn) {
  watchers.add(fn);
  start();
  return () => watchers.delete(fn);
}
