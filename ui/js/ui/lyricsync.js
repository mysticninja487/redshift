// The lyrics sync tool: play the song, press Space as each line starts, and the line is stamped
// with the time. Timings can be nudged afterwards, one line or all of them.

import { api } from '../api.js';
import { act, playPaths } from '../controller.js';
import { fmtStamp, lyricsToText, parseLyricsText } from '../lrc.js';
import { duration, isPlaying, position, state } from '../state.js';
import { clamp, fmtTime, h } from '../util.js';
import { button } from './controls.js';
import { modal } from './dialog.js';
import { icon } from './icons.js';

const NUDGE_MS = 100;
const REACTION = [['0', 'None'], ['150', '0.15 s'], ['300', '0.3 s']];

/**
 * Resolves to the lyrics text with the new timings, or null if cancelled. `text` can be plain or
 * already stamped; existing stamps are kept until a line is stamped again.
 */
export function syncLyrics(path, text) {
  return modal((box, done) => {
    const { lines: all, meta } = parseLyricsText(text);
    const lines = all.filter((l) => l.text).map((l) => ({ text: l.text, time: l.time }));
    let cursor = Math.max(0, lines.findIndex((l) => l.time == null));
    let sel = null, reaction = 150, ticker = 0;

    // ---- parts ----
    const rows = lines.map((l, i) => {
      const time = h('button', { class: 'sy-time', type: 'button', title: 'Jump to this line in the song', onclick: () => { if (l.time != null) seekTo(l.time - 1500); } });
      const words = h('button', { class: 'sy-text', type: 'button', title: 'Stamp this line next', onclick: () => pick(i) }, l.text);
      return { el: h('div', { class: 'sy-row', role: 'listitem' }, time, words), time };
    });
    const list = h('div', { class: 'sy-list', role: 'list', 'aria-label': 'Lyrics lines' }, rows.map((r) => r.el));
    const clock = h('span', { class: 'sy-clock', 'aria-live': 'off' }, '0:00.0');
    const play = h('button', { class: 'btn icon-btn', type: 'button', onclick: () => act(api.togglePause()) });
    const status = h('p', { class: 'hint sy-status' });
    const stampBtn = button('Stamp line', () => stamp(), { cls: 'primary sy-stamp' });
    const nudgeBack = button('−0.1 s', () => nudge(-NUDGE_MS), { cls: 'sy-small' });
    const nudgeFwd = button('+0.1 s', () => nudge(NUDGE_MS), { cls: 'sy-small' });
    const shiftBack = button('Shift all −0.1 s', () => shiftAll(-NUDGE_MS), { cls: 'sy-small' });
    const shiftFwd = button('Shift all +0.1 s', () => shiftAll(NUDGE_MS), { cls: 'sy-small' });
    const lag = h('select', { class: 'select sy-lag', 'aria-label': 'Allow for reaction time', onchange: (e) => { reaction = +e.target.value; } },
      REACTION.map(([v, label]) => h('option', { value: v, selected: +v === reaction }, label)));
    const useBtn = button('Use these timings', () => finish(), { cls: 'primary' });

    // ---- behaviour ----
    const missing = () => lines.filter((l) => l.time == null).length;
    function seekTo(ms) { act(api.seek(clamp(ms / 1000, 0, duration() || 1e9))); }
    /** Lines whose time is earlier than an earlier line's: a file has to be in order to play right. */
    const outOfOrder = () => {
      const bad = new Set(); let last = -1;
      lines.forEach((l, i) => { if (l.time == null) return; if (l.time < last) bad.add(i); else last = l.time; });
      return bad;
    };
    function paint() {
      const bad = outOfOrder();
      rows.forEach((r, i) => {
        const l = lines[i];
        r.el.classList.toggle('is-off', bad.has(i));
        r.time.textContent = l.time == null ? '——' : fmtStamp(l.time);
        r.time.disabled = l.time == null;
        r.el.classList.toggle('is-next', i === cursor);
        r.el.classList.toggle('is-sel', i === sel);
        r.el.classList.toggle('is-done', l.time != null);
      });
      const m = missing();
      status.textContent = bad.size ? `Line ${[...bad][0] + 1} is timed earlier than a line before it. Fix it with the nudge buttons, or take its time back with Backspace.`
        : !lines.length ? 'There are no lyrics to sync yet.'
        : m ? `${m} of ${lines.length} lines still need a time. Next: “${lines[cursor]?.text ?? ''}”`
        : 'Every line has a time. Play it through to check, then use these timings.';
      nudgeBack.disabled = nudgeFwd.disabled = sel == null || lines[sel]?.time == null;
      shiftBack.disabled = shiftFwd.disabled = lines.every((l) => l.time == null);
      stampBtn.disabled = !lines.length;
      useBtn.disabled = lines.every((l) => l.time == null) || bad.size > 0;
    }
    const follow = () => rows[cursor]?.el.scrollIntoView({ block: 'center', behavior: matchMedia('(prefers-reduced-motion: reduce)').matches ? 'auto' : 'smooth' });
    function pick(i) { cursor = i; sel = i; paint(); follow(); }
    function stamp() {
      if (!lines.length) return;
      const i = cursor >= lines.length ? 0 : cursor;
      lines[i].time = Math.max(0, Math.round(position() * 1000 - reaction));
      sel = i;
      cursor = Math.min(lines.length, i + 1);
      if (cursor >= lines.length) cursor = lines.every((l) => l.time != null) ? lines.length - 1 : lines.findIndex((l) => l.time == null);
      paint(); follow();
    }
    function undo() {
      const i = lines[cursor]?.time != null ? cursor : cursor - 1;
      const target = lines[i];
      if (i < 0 || target?.time == null) return;
      seekTo(target.time - 2500);
      target.time = null; cursor = i; sel = null; paint(); follow();
    }
    function nudge(d) { if (sel != null && lines[sel].time != null) { lines[sel].time = Math.max(0, lines[sel].time + d); paint(); } }
    function shiftAll(d) { for (const l of lines) if (l.time != null) l.time = Math.max(0, l.time + d); paint(); }
    function finish() {
      cleanup();
      done(lyricsToText(lines.map((l) => ({ time: l.time, text: l.text })), meta));
    }
    function cleanup() { clearInterval(ticker); removeEventListener('keydown', onKey, true); }

    const onKey = (e) => {
      if (e.ctrlKey || e.metaKey || e.altKey) return;
      if (e.target instanceof HTMLSelectElement) return;
      const take = () => { e.preventDefault(); e.stopPropagation(); };
      switch (e.key) {
        case ' ': case 'Enter': take(); stamp(); break;
        case 'Backspace': take(); undo(); break;
        case 'ArrowUp': take(); cursor = Math.max(0, cursor - 1); paint(); follow(); break;
        case 'ArrowDown': take(); cursor = Math.min(lines.length - 1, cursor + 1); paint(); follow(); break;
        case 'ArrowLeft': take(); seekTo(position() * 1000 - 5000); break;
        case 'ArrowRight': take(); seekTo(position() * 1000 + 5000); break;
        case '[': take(); nudge(-NUDGE_MS); break;
        case ']': take(); nudge(NUDGE_MS); break;
        case 'p': case 'P': take(); act(api.togglePause()); break;
        case 'Escape': take(); cleanup(); done(null); break;
        default:
      }
    };
    addEventListener('keydown', onKey, true);

    const tick = () => {
      const pos = position();
      clock.textContent = `${fmtTime(pos)}.${Math.floor((pos % 1) * 10)}`;
      const p = isPlaying();
      play.replaceChildren(icon(p ? 'pause' : 'play', 18));
      play.title = play.ariaLabel = p ? 'Pause (P)' : 'Play (P)';
      let now = -1;
      lines.forEach((l, i) => { if (l.time != null && l.time <= pos * 1000) now = i; });
      rows.forEach((r, i) => r.el.classList.toggle('is-now', i === now && p));
    };
    ticker = setInterval(tick, 80);

    box.classList.add('tg-sync');
    box.append(
      h('h3', { class: 'dialog-title' }, 'Sync lyrics'),
      h('p', { class: 'hint' }, 'Play the song and press Space the moment each highlighted line starts. Backspace takes the last stamp back; [ and ] nudge a line by a tenth of a second.'),
      h('div', { class: 'sy-bar' }, play, clock,
        button('−5 s', () => seekTo(position() * 1000 - 5000), { cls: 'sy-small' }), button('+5 s', () => seekTo(position() * 1000 + 5000), { cls: 'sy-small' }),
        h('label', { class: 'sy-lagwrap' }, 'Reaction time', lag)),
      list,
      h('div', { class: 'sy-bar' }, stampBtn, nudgeBack, nudgeFwd, shiftBack, shiftFwd),
      status,
      h('div', { class: 'dialog-actions' }, button('Cancel', () => { cleanup(); done(null); }), useBtn));
    paint(); tick();
    setTimeout(follow, 50);
    if (state.trackPath !== path) playPaths([path], 0);
  });
}
