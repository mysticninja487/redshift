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
  el.append(h('div', { class: 'lyrics' }, badge, scroller, empty));
  let lines = [], active = -1, userScrollUntil = 0, wordEls = [];

  const sizeVars = () => {
    el.style.setProperty('--ly-size', String(get('lyrics.size')));
    el.style.setProperty('--ly-dim', String(get('lyrics.dim')));
    el.style.setProperty('--ly-blur', `${get('lyrics.blur')}px`);
    el.dataset.align = get('lyrics.align');
  };

  function build() {
    const ly = state.lyrics;
    scroller.replaceChildren(); lines = []; wordEls = []; active = -1;
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
      if (idx >= 0 && performance.now() > userScrollUntil) {
        const p = lines[idx];
        const top = p.offsetTop - scroller.clientHeight / 2 + p.offsetHeight / 2;
        scroller.scrollTo({ top, behavior: document.documentElement.dataset.motion === 'reduced' || force ? 'auto' : 'smooth' });
      }
    }
    const words = wordEls[idx];
    if (words) { const wl = ly.lines[idx].words; words.forEach((w, k) => w.classList.toggle('done', wl[k].time_ms <= t)); }
  }
  scroller.addEventListener('wheel', () => { userScrollUntil = performance.now() + 3500; }, { passive: true });
  scroller.addEventListener('pointerdown', () => { userScrollUntil = performance.now() + 3500; });

  L.frame(() => update(), { whenPlaying: true });
  L.on('lyrics', build);
  L.on('player', () => { if (!isPlaying()) update(); });
  L.on('ui', () => { sizeVars(); });
  sizeVars();
  build();
  return { destroy: () => L.destroy() };
}
