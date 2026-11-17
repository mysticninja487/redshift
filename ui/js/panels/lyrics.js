// Lyrics that follow the song. Synced lyrics highlight the current line (and word, when the file
// has word timing) and click-to-seek; plain lyrics just scroll.

import { api } from '../api.js';
import { act, chooseLyrics, lyricsOnlineAllowed, saveOnlineLyrics, saveOnlineLyricsToTags, searchOnlineLyrics } from '../controller.js';
import { askToGoOnline, online } from '../online.js';
import { get } from '../prefs.js';
import { isPlaying, position, state } from '../state.js';
import { h, toast } from '../util.js';
import { icon } from '../ui/icons.js';
import { confirmDialog } from '../ui/dialog.js';
import { segmented } from '../ui/controls.js';
import { lifecycle } from './common.js';

export function mount(el) {
  const L = lifecycle();
  const scroller = h('div', { class: 'ly-scroll', tabindex: '0', 'aria-label': 'Lyrics' });
  const emptyTitle = h('p', { class: 'ly-empty-title' }), emptyHint = h('p', { class: 'ly-empty-hint' });
  const findBtn = h('button', { class: 'btn', type: 'button', onclick: () => findOnline() }, 'Find lyrics online\u2026');
  const addBtn = h('button', { class: 'btn', type: 'button', onclick: () => editLyrics() }, 'Add lyrics\u2026');
  const empty = h('div', { class: 'empty ly-empty' }, icon('lyrics', 28), emptyTitle, emptyHint, h('div', { class: 'ly-empty-actions' }, findBtn, addBtn));
  const badge = h('div', { class: 'ly-badge' });
  const editBtn = h('button', { class: 'btn icon-btn ly-edit', type: 'button', title: 'Edit or sync lyrics', 'aria-label': 'Edit or sync lyrics', onclick: () => editLyrics() }, icon('edit', 16));
  // The bar above the lyrics: a switch between a song's own lyrics and the online ones, where they came
  // from, and what can be done with online lyrics (keep them in the tags, or in a .lrc file).
  const pick = segmented({ label: 'Which lyrics', options: [{ value: 'local', label: 'Built-in' }, { value: 'online', label: 'Online' }], value: 'local', onChange: (v) => chooseLyrics(v) });
  const source = h('span', { class: 'ly-source' });
  const tagsBtn = h('button', { class: 'btn ly-save', type: 'button', title: 'Write these lyrics into the song\u2019s tags, line by line', onclick: async () => {
    const ok = await saveOnlineLyricsToTags(() => confirmDialog('Replace the lyrics in the tags?', 'This song already has lyrics in its tags. They will be replaced by the online ones.', { ok: 'Replace', danger: false }));
    if (ok) toast('Lyrics saved to the song\u2019s tags');
  } }, 'Save to tags');
  const fileBtn = h('button', { class: 'btn ly-save', type: 'button', title: 'Save a .lrc file next to the song, with word timing when the service has it', onclick: async () => { if (await saveOnlineLyrics()) toast('Lyrics saved next to the song'); } }, 'Save as .lrc');
  const actions = h('div', { class: 'ly-actions' }, tagsBtn, fileBtn);
  const fromOnline = h('div', { class: 'ly-online', hidden: true }, pick, source, actions);
  const root = h('div', { class: 'lyrics' }, badge, fromOnline, editBtn, scroller, empty);
  /** The Find button: asks to go online first if that is still off, then looks. */
  async function findOnline() {
    if (!await askToGoOnline()) return;
    searchOnlineLyrics();
  }
  async function editLyrics() {
    if (!state.trackPath) return;
    const { openTagEditor } = await import('../ui/tageditor.js');
    openTagEditor([state.trackPath], { tab: 'lyrics' });
  }
  el.append(root);
  let lines = [], active = -1, userScrollUntil = 0, wordEls = [], wordsOn = get('lyrics.words');

  const sizeVars = () => {
    el.style.setProperty('--ly-size', String(get('lyrics.size')));
    el.style.setProperty('--ly-dim', String(get('lyrics.dim')));
    el.style.setProperty('--ly-blur', `${get('lyrics.blur')}px`);
    el.dataset.align = root.dataset.align = get('lyrics.align'); // the styles key off .lyrics[data-align]
  };

  /** The empty panel says what is going on: nothing yet, searching, nothing found, or a problem. */
  function fillEmpty() {
    const st = state.lyricsStatus, o = online();
    const [title, hint] = {
      searching: ['Looking for lyrics online\u2026', ''],
      none: ['No lyrics found online', 'Nothing has been found for this song yet. You can add your own.'],
      instrumental: ['Instrumental', 'LRCLIB lists this song as having no lyrics.'],
      error: ['Could not look up lyrics', state.lyricsError],
    }[st] ?? ['No lyrics for this track', o.enabled ? 'Put a .lrc file with the same name next to the song, or embed lyrics in its tags.' : 'Put a .lrc file with the same name next to the song, embed lyrics in its tags, or let Redshift look them up online.'];
    emptyTitle.textContent = title; emptyHint.textContent = hint; emptyHint.hidden = !hint;
    findBtn.hidden = st === 'searching' || st === 'instrumental' || !state.trackPath;
    findBtn.textContent = st === 'error' || st === 'none' ? 'Try again' : 'Find lyrics online\u2026';
    addBtn.hidden = st === 'searching' || st === 'instrumental';
    empty.dataset.busy = String(st === 'searching');
  }

  /** The bar shows when there is something to choose between or to save, and says what is going on. */
  function paintBar() {
    const view = state.lyricsView, st = state.lyricsStatus, local = !!state.lyricsLocal;
    const canSwitch = local && lyricsOnlineAllowed();
    const showingOnline = view === 'online' && !!state.lyricsOnline;
    fromOnline.hidden = !(canSwitch || showingOnline);
    pick.hidden = !canSwitch;
    pick.set(showingOnline ? 'online' : 'local');
    actions.hidden = !showingOnline;
    const o = state.lyricsOnline;
    source.textContent = st === 'searching' ? 'Looking online\u2026' : st === 'none' ? 'Nothing found online' : st === 'instrumental' ? 'Listed as instrumental' : st === 'error' ? `Could not look up lyrics` : showingOnline ? `From ${o.provider}` : '';
    source.title = st === 'error' ? state.lyricsError : showingOnline && o.word_synced ? 'Every word has its own time' : '';
    source.hidden = !source.textContent;
  }

  function build() {
    const ly = state.lyrics;
    scroller.replaceChildren(); lines = []; wordEls = []; active = -1; wordsOn = get('lyrics.words');
    el.dataset.synced = String(!!ly?.synced);
    editBtn.hidden = !state.trackPath;
    empty.hidden = !!ly; scroller.hidden = !ly;
    badge.textContent = ly ? (ly.synced ? '' : 'Not synced') : '';
    paintBar();
    if (!ly) fillEmpty();
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
  L.on('track', () => { editBtn.hidden = !state.trackPath; });
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
