// "Find lyrics online…" for several songs at once (an album, an artist, a selection): looks up the ones that have
// no lyrics, one after another, and saves what it finds next to each song, in its tags, or both.

import { api } from '../api.js';
import { askToGoOnline, online } from '../online.js';
import { reloadLyrics } from '../controller.js';
import { state } from '../state.js';
import { h, plural, toast } from '../util.js';
import { button } from './controls.js';
import { modal } from './dialog.js';

const baseName = (p) => p.split(/[\\/]/).pop();
const TARGETS = [['file', 'Next to each song, as a .lrc file'], ['tags', 'In each song’s tags'], ['both', 'Both']];
const STATUS = { wait: '', look: 'Looking…', saved: 'Saved', none: 'Nothing found', instrumental: 'Listed as instrumental', had: 'Already has lyrics', fail: 'Could not save' };

/** Looks up and saves lyrics for `paths`. Resolves when the dialog closes. */
export async function findLyricsFor(paths) {
  if (!paths.length || !await askToGoOnline()) return;
  if (!online().lyrics) { toast('Lyrics lookup is turned off in Settings > Online.'); return; }

  let running = false; // Escape and clicking outside do nothing while songs are being looked up; Stop does
  await modal((box, done) => {
    let stopped = false;
    const rows = paths.map((path) => {
      const status = h('span', { class: 'lb-status' });
      const el = h('div', { class: 'lb-row', role: 'listitem' }, h('span', { class: 'lb-name', translate: 'no' }, baseName(path)), status);
      return { path, el, status, state: 'wait' };
    });
    const mark = (r, st) => { r.state = st; r.status.textContent = STATUS[st]; r.el.dataset.state = st; };
    const list = h('div', { class: 'lb-list', role: 'list', hidden: true }, rows.map((r) => r.el));
    const target = h('select', { class: 'select', 'aria-label': 'Save lyrics' }, TARGETS.map(([v, l]) => h('option', { value: v }, l)));
    const intro = h('p', { class: 'dialog-text' }, 'Looking at which songs already have lyrics…');
    const note = h('p', { class: 'hint' }, 'Only songs without lyrics are looked up, and nothing is replaced. Each song sends its title, artist, album and length to the lyrics services chosen in Settings > Online.');
    const sum = h('div', { class: 'lb-sum', hidden: true });
    const bar = h('progress', { class: 'lb-bar', max: paths.length, value: 0, hidden: true });
    const actions = h('div', { class: 'dialog-actions' });
    const closeBtn = button('Close', () => done(true));
    const startBtn = button('Find lyrics', () => run(), { cls: 'primary' });
    const stopBtn = button('Stop', () => { stopped = true; stopBtn.disabled = true; });
    startBtn.disabled = true;
    const cancelBtn = button('Cancel', () => done(false));
    actions.append(cancelBtn, startBtn);
    box.append(h('h3', { class: 'dialog-title' }, paths.length > 1 ? 'Find lyrics online' : 'Find lyrics online for this song'), intro, h('div', { class: 'lb-target' }, h('label', {}, 'Save them'), target), note, bar, list, sum, actions);
    box.classList.add('lb-dialog');

    const todo = () => rows.filter((r) => r.state === 'wait');
    (async () => {
      for (const r of rows) {
        let has = false;
        try { has = !!(await api.lyrics(r.path))?.lines?.length; } catch { /* treated as none */ }
        if (has) mark(r, 'had');
      }
      const n = todo().length;
      const rest = paths.length - n;
      if (!n) intro.textContent = 'Every one of these songs already has lyrics.';
      else intro.replaceChildren(n === 1 ? '1 song without lyrics.' : `${n} songs without lyrics.`, ...(rest ? [' ', h('span', {}, `The other ${rest} already have some.`)] : []));
      startBtn.disabled = !n; startBtn.textContent = 'Find lyrics';
      if (!n) { actions.replaceChildren(closeBtn); }
    })();

    async function run() {
      running = true; stopped = false;
      const tgt = target.value;
      list.hidden = false; bar.hidden = false; note.hidden = true; target.parentElement.hidden = true;
      actions.replaceChildren(stopBtn);
      const queue = todo();
      bar.max = queue.length; let n = 0, strikes = 0, reachedFail = false;
      for (const r of queue) {
        if (stopped) break;
        mark(r, 'look'); r.el.scrollIntoView({ block: 'nearest' });
        intro.textContent = `Looking up ${n + 1} of ${queue.length}…`;
        try {
          const found = await api.onlineLyrics(r.path);
          strikes = 0;
          if (!found) mark(r, 'none');
          else if (!found.lyrics) mark(r, 'instrumental');
          else {
            try {
              if (tgt !== 'tags') await api.saveLyricsFile(r.path, found.text, found.synced);
              if (tgt !== 'file') await api.writeTags(r.path, { lyrics: found.tag_text || found.text });
              mark(r, 'saved');
            } catch { mark(r, 'fail'); }
          }
        } catch {
          mark(r, 'fail'); strikes += 1;
          if (strikes >= 2) { reachedFail = true; break; } // the services cannot be reached: stop instead of failing every song
        }
        bar.value = ++n;
      }
      for (const r of rows) if (r.state === 'look') mark(r, 'wait');
      running = false;
      const count = (st) => rows.filter((r) => r.state === st).length;
      intro.textContent = reachedFail ? 'Stopped: the lyrics services could not be reached.' : stopped ? 'Stopped.' : 'Done.';
      const chips = [['saved', 'saved'], ['none', 'found nothing'], ['instrumental', 'instrumental'], ['fail', 'failed']].filter(([st]) => count(st));
      sum.replaceChildren(...chips.map(([st, word]) => h('span', { class: 'lb-chip', 'data-state': st }, `${count(st)} ${word}`)));
      sum.hidden = !chips.length;
      actions.replaceChildren(closeBtn); closeBtn.focus();
      if (count('saved') && state.trackPath && rows.some((r) => r.path === state.trackPath && r.state === 'saved')) reloadLyrics();
    }
  }, { guard: async () => !running });
}
