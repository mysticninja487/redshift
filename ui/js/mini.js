// The mini player: a small always-on-top window, started with `?mini=1`. It has two sizes: a compact
// strip (cover, track, controls, progress) and an expanded card (the queue or the lyrics beside it).

import { api, COVER, coverUrl } from './api.js';
import { on } from './bus.js';
import { flushPrefs, getPrefs, setPref } from './prefs.js';
import { act, getMeta } from './controller.js';
import { duration, isPlaying, position, state } from './state.js';
import { debounce, fmtTime, h } from './util.js';
import { icon } from './ui/icons.js';

const win = () => window.__TAURI__?.window?.getCurrentWindow?.();
const KEY = 'redshift-mini-expanded';
const savedExpanded = () => { try { return localStorage.getItem(KEY) === '1'; } catch { return false; } };
const saveExpanded = (v) => { try { localStorage.setItem(KEY, v ? '1' : '0'); } catch { /* private window: it just starts compact next time */ } };

// The size the person gave each layout by dragging the corner, so it comes back at that size.
const SIZES = 'redshift-mini-sizes';
const savedSize = (expanded) => { try { const v = JSON.parse(localStorage.getItem(SIZES))?.[expanded ? 'full' : 'compact']; return Array.isArray(v) && v.every(Number.isFinite) ? v : null; } catch { return null; } };
const saveSize = (expanded, size) => { try { const all = JSON.parse(localStorage.getItem(SIZES)) ?? {}; localStorage.setItem(SIZES, JSON.stringify({ ...all, [expanded ? 'full' : 'compact']: size })); } catch { /* it just opens at the default size */ } };
const miniPrefs = () => getPrefs().mini ?? {};

const UP_NEXT = 40;
const REPEAT_NEXT = { off: 'all', all: 'one', one: 'off' };

/** Index of the lyric line being sung now (-1 before the first line, or when the lyrics aren't synced). */
function currentIndex() {
  const ly = state.lyrics;
  if (!ly?.synced) return -1;
  const t = Math.round(position() * 1000);
  let lo = 0, hi = ly.lines.length;
  while (lo < hi) { const mid = (lo + hi) >> 1; if (ly.lines[mid].time_ms <= t) lo = mid + 1; else hi = mid; }
  return lo - 1;
}

/** A thin progress bar you can click or drag. */
function seekBar(cls) {
  const fill = h('i');
  const el = h('div', { class: `mini-seek ${cls}`, role: 'slider', 'aria-label': 'Position', tabindex: '0' }, fill);
  let dragging = false;
  const frac = (e) => { const r = el.getBoundingClientRect(); return Math.max(0, Math.min(1, (e.clientX - r.left) / r.width)); };
  el.addEventListener('pointerdown', (e) => { if (e.button !== 0) return; dragging = true; el.setPointerCapture(e.pointerId); fill.style.width = `${frac(e) * 100}%`; });
  el.addEventListener('pointermove', (e) => { if (dragging) fill.style.width = `${frac(e) * 100}%`; });
  el.addEventListener('pointerup', (e) => { if (!dragging) return; dragging = false; const d = duration(); if (d) act(api.seek(frac(e) * d)); });
  el.addEventListener('pointercancel', () => { dragging = false; });
  el.addEventListener('keydown', (e) => { if (e.key === 'ArrowRight') act(api.seek(position() + 5)); else if (e.key === 'ArrowLeft') act(api.seek(Math.max(0, position() - 5))); });
  el.set = (f) => { if (!dragging) fill.style.width = `${f * 100}%`; };
  return el;
}

export function mountMini(root) {
  const btn = (name, label, fn, cls = '', size = 18) => h('button', { class: `mini-btn ${cls}`, type: 'button', title: label, 'aria-label': label, onclick: fn }, icon(name, size));

  const togglePin = () => setPref('mini.onTop', !(miniPrefs().onTop !== false));
  const cPin = btn('pin', 'Keep on top', togglePin, 'mini-pin', 16), fPin = btn('pin', 'Keep on top', togglePin, 'mini-ghost mini-pin', 16);

  // ---- compact strip ----
  const cTitle = h('div', { class: 'mini-title' }), cArtist = h('div', { class: 'mini-artist' });
  const cPlay = btn('play', 'Play', () => act(api.togglePause()), 'mini-play', 20);
  const cSeek = seekBar('mini-progress');
  const compact = h('div', { class: 'mini-compact' },
    h('div', { class: 'mini-cover' }),
    h('div', { class: 'mini-info' }, h('div', { class: 'mini-meta' }, cTitle, cArtist),
      h('div', { class: 'mini-controls' }, btn('prev', 'Previous', () => act(api.previous())), cPlay, btn('next', 'Next', () => act(api.next())))),
    h('div', { class: 'mini-corner' }, cPin, btn('expand', 'Expand', () => setExpanded(true))),
    cSeek);

  // ---- expanded card ----
  const fTitle = h('div', { class: 'mini-title' }), fArtist = h('div', { class: 'mini-artist' });
  const fPlay = btn('play', 'Play', () => act(api.togglePause()), 'mini-play mini-play-lg', 22);
  const fSeek = seekBar('mini-bar');
  const mute = btn('volume', 'Mute', () => act(api.setMuted(!state.player.muted)), 'mini-ghost');
  const shuffle = btn('shuffle', 'Shuffle', () => act(setShuffle(!state.queue.shuffle)));
  const repeat = btn('repeat', 'Repeat', () => act(setRepeat(REPEAT_NEXT[state.queue.repeat] ?? 'off')));
  const lyricsBtn = btn('lyrics', 'Show lyrics', () => setList(list === 'lyrics' ? 'queue' : 'lyrics'));
  const listEl = h('div', { class: 'mini-list', role: 'list' });
  const full = h('div', { class: 'mini-full' },
    h('div', { class: 'mini-left' },
      h('div', { class: 'mini-cover' }),
      h('div', { class: 'mini-tools' }, fPin, mute, btn('pip', 'Back to the full player', () => api.toggleMiniPlayer(), 'mini-ghost'), btn('collapse', 'Collapse', () => setExpanded(false), 'mini-ghost')),
      h('div', { class: 'mini-over' }, h('div', { class: 'mini-meta' }, fTitle, fArtist),
        h('div', { class: 'mini-line' }, btn('prev', 'Previous', () => act(api.previous()), 'mini-ghost'), fSeek, btn('next', 'Next', () => act(api.next()), 'mini-ghost')))),
    h('div', { class: 'mini-right' }, listEl,
      h('div', { class: 'mini-foot' }, shuffle, fPlay, repeat, h('span', { class: 'mini-spacer' }), lyricsBtn)));

  const grip = h('div', { class: 'mini-grip', title: 'Drag to resize', 'aria-hidden': 'true', onpointerdown: (e) => { if (e.button !== 0) return; e.preventDefault(); win()?.startResizeDragging?.('SouthEast'); } });
  const mini = h('div', { class: 'mini', dataset: { mode: 'compact' } }, h('div', { class: 'mini-bg' }), compact, full, grip);
  root.append(mini);

  let expanded = false, list = 'queue', listKey = '', lyricIdx = -2;

  function setExpanded(v, remember = true) {
    expanded = v;
    mini.dataset.mode = v ? 'full' : 'compact';
    if (remember) saveExpanded(v);
    act(api.miniSize(v, savedSize(v)));
    if (v) { listKey = ''; paintList(true); }
  }
  function setList(which) {
    list = which; listKey = '';
    lyricsBtn.setAttribute('aria-pressed', String(which === 'lyrics'));
    lyricsBtn.title = lyricsBtn.ariaLabel = which === 'lyrics' ? 'Show the queue' : 'Show lyrics';
    paintList(true);
  }

  // ---- the person's choices (Settings > Mini player) ----
  let lastTop = null, lastList = null;
  function applyPrefs() {
    const m = miniPrefs();
    const d = mini.dataset;
    d.bg = m.bg ?? 'cover'; d.cover = m.cover === false ? 'off' : 'on'; d.artist = m.artist === false ? 'off' : 'on'; d.progress = m.progress === false ? 'off' : 'on';
    mini.style.setProperty('--mini-alpha', String(Math.max(0.3, Math.min(1, (m.opacity ?? 100) / 100))));
    mini.style.setProperty('--mini-radius', `${Math.max(0, Math.min(32, m.radius ?? 16))}px`);
    const top = m.onTop !== false;
    for (const b of [cPin, fPin]) { b.setAttribute('aria-pressed', String(top)); b.title = b.ariaLabel = top ? 'Stop keeping on top' : 'Keep on top'; }
    if (top !== lastTop) { lastTop = top; act(api.miniOnTop(top)); }
    const want = m.list === 'lyrics' ? 'lyrics' : 'queue';
    if (want !== lastList) { lastList = want; if (want !== list) setList(want); }
  }

  // ---- painting ----
  const paintTrack = () => {
    const t = state.track;
    const title = t?.title ?? 'Nothing playing';
    const artist = t ? [t.artist, t.album].filter(Boolean).join(' · ') : 'Open the full player to pick something';
    cTitle.textContent = fTitle.textContent = title;
    cArtist.textContent = fArtist.textContent = artist;
    const img = state.coverUrl ? `url("${state.coverUrl}")` : 'none';
    mini.style.setProperty('--cover', img);
    mini.classList.toggle('no-cover', !state.coverUrl);
    document.title = t ? `${t.title} · Redshift` : 'Redshift';
  };
  const paintPlay = () => {
    const p = isPlaying();
    for (const [b, size] of [[cPlay, 20], [fPlay, 22]]) { b.replaceChildren(icon(p ? 'pause' : 'play', size)); b.title = b.ariaLabel = p ? 'Pause' : 'Play'; }
  };
  const paintModes = () => {
    shuffle.setAttribute('aria-pressed', String(!!state.queue.shuffle));
    const r = state.queue.repeat;
    repeat.replaceChildren(icon(r === 'one' ? 'repeatOne' : 'repeat', 18));
    repeat.setAttribute('aria-pressed', String(r !== 'off'));
    const m = state.player.muted;
    mute.replaceChildren(icon(m ? 'mute' : 'volume', 18)); mute.title = mute.ariaLabel = m ? 'Unmute' : 'Mute';
  };

  /** The right-hand list: what plays next, or the lyrics around the current line. */
  function paintList(force = false) {
    if (!expanded) return;
    if (list === 'lyrics') return paintLyrics(force);
    const entries = state.queue.entries, cur = entries.findIndex((e) => e.id === state.queue.current_entry);
    const next = entries.slice(cur + 1, cur + 1 + UP_NEXT);
    const key = `q:${state.queue.current_entry}:${next.map((e) => e.id).join(',')}`;
    if (!force && key === listKey) return;
    listKey = key;
    listEl.classList.remove('is-lyrics');
    if (!next.length) { listEl.replaceChildren(h('div', { class: 'mini-empty' }, entries.length ? 'Nothing after this' : 'The queue is empty')); return; }
    listEl.replaceChildren(...next.map((e) => {
      const name = h('span', { class: 'mini-row-title' }, e.path.split(/[\\/]/).pop().replace(/\.[^.]+$/, ''));
      const sub = h('span', { class: 'mini-row-artist' });
      getMeta(e.path).then((m) => { if (m?.title) name.textContent = m.title; if (m?.artist) sub.textContent = m.artist; });
      return h('button', { class: 'mini-row', type: 'button', role: 'listitem', onclick: () => act(api.playEntry(e.id)) },
        h('span', { class: 'mini-thumb', style: { backgroundImage: `url("${coverUrl(e.path, COVER.row)}")` } }), h('span', { class: 'mini-row-text' }, name, sub));
    }));
    listEl.scrollTop = 0;
  }
  function paintLyrics(force) {
    const ly = state.lyrics, idx = currentIndex();
    const key = `l:${state.trackPath}:${ly ? ly.lines.length : 'none'}`;
    if (key !== listKey || force) {
      listKey = key; lyricIdx = -2;
      listEl.classList.add('is-lyrics');
      if (!ly) { listEl.replaceChildren(h('div', { class: 'mini-empty' }, 'No lyrics for this track')); return; }
      listEl.replaceChildren(...ly.lines.map((ln, i) => h('div', { class: `mini-ly${ln.text ? '' : ' gap'}`, dataset: { i }, onclick: ly.synced && ln.text ? () => act(api.seek(ln.time_ms / 1000)) : null }, ln.text || ' ')));
    }
    if (!ly?.synced || idx === lyricIdx) return;
    lyricIdx = idx;
    const kids = listEl.children;
    for (let i = 0; i < kids.length; i++) { kids[i].classList.toggle('active', i === idx); kids[i].dataset.d = String(Math.min(3, Math.abs(i - idx))); }
    const a = kids[idx];
    if (a) listEl.scrollTo({ top: a.offsetTop - listEl.clientHeight / 2 + a.offsetHeight / 2, behavior: 'smooth' });
  }
  const tick = () => {
    const d = duration() || 0, p = position(), f = d ? Math.min(1, p / d) : 0;
    cSeek.set(f); fSeek.set(f);
    if (expanded && list === 'lyrics') paintLyrics(false);
  };

  on('track', () => { paintTrack(); paintList(); tick(); });
  on('art', paintTrack);
  on('lyrics', () => { paintList(list === 'lyrics'); tick(); });
  on('queue', () => { paintModes(); paintList(); });
  on('player', () => { paintModes(); tick(); });
  on('status', paintPlay);
  on('ui', applyPrefs);
  applyPrefs();
  paintTrack(); paintPlay(); paintModes(); tick();
  setInterval(tick, 250);
  const open = miniPrefs().open ?? 'last';
  if (open === 'expanded' || (open === 'last' && savedExpanded())) setExpanded(true, false);
  else if (savedSize(false)) act(api.miniSize(false, savedSize(false)));
  // Remember the size the corner drag leaves it at.
  addEventListener('resize', debounce(() => { if (innerWidth > 100 && innerHeight > 60) saveSize(expanded, [Math.round(innerWidth), Math.round(innerHeight)]); }, 400));
  addEventListener('pagehide', flushPrefs);

  addEventListener('keydown', (e) => {
    if (e.code === 'Space' && !e.target.closest?.('button')) { e.preventDefault(); act(api.togglePause()); }
    else if (e.key === 'Escape' && expanded) setExpanded(false);
  });
  // Drag the window by any part that isn't a control.
  const interactive = 'button, .mini-seek, .mini-ly, .mini-grip';
  root.addEventListener('pointerdown', (e) => { if (e.button === 0 && !e.target.closest(interactive)) win()?.startDragging?.(); });
  root.addEventListener('dblclick', (e) => { if (!e.target.closest(interactive)) api.toggleMiniPlayer(); });
}

async function setShuffle(on) { state.queue.shuffle = on; return api.setShuffle(on); }
async function setRepeat(mode) { state.queue.repeat = mode; return api.setRepeat(mode); }
