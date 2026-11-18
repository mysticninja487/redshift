// The lyrics sync tool: play the song, press Space as each line (or, in Words mode, each word) starts, and
// it is stamped with the time. Timings can be nudged afterwards, one at a time or all of them.

import { api } from '../api.js';
import { act, playPaths } from '../controller.js';
import { fmtStamp, lyricsToText, parseLyricsText } from '../lrc.js';
import { duration, isPlaying, position, state } from '../state.js';
import { clamp, fmtTime, h } from '../util.js';
import { button, segmented } from './controls.js';
import { modal } from './dialog.js';
import { icon } from './icons.js';

const NUDGE_MS = 100;
const REACTION = [['0', 'None'], ['150', '0.15 s'], ['300', '0.3 s']];

/**
 * The words of a line, each with its time: the line's own word stamps when they spell the same text, otherwise
 * its words split at spaces, untimed. Every word but the last keeps its trailing space, so joined they read as the line.
 */
function wordsOf(l) {
  if (l.words?.length && l.words.map((w) => w.text).join('').replace(/\s+/g, ' ').trim() === l.text) {
    return l.words.map((w) => ({ text: w.text, time: w.time }));
  }
  const toks = l.text.split(/\s+/).filter(Boolean);
  return toks.map((t, i) => ({ text: i < toks.length - 1 ? `${t} ` : t, time: null }));
}

/**
 * Resolves to the lyrics text with the new timings, or null if cancelled. `text` can be plain or
 * already stamped; existing stamps are kept until a line is stamped again.
 */
export function syncLyrics(path, text) {
  return modal((box, done) => {
    const { lines: all, meta } = parseLyricsText(text);
    const lines = all.filter((l) => l.text).map((l) => ({ text: l.text, time: l.time, words: wordsOf(l) }));
    let cursor = Math.max(0, lines.findIndex((l) => l.time == null));
    let sel = null, reaction = 150, ticker = 0;
    let mode = 'lines', wc = 0; // wc: the word the next stamp goes to, in Words mode
    let selWord = null; // the word picked in Words mode

    // ---- parts ----
    const rows = lines.map((l, i) => {
      const time = h('button', { class: 'sy-time', type: 'button', title: 'Jump to this line in the song', onclick: () => { if (l.time != null) seekTo(l.time - 1500); } });
      const text = h('button', { class: 'sy-text', type: 'button', title: 'Stamp this line next', onclick: () => pick(i) }, l.text);
      const chips = l.words.map((w, j) => h('button', { class: 'sy-w', type: 'button', title: 'Stamp this word next', onclick: () => pickWord(i, j) }, w.text.trim()));
      const wordBox = h('div', { class: 'sy-words', hidden: true }, chips);
      return { el: h('div', { class: 'sy-row', role: 'listitem' }, time, text, wordBox), time, text, wordBox, chips };
    });
    const list = h('div', { class: 'sy-list', role: 'list', 'aria-label': 'Lyrics lines' }, rows.map((r) => r.el));
    const clock = h('span', { class: 'sy-clock', 'aria-live': 'off' }, '0:00.0');
    const play = h('button', { class: 'btn icon-btn', type: 'button', onclick: () => act(api.togglePause()) });
    const status = h('p', { class: 'hint sy-status' });
    const stampBtn = button('Stamp line', () => stamp(), { cls: 'primary sy-stamp' });
    const modePick = segmented({ label: 'What to time', options: [{ value: 'lines', label: 'Lines' }, { value: 'words', label: 'Words' }], value: 'lines', onChange: (v) => setMode(v) });
    const intro = h('p', { class: 'hint' });
    const nudgeBack = button('−0.1 s', () => nudge(-NUDGE_MS), { cls: 'sy-small' });
    const nudgeFwd = button('+0.1 s', () => nudge(NUDGE_MS), { cls: 'sy-small' });
    const shiftBack = button('Shift all −0.1 s', () => shiftAll(-NUDGE_MS), { cls: 'sy-small' });
    const shiftFwd = button('Shift all +0.1 s', () => shiftAll(NUDGE_MS), { cls: 'sy-small' });
    const lag = h('select', { class: 'select sy-lag', 'aria-label': 'Allow for reaction time', onchange: (e) => { reaction = +e.target.value; } },
      REACTION.map(([v, label]) => h('option', { value: v, selected: +v === reaction }, label)));
    const useBtn = button('Use these timings', () => finish(), { cls: 'primary' });

    // ---- behaviour ----
    const missing = () => lines.filter((l) => l.time == null).length;
    const missingWords = () => lines.reduce((n, l) => n + l.words.filter((w) => w.time == null).length, 0);
    function seekTo(ms) { act(api.seek(clamp(ms / 1000, 0, duration() || 1e9))); }
    /** Lines whose time (or a word's) is earlier than one before it: a file has to be in order to play right. */
    const outOfOrder = () => {
      const bad = new Set(); let last = -1;
      lines.forEach((l, i) => {
        const times = (mode === 'words' || l.words.some((w) => w.time != null)) ? l.words.map((w) => w.time).filter((t) => t != null) : [];
        for (const t of [l.time, ...times]) { if (t == null) continue; if (t < last) bad.add(i); else last = t; }
      });
      return bad;
    };
    function paint() {
      const bad = outOfOrder(), words = mode === 'words';
      rows.forEach((r, i) => {
        const l = lines[i];
        r.el.classList.toggle('is-off', bad.has(i));
        r.time.textContent = l.time == null ? '——' : fmtStamp(l.time);
        r.time.disabled = l.time == null;
        r.el.classList.toggle('is-next', i === cursor);
        r.el.classList.toggle('is-sel', !words && i === sel);
        r.el.classList.toggle('is-done', l.time != null);
        r.text.hidden = words; r.wordBox.hidden = !words;
        if (words) r.chips.forEach((c, j) => {
          c.classList.toggle('is-wdone', l.words[j].time != null);
          c.classList.toggle('is-wnext', i === cursor && j === wc);
          c.classList.toggle('is-wsel', selWord?.i === i && selWord?.j === j);
        });
      });
      const m = missing(), mw = missingWords();
      status.textContent = bad.size ? `Line ${[...bad][0] + 1} is timed earlier than something before it. Fix it with the nudge buttons, or take its time back with Backspace.`
        : !lines.length ? 'There are no lyrics to sync yet.'
        : words ? (mw ? `${mw === 1 ? '1 word still needs' : `${mw} words still need`} a time. Next: “${lines[cursor]?.words[wc]?.text.trim() ?? ''}”` : 'Every word has a time. Play it through to check, then use these timings.')
        : m ? `${m} of ${lines.length} lines still need a time. Next: “${lines[cursor]?.text ?? ''}”`
        : 'Every line has a time. Play it through to check, then use these timings.';
      const target = words ? selWord != null && lines[selWord.i]?.words[selWord.j]?.time != null : sel != null && lines[sel]?.time != null;
      nudgeBack.disabled = nudgeFwd.disabled = !target;
      shiftBack.disabled = shiftFwd.disabled = lines.every((l) => l.time == null);
      stampBtn.disabled = !lines.length;
      stampBtn.textContent = words ? 'Stamp word' : 'Stamp line';
      useBtn.disabled = lines.every((l) => l.time == null) || bad.size > 0;
      intro.textContent = words
        ? 'Play the song and press Space the moment each highlighted word starts. Backspace takes the last stamp back; [ and ] nudge the picked word by a tenth of a second. A line needs every word timed to be saved word by word.'
        : 'Play the song and press Space the moment each highlighted line starts. Backspace takes the last stamp back; [ and ] nudge a line by a tenth of a second. Switch to Words to time each word.';
    }
    const follow = () => rows[cursor]?.el.scrollIntoView({ block: 'center', behavior: matchMedia('(prefers-reduced-motion: reduce)').matches ? 'auto' : 'smooth' });
    function pick(i) { cursor = i; sel = i; wc = 0; paint(); follow(); }
    function pickWord(i, j) {
      cursor = i; wc = j; selWord = { i, j }; paint();
      const t = lines[i].words[j].time;
      if (t != null) seekTo(t - 1500);
    }
    function setMode(m) {
      mode = m; sel = null; selWord = null;
      if (m === 'words') {
        const i = lines.findIndex((l) => l.words.some((w) => w.time == null));
        cursor = i < 0 ? 0 : i; wc = Math.max(0, lines[cursor]?.words.findIndex((w) => w.time == null) ?? 0);
      } else {
        cursor = Math.max(0, lines.findIndex((l) => l.time == null)); wc = 0;
      }
      paint(); follow();
    }
    function stamp() {
      if (!lines.length) return;
      if (cursor >= lines.length) cursor = 0;
      const now = Math.max(0, Math.round(position() * 1000 - reaction)), l = lines[cursor];
      if (mode === 'words') {
        const j = Math.min(wc, l.words.length - 1);
        l.words[j].time = now;
        if (j === 0) l.time = now;
        selWord = { i: cursor, j };
        if (j + 1 < l.words.length) wc = j + 1;
        else if (cursor + 1 < lines.length) { cursor += 1; wc = 0; }
        paint(); follow();
        return;
      }
      l.time = now;
      l.words.forEach((w) => { w.time = null; }); // the old word times belonged to the old line time
      sel = cursor;
      cursor = Math.min(lines.length, cursor + 1);
      if (cursor >= lines.length) cursor = lines.every((x) => x.time != null) ? lines.length - 1 : lines.findIndex((x) => x.time == null);
      paint(); follow();
    }
    function undo() {
      if (mode === 'words') {
        let i = cursor, j = wc - 1;
        if (j < 0) { i -= 1; j = (lines[i]?.words.length ?? 1) - 1; }
        const w = lines[i]?.words[j];
        if (!w || w.time == null) return;
        seekTo(w.time - 2500);
        w.time = null; if (j === 0) lines[i].time = null;
        cursor = i; wc = j; selWord = null; paint(); follow();
        return;
      }
      const i = lines[cursor]?.time != null ? cursor : cursor - 1;
      const target = lines[i];
      if (i < 0 || target?.time == null) return;
      seekTo(target.time - 2500);
      target.time = null; target.words.forEach((w) => { w.time = null; }); cursor = i; sel = null; paint(); follow();
    }
    function nudge(d) {
      if (mode === 'words') {
        const w = selWord && lines[selWord.i]?.words[selWord.j];
        if (!w || w.time == null) return;
        w.time = Math.max(0, w.time + d);
        if (selWord.j === 0) lines[selWord.i].time = w.time;
        paint(); return;
      }
      const l = lines[sel];
      if (sel == null || l?.time == null) return;
      const was = l.time; l.time = Math.max(0, l.time + d);
      for (const w of l.words) if (w.time != null) w.time = Math.max(0, w.time + (l.time - was));
      paint();
    }
    function shiftAll(d) {
      for (const l of lines) {
        if (l.time != null) l.time = Math.max(0, l.time + d);
        for (const w of l.words) if (w.time != null) w.time = Math.max(0, w.time + d);
      }
      paint();
    }
    function finish() {
      cleanup();
      done(lyricsToText(lines.map((l) => ({ time: l.time, text: l.text, words: l.words })), meta));
    }
    function cleanup() { clearInterval(ticker); removeEventListener('keydown', onKey, true); }

    const onKey = (e) => {
      if (e.ctrlKey || e.metaKey || e.altKey) return;
      if (e.target instanceof HTMLSelectElement) return;
      const take = () => { e.preventDefault(); e.stopPropagation(); };
      switch (e.key) {
        case ' ': case 'Enter': take(); stamp(); break;
        case 'Backspace': take(); undo(); break;
        case 'ArrowUp': take(); cursor = Math.max(0, cursor - 1); wc = 0; paint(); follow(); break;
        case 'ArrowDown': take(); cursor = Math.min(lines.length - 1, cursor + 1); wc = 0; paint(); follow(); break;
        case 'w': case 'W': take(); modePick.set(mode === 'words' ? 'lines' : 'words'); setMode(mode === 'words' ? 'lines' : 'words'); break;
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
      rows.forEach((r, i) => {
        r.el.classList.toggle('is-now', i === now && p);
        if (mode !== 'words') return;
        let wn = -1;
        if (i === now) lines[i].words.forEach((w, j) => { if (w.time != null && w.time <= pos * 1000) wn = j; });
        r.chips.forEach((c, j) => c.classList.toggle('is-wnow', p && j === wn));
      });
    };
    ticker = setInterval(tick, 80);

    box.classList.add('tg-sync');
    box.append(
      h('h3', { class: 'dialog-title' }, 'Sync lyrics'),
      intro,
      h('div', { class: 'sy-bar' }, play, clock,
        button('−5 s', () => seekTo(position() * 1000 - 5000), { cls: 'sy-small' }), button('+5 s', () => seekTo(position() * 1000 + 5000), { cls: 'sy-small' }),
        h('label', { class: 'sy-lagwrap' }, 'Reaction time', lag)),
      list,
      h('div', { class: 'sy-bar' }, modePick, stampBtn, nudgeBack, nudgeFwd, shiftBack, shiftFwd),
      status,
      h('div', { class: 'dialog-actions' }, button('Cancel', () => { cleanup(); done(null); }), useBtn));
    paint(); tick();
    setTimeout(follow, 50);
    if (state.trackPath !== path) playPaths([path], 0);
  });
}
