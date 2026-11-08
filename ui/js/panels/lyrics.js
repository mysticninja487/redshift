// Lyrics that follow the song. Synced lyrics highlight the current line (and word, when the file
// has word timing) and click-to-seek; plain lyrics just scroll.

import { api } from '../api.js';
import { act } from '../controller.js';
import { get } from '../prefs.js';
import { isPlaying, position, state } from '../state.js';
import { h } from '../util.js';
import { icon } from '../ui/icons.js';
import { lifecycle } from './common.js';

export function mount(el) {
  const L = lifecycle();
  const scroller = h('div', { class: 'ly-scroll', tabindex: '0', 'aria-label': 'Lyrics' });
  const empty = h('div', { class: 'empty ly-empty' }, icon('lyrics', 28), h('p', { class: 'ly-empty-title' }, 'No lyrics for this track'),
    h('p', { class: 'ly-empty-hint' }, 'Put a .lrc file with the same name next to the song, or embed lyrics in its tags.'));
  const badge = h('div', { class: 'ly-badge' });
  const root = h('div', { class: 'lyrics' }, badge, scroller, empty);
  el.append(root);
  let lines = [], active = -1, userScrollUntil = 0, wordEls = [], wordsOn = get('lyrics.words');

  const sizeVars = () => {
    el.style.setProperty('--ly-size', String(get('lyrics.size')));
    el.style.setProperty('--ly-dim', String(get('lyrics.dim')));
    el.style.setProperty('--ly-blur', `${get('lyrics.blur')}px`);
    el.dataset.align = root.dataset.align = get('lyrics.align'); // the styles key off .lyrics[data-align]
  };

  function build() {
    const ly = state.lyrics;
    scroller.replaceChildren(); lines = []; wordEls = []; active = -1; wordsOn = get('lyrics.words');
    el.dataset.synced = String(!!ly?.synced);
    empty.hidden = !!ly; scroller.hidden = !ly;
    badge.textContent = ly ? (ly.synced ? '' : 'Not synced') : '';
    if (!ly) return;
    ly.lines.forEach((ln, i) => {
      const words = get('lyrics.words') && ln.words?.length ? ln.words.map((w) => h('span', { class: 'ly-w' }, w.text, ' ')) : null;
      const p = h('p', { class: `ly-line${ln.text ? '' : ' gap'}`, dataset: { i } }, words ?? (ln.text || ' '));
      if (ly.synced && ln.text) p.addEventListener('click', () => act(api.seek(ln.time_ms / 1000)));
      scroller.append(p); lines.push(p); wordEls.push(words);
    });
    update(true);
  }

  /** Scrolls the current line to the middle. Instant when asked to (a rebuild, a resize), smooth for the next line. */
  function center(instant) {
    const p = lines[active];
    if (!p || !scroller.clientHeight) return;
    const top = p.offsetTop - scroller.clientHeight / 2 + p.offsetHeight / 2;
    scroller.scrollTo({ top, behavior: document.documentElement.dataset.motion === 'reduced' || instant ? 'auto' : 'smooth' });
  }

  function update(force = false) {
    const ly = state.lyrics;
    if (!ly?.synced) return;
    const t = Math.round(position() * 1000);
    let lo = 0, hi = ly.lines.length; // last line starting at or before t
    while (lo < hi) { const mid = (lo + hi) >> 1; if (ly.lines[mid].time_ms <= t) lo = mid + 1; else hi = mid; }
    const idx = lo - 1;
    if (idx !== active || force) {
      active = idx;
      lines.forEach((p, i) => {
        p.classList.toggle('active', i === idx); p.classList.toggle('past', i < idx);
        p.dataset.d = String(Math.min(4, Math.abs(i - idx)));
      });
      if (idx >= 0 && performance.now() > userScrollUntil) center(force);
    }
    const words = wordEls[idx];
    if (words) { const wl = ly.lines[idx].words; words.forEach((w, k) => w.classList.toggle('done', wl[k].time_ms <= t)); }
  }
  scroller.addEventListener('wheel', () => { userScrollUntil = performance.now() + 3500; }, { passive: true });
  scroller.addEventListener('pointerdown', () => { userScrollUntil = performance.now() + 3500; });

  L.frame(() => update(), { whenPlaying: true });
  L.on('lyrics', build);
  L.on('player', () => { if (!isPlaying()) update(); });
  // The panel is built before it is in the page and moves when the skin or layout changes, so its size is
  // not known at first: re-centre whenever it changes, or the line sits off-screen until the next one.
  const ro = new ResizeObserver(() => { if (active >= 0 && performance.now() > userScrollUntil) center(true); });
  ro.observe(scroller);
  // A skin's font arriving later changes the line heights without changing the panel's size.
  const refit = () => { if (active >= 0 && performance.now() > userScrollUntil) center(true); };
  document.fonts?.addEventListener?.('loadingdone', refit);
  L.on('ui', () => { sizeVars(); if (get('lyrics.words') !== wordsOn) build(); else requestAnimationFrame(() => { if (active >= 0 && performance.now() > userScrollUntil) center(true); }); }); // the word highlight is baked into the lines, so changing it rebuilds them
  sizeVars();
  build();
  return { destroy: () => { ro.disconnect(); document.fonts?.removeEventListener?.('loadingdone', refit); L.destroy(); } };
}
