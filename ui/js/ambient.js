// Ambient mode: the whole window becomes one thing to look at, the visualiser, the lyrics, or the
// lyrics over the visualiser. Controls fade away when the mouse is still. Esc leaves.

import { api } from './api.js';
import { on } from './bus.js';
import { act } from './controller.js';
import { DEFAULT_AMBIENT, get, getPrefs, setPref } from './prefs.js';
import { duration, isPlaying, position, state } from './state.js';
import { h, toast } from './util.js';
import { icon } from './ui/icons.js';
import { VIZ, getViz } from './viz/index.js';
import { mountViz } from './viz/engine.js';

export const AMBIENT_MODES = [{ value: 'viz', label: 'Visualiser' }, { value: 'lyrics', label: 'Lyrics' }, { value: 'both', label: 'Both' }];

const ambientPrefs = () => ({ ...DEFAULT_AMBIENT, ...(getPrefs().ambient ?? {}) });
let current = null;

export const ambientOpen = () => !!current;
export function toggleAmbient(mode) { return current ? (mode && mode !== current.mode() ? current.setMode(mode) : closeAmbient()) : openAmbient(mode); }
export function closeAmbient() { current?.close(); }

/** The visualiser ambient mode draws: the person's pick, or whatever the current skin would show. */
function vizId() {
  const p = ambientPrefs().viz;
  if (p && p !== 'auto' && getViz(p)) return p;
  return get('hero.mode') === 'viz' ? get('hero.viz') : get('viz.main');
}

export async function openAmbient(mode) {
  if (current) return;
  const app = document.getElementById('app');
  if (!app) return;
  const prefs = ambientPrefs();
  let shown = mode ?? prefs.mode, vizHandle = null, lyricsHandle = null, hideTimer = 0, flashTimer = 0, lock = null, enteredFullscreen = false, closed = false;

  // ---- parts ----
  const bg = h('div', { class: 'am-bg', 'aria-hidden': 'true' });
  const vizHost = h('div', { class: 'am-viz', 'aria-hidden': 'true' });
  const lyricsHost = h('div', { class: 'panel am-lyrics' });
  const noLyrics = h('div', { class: 'am-nolyrics' });
  const title = h('div', { class: 'am-title' }), artist = h('div', { class: 'am-artist' });
  const info = h('div', { class: 'am-info' }, title, artist);
  const fill = h('i');
  const seek = h('div', { class: 'am-seek', role: 'slider', 'aria-label': 'Position', tabindex: '-1' }, fill);
  const btn = (name, label, fn, cls = '') => h('button', { class: `am-btn ${cls}`, type: 'button', title: label, 'aria-label': label, onclick: fn }, icon(name, 20));
  const modeBtns = AMBIENT_MODES.map((m) => h('button', { class: 'am-mode', type: 'button', dataset: { mode: m.value }, onclick: () => setMode(m.value) }, m.label));
  const play = btn('play', 'Play', () => act(api.togglePause()), 'am-play');
  const bar = h('div', { class: 'am-bar' },
    h('div', { class: 'am-modes', role: 'group', 'aria-label': 'Ambient view' }, modeBtns),
    h('div', { class: 'am-transport' }, btn('prev', 'Previous', () => act(api.previous())), play, btn('next', 'Next', () => act(api.next()))),
    h('div', { class: 'am-end' }, btn('viz', 'Next visualiser (V)', nextViz, 'am-vizbtn'), btn('close', 'Leave ambient mode (Esc)', () => close())));
  const root = h('div', { class: 'ambient', dataset: { mode: shown, bg: prefs.bg, title: prefs.title, idle: 'false', flash: 'false' }, role: 'region', 'aria-label': 'Ambient mode' },
    bg, vizHost, lyricsHost, noLyrics, info, seek, bar);
  root.style.setProperty('--am-lyrics-size', String(prefs.lyricsSize));

  // ---- content ----
  const paintTrack = () => {
    const t = state.track;
    title.textContent = t?.title ?? 'Nothing playing';
    artist.textContent = t ? [t.artist, t.album].filter(Boolean).join(' · ') : '';
    root.classList.toggle('no-track', !t);
    paintNoLyrics();
  };
  const paintArt = () => root.style.setProperty('--am-cover', state.coverUrl ? `url("${state.coverUrl}")` : 'none');
  const paintNoLyrics = () => {
    const missing = !state.lyrics && state.track;
    noLyrics.textContent = missing ? 'No lyrics for this track' : '';
    root.classList.toggle('lyrics-missing', !!missing);
  };
  const paintPlay = () => { const p = isPlaying(); play.replaceChildren(icon(p ? 'pause' : 'play', 24)); play.title = play.ariaLabel = p ? 'Pause' : 'Play'; };
  const tick = () => { const d = duration() || 0; fill.style.width = d ? `${Math.min(100, (position() / d) * 100)}%` : '0%'; };

  async function mountParts() {
    const wantViz = shown === 'viz' || shown === 'both', wantLyrics = shown === 'lyrics' || shown === 'both';
    if (wantViz && !vizHandle) {
      const cv = h('canvas', { class: 'viz-canvas am-canvas' });
      vizHost.replaceChildren(cv);
      vizHandle = mountViz(cv, { id: vizId });
    } else if (!wantViz && vizHandle) { vizHandle.destroy(); vizHandle = null; vizHost.replaceChildren(); }
    if (wantLyrics && !lyricsHandle) {
      const { mount } = await import('./panels/lyrics.js');
      if (closed || !(shown === 'lyrics' || shown === 'both') || lyricsHandle) return;
      lyricsHandle = mount(lyricsHost);
    } else if (!wantLyrics && lyricsHandle) { lyricsHandle.destroy(); lyricsHandle = null; lyricsHost.replaceChildren(); }
  }
  function setMode(m) {
    if (!AMBIENT_MODES.some((x) => x.value === m)) return;
    shown = m;
    root.dataset.mode = m;
    modeBtns.forEach((b) => b.setAttribute('aria-pressed', String(b.dataset.mode === m)));
    setPref('ambient.mode', m);
    mountParts();
  }
  function nextViz() {
    const ids = VIZ.map((v) => v.id), cur = vizId(), n = ids[(ids.indexOf(cur) + 1) % ids.length];
    setPref('ambient.viz', n);
    vizHandle?.reset?.();
    toast(`Visualiser: ${getViz(n).name}`);
    if (shown === 'lyrics') setMode('both');
  }

  // ---- controls fade away when nothing moves ----
  const wake = () => {
    root.dataset.idle = 'false';
    clearTimeout(hideTimer);
    const s = ambientPrefs().hideAfter;
    if (s > 0) hideTimer = setTimeout(() => { root.dataset.idle = 'true'; }, s * 1000);
  };
  const flash = () => { root.dataset.flash = 'true'; clearTimeout(flashTimer); flashTimer = setTimeout(() => { root.dataset.flash = 'false'; }, 5000); };

  // ---- seek ----
  seek.addEventListener('pointerdown', (e) => { const d = duration(); if (e.button !== 0 || !d) return; const r = seek.getBoundingClientRect(); act(api.seek(Math.max(0, Math.min(1, (e.clientX - r.left) / r.width)) * d)); });

  // ---- keys: the few that mean something here; the rest (space, arrows, M) work as usual ----
  const onKey = (e) => {
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    const k = e.key.length === 1 ? e.key.toLowerCase() : e.key;
    const take = () => { e.preventDefault(); e.stopPropagation(); wake(); };
    if (k === 'Escape') { take(); close(); }
    else if (k === '1' || k === '2' || k === '3') { take(); setMode(AMBIENT_MODES[+k - 1].value); }
    else if (k === 'v' && !e.shiftKey) { take(); nextViz(); }
    else if (k === 'f') { take(); }
    else wake();
  };
  const onFullscreen = () => { if (enteredFullscreen && !document.fullscreenElement) { enteredFullscreen = false; close(); } };
  const onVisible = () => { if (document.visibilityState === 'visible' && !closed) hold(); };

  async function hold() { try { lock = (await navigator.wakeLock?.request?.('screen')) ?? null; } catch { lock = null; } } // keeps the screen from sleeping while you watch

  function close() {
    if (closed) return;
    closed = true;
    clearTimeout(hideTimer); clearTimeout(flashTimer);
    removeEventListener('keydown', onKey, true);
    document.removeEventListener('fullscreenchange', onFullscreen);
    document.removeEventListener('visibilitychange', onVisible);
    offs.forEach((f) => f());
    clearInterval(ticker);
    vizHandle?.destroy(); lyricsHandle?.destroy();
    lock?.release?.().catch?.(() => {});
    root.remove();
    delete document.documentElement.dataset.ambient;
    if (enteredFullscreen && document.fullscreenElement) document.exitFullscreen().catch(() => {});
    enteredFullscreen = false;
    current = null;
    // Everything behind was hidden; give panels a nudge to measure themselves again.
    dispatchEvent(new Event('resize'));
  }

  // ---- go ----
  const offs = [on('track', () => { paintTrack(); flash(); }), on('art', paintArt), on('lyrics', paintNoLyrics), on('status', paintPlay), on('ui', () => {
    const p = ambientPrefs();
    root.dataset.bg = p.bg; root.dataset.title = p.title; root.style.setProperty('--am-lyrics-size', String(p.lyricsSize));
    wake();
  })];
  const ticker = setInterval(tick, 400);
  current = { close, setMode, mode: () => shown };
  app.append(root);
  document.documentElement.dataset.ambient = 'true';
  addEventListener('keydown', onKey, true);
  root.addEventListener('pointermove', wake); root.addEventListener('pointerdown', wake);
  document.addEventListener('visibilitychange', onVisible);
  paintTrack(); paintArt(); paintPlay(); tick();
  modeBtns.forEach((b) => b.setAttribute('aria-pressed', String(b.dataset.mode === shown)));
  mountParts(); wake(); flash(); hold();
  if (prefs.fullscreen && !document.fullscreenElement) {
    try {
      await document.documentElement.requestFullscreen();
      if (closed) { document.exitFullscreen().catch(() => {}); return; }
      enteredFullscreen = true; document.addEventListener('fullscreenchange', onFullscreen);
    } catch { /* not allowed here: stay in the window */ }
  }
}
