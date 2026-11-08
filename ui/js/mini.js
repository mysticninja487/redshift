// The mini player: a small always-on-top window with the cover, the track, the current lyric
// line and the basic controls. It's the same page as the full player, started with `?mini=1`.

import { api } from './api.js';
import { on } from './bus.js';
import { act } from './controller.js';
import { duration, isPlaying, position, state } from './state.js';
import { fmtTime, h } from './util.js';
import { icon } from './ui/icons.js';

const win = () => window.__TAURI__?.window?.getCurrentWindow?.();

/** The lyric line being sung now ('' when the track has no synced lyrics). */
function currentLine() {
  const ly = state.lyrics;
  if (!ly?.synced) return '';
  const t = Math.round(position() * 1000);
  let line = '';
  for (const l of ly.lines) { if (l.time_ms > t) break; if (l.text) line = l.text; }
  return line;
}

export function mountMini(root) {
  const art = h('div', { class: 'mini-art' });
  const title = h('div', { class: 'mini-title' }), artist = h('div', { class: 'mini-artist' }), lyric = h('div', { class: 'mini-lyric' });
  const fill = h('i'), seek = h('div', { class: 'mini-seek', role: 'slider', 'aria-label': 'Position', tabindex: '0' }, fill);
  const time = h('span', { class: 'mini-time' });
  const btn = (name, label, fn, cls = '') => h('button', { class: `mini-btn ${cls}`, type: 'button', title: label, 'aria-label': label, onclick: fn }, icon(name, name === 'play' || name === 'pause' ? 20 : 18));
  const play = btn('play', 'Play', () => act(api.togglePause()), 'mini-play');
  const expand = btn('fullscreen', 'Back to the full player', () => api.toggleMiniPlayer(), 'mini-expand');
  root.append(h('div', { class: 'mini' }, art,
    h('div', { class: 'mini-body' }, h('div', { class: 'mini-meta' }, title, artist), lyric,
      h('div', { class: 'mini-row' }, btn('prev', 'Previous', () => act(api.previous())), play, btn('next', 'Next', () => act(api.next())), seek, time, expand))));

  const paintTrack = () => {
    const t = state.track;
    title.textContent = t?.title ?? 'Nothing playing';
    artist.textContent = t ? [t.artist, t.album].filter(Boolean).join(' · ') : 'Open the full player to pick something';
    art.style.backgroundImage = state.coverUrl ? `url("${state.coverUrl}")` : '';
    art.classList.toggle('empty', !state.coverUrl);
    document.title = t ? `${t.title} · Redshift` : 'Redshift';
  };
  const paintPlay = () => { const p = isPlaying(); play.replaceChildren(icon(p ? 'pause' : 'play', 20)); play.title = play.ariaLabel = p ? 'Pause' : 'Play'; };
  const tick = () => {
    const d = duration() || 0, p = position();
    fill.style.width = `${d ? Math.min(100, (p / d) * 100) : 0}%`;
    time.textContent = `${fmtTime(p)} / ${fmtTime(d)}`;
    const line = currentLine();
    if (lyric.textContent !== line) lyric.textContent = line;
    lyric.hidden = !line;
  };
  on('track', () => { paintTrack(); tick(); }); on('art', paintTrack); on('lyrics', tick); on('player', tick); on('status', paintPlay);
  paintTrack(); paintPlay(); tick();
  setInterval(tick, 250);

  seek.addEventListener('pointerdown', (e) => {
    const r = seek.getBoundingClientRect(), d = duration();
    if (d) act(api.seek(Math.max(0, Math.min(1, (e.clientX - r.left) / r.width)) * d));
  });
  seek.addEventListener('keydown', (e) => { if (e.key === 'ArrowRight') act(api.seek(position() + 5)); else if (e.key === 'ArrowLeft') act(api.seek(Math.max(0, position() - 5))); });
  addEventListener('keydown', (e) => { if (e.code === 'Space' && !e.target.closest?.('button')) { e.preventDefault(); act(api.togglePause()); } });
  // Drag the window by any part that isn't a control.
  root.addEventListener('pointerdown', (e) => { if (e.button === 0 && !e.target.closest('button, .mini-seek')) win()?.startDragging?.(); });
  root.addEventListener('dblclick', (e) => { if (!e.target.closest('button, .mini-seek')) api.toggleMiniPlayer(); });
}
