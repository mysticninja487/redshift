// Lyrics as editable text: reading and writing `[mm:ss.xx]` stamps, for the tag editor and the sync tool.
// The backend parses lyrics for playback; this is only for working on the text before it is saved.

const STAMP = /\[(\d{1,3}):(\d{2})(?:[.:](\d{1,3}))?\]/g;
const META = /^\s*\[(ti|ar|al|au|by|length|offset|re|ve|la):[^\]]*\]\s*$/i;

export function fmtStamp(ms) {
  const t = Math.max(0, Math.round(ms / 10) * 10), m = Math.floor(t / 60000), s = Math.floor((t % 60000) / 1000), cs = Math.floor((t % 1000) / 10);
  return `${String(m).padStart(2, '0')}:${String(s).padStart(2, '0')}.${String(cs).padStart(2, '0')}`;
}

const toMs = (m, s, f) => (+m * 60 + +s) * 1000 + (f ? Math.round(+(`0.${f}`) * 1000) : 0);

/**
 * Splits lyrics text into lines: `{ time, text }` with `time` in ms or null when the line has no
 * stamp. A line with several stamps becomes several lines. `[ti:]`-style header lines are returned
 * apart in `meta`, so they can be written back as they were.
 */
export function parseLyricsText(text) {
  const lines = [], meta = [];
  for (const raw of String(text ?? '').replace(/\r\n?/g, '\n').split('\n')) {
    if (META.test(raw)) { meta.push(raw.trim()); continue; }
    const stamps = [...raw.matchAll(STAMP)];
    const body = raw.replace(STAMP, '').trim();
    if (!stamps.length) { lines.push({ time: null, text: raw.trim() }); continue; }
    for (const s of stamps) lines.push({ time: toMs(s[1], s[2], s[3]), text: body });
  }
  while (lines.length && !lines[lines.length - 1].text && lines[lines.length - 1].time == null) lines.pop();
  return { lines, meta };
}

/** Text for `lines` (as from `parseLyricsText`): `[mm:ss.xx]words` per stamped line, plain otherwise. */
export function lyricsToText(lines, meta = []) {
  const body = lines.map((l) => (l.time == null ? l.text : `[${fmtStamp(l.time)}]${l.text}`));
  return [...meta, ...body].join('\n');
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
  return lyricsToText(ly.lines.map((l) => ({ time: ly.synced ? l.time_ms : null, text: l.text })), meta);
}
