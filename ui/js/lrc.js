// Lyrics as editable text: reading and writing `[mm:ss.xx]` stamps, for the tag editor and the sync tool.
// The backend parses lyrics for playback; this is only for working on the text before it is saved.

const STAMP = /\[(\d{1,3}):(\d{2})(?:[.:](\d{1,3}))?\]/g;
const WORD = /<(\d{1,3}):(\d{2})(?:[.:](\d{1,3}))?>/g;
const META = /^\s*\[(ti|ar|al|au|by|length|offset|re|ve|la):[^\]]*\]\s*$/i;

export function fmtStamp(ms) {
  const t = Math.max(0, Math.round(ms / 10) * 10), m = Math.floor(t / 60000), s = Math.floor((t % 60000) / 1000), cs = Math.floor((t % 1000) / 10);
  return `${String(m).padStart(2, '0')}:${String(s).padStart(2, '0')}.${String(cs).padStart(2, '0')}`;
}

const toMs = (m, s, f) => (+m * 60 + +s) * 1000 + (f ? Math.round(+(`0.${f}`) * 1000) : 0);

/**
 * Splits lyrics text into lines: `{ time, text, words }` with `time` in ms or null when the line has no
 * stamp. `words` is `[{ time, text }]` when the line has `<mm:ss.xx>` word stamps (enhanced LRC), and
 * `text` is always the plain words. A line with several stamps becomes several lines. `[ti:]`-style header
 * lines are returned apart in `meta`, so they can be written back as they were.
 */
export function parseLyricsText(text) {
  const lines = [], meta = [];
  for (const raw of String(text ?? '').replace(/\r\n?/g, '\n').split('\n')) {
    if (META.test(raw)) { meta.push(raw.trim()); continue; }
    const stamps = [...raw.matchAll(STAMP)];
    const rest = raw.replace(STAMP, '');
    const marks = [...rest.matchAll(WORD)];
    const body = rest.replace(WORD, '').replace(/\s+/g, ' ').trim();
    let words = null;
    if (marks.length) {
      words = marks.map((m, i) => ({ time: toMs(m[1], m[2], m[3]), text: rest.slice(m.index + m[0].length, marks[i + 1]?.index ?? rest.length) }))
        .filter((w) => w.text.trim());
      if (!words.length) words = null;
    }
    if (!stamps.length) { lines.push({ time: null, text: raw.replace(WORD, '').trim() }); continue; }
    for (const s of stamps) lines.push({ time: toMs(s[1], s[2], s[3]), text: body, ...(words ? { words: words.map((w) => ({ ...w })) } : {}) });
  }
  while (lines.length && !lines[lines.length - 1].text && lines[lines.length - 1].time == null) lines.pop();
  return { lines, meta };
}

/** Whether every word of the line has a time (so it can be written with word stamps). */
const wordTimed = (l) => l.time != null && l.words?.length > 1 && l.words.every((w) => w.time != null);

/** Text for `lines` (as from `parseLyricsText`): `[mm:ss.xx]words` per stamped line, with `<mm:ss.xx>` before each word when every word is timed, plain otherwise. */
export function lyricsToText(lines, meta = []) {
  const body = lines.map((l) => {
    if (l.time == null) return l.text;
    if (wordTimed(l)) return `[${fmtStamp(l.time)}]` + l.words.map((w) => `<${fmtStamp(w.time)}>${w.text}`).join('').trimEnd();
    return `[${fmtStamp(l.time)}]${l.text}`;
  });
  return [...meta, ...body].join('\n');
}

/** How many lines carry word timing. */
export function wordTimedCount(text) {
  return parseLyricsText(text).lines.filter((l) => l.words?.length > 1).length;
}

/** The same lyrics with line stamps only: what goes into a song's tags, where most players cannot read word stamps. */
export function lineStampsOnly(text) {
  return String(text ?? '').replace(WORD, '').replace(/[ \t]+$/gm, '').replace(/(\])[ \t]+/g, '$1');
}

/** How stamped the text is: `{ total, stamped }` over lines that have words. */
export function stampCount(text) {
  const lines = parseLyricsText(text).lines.filter((l) => l.text);
  return { total: lines.length, stamped: lines.filter((l) => l.time != null).length };
}

/** The same lyrics with every stamp removed. */
export function stripStamps(text) {
  return parseLyricsText(text).lines.map((l) => l.text).join('\n').replace(/\n{3,}/g, '\n\n').trim();
}

/** Editable text for lyrics the backend returned (`state.lyrics`): LRC when synced, plain otherwise. */
export function textOfLyrics(ly) {
  if (!ly?.lines?.length) return '';
  const meta = [];
  if (ly.synced) {
    if (ly.title) meta.push(`[ti:${ly.title}]`);
    if (ly.artist) meta.push(`[ar:${ly.artist}]`);
    if (ly.album) meta.push(`[al:${ly.album}]`);
    if (ly.offset_ms) meta.push(`[offset:${ly.offset_ms}]`);
  }
  return lyricsToText(ly.lines.map((l) => ({
    time: ly.synced ? l.time_ms : null, text: l.text,
    ...(ly.synced && l.words?.length > 1 ? { words: l.words.map((w) => ({ time: w.time_ms, text: w.text })) } : {}),
  })), meta);
}
