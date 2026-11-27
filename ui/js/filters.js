// Library search and filters. Pure functions over the track list the Library already holds, so they
// run in the page without asking the backend, and can be tested without a browser.
//
// A filter is { text, match, rules }:
//   text   words that must all appear in the title, artist, album, album artist, genre or file name
//   match  'all' (every rule) or 'any' (at least one rule)
//   rules  [{ id, field, op, a, b, unit }]  one condition each; a rule without a value yet is ignored

const DAY = 86400;

/** Lower case, without accents, so "cafe" finds "Café". */
export const fold = (s) => String(s ?? '').normalize('NFD').replace(/[̀-ͯ]/g, '').toLowerCase();

const LOSSLESS = new Set(['flac', 'wav', 'wave', 'aif', 'aiff', 'alac', 'ape', 'wv', 'tta', 'dsf', 'dff']);
/** Whether the file is a lossless one. An .m4a is lossless only when it is ALAC, which has a bit depth. */
export const isLossless = (t) => LOSSLESS.has(t.format) || (['m4a', 'mp4'].includes(t.format) && !!t.bit_depth);
export const isHiRes = (t) => isLossless(t) && ((t.bit_depth ?? 0) > 16 || (t.sample_rate ?? 0) > 48000);

const stat = (ctx, t) => ctx.stats?.get(t.path) ?? { plays: 0, last_played: 0 };

export const QUALITY = [['lossless', 'Lossless'], ['lossy', 'Lossy'], ['hires', 'Hi-res (above 16-bit / 48 kHz)'], ['cd', 'CD quality (16-bit / 44.1 kHz)']];
const qualityOf = (t, q) => ({ lossless: isLossless(t), lossy: !isLossless(t), hires: isHiRes(t), cd: isLossless(t) && (t.bit_depth ?? 16) === 16 && t.sample_rate === 44100 })[q];

const baseName = (p) => String(p).split(/[\\/]/).pop();

/**
 * Every field a rule can look at. `kind` decides the operators and the value controls:
 * text, number, date, choice, bool or playlist. `get` reads the value from a track (and `ctx`).
 */
export const FIELDS = [
  { id: 'title', label: 'Title', kind: 'text', get: (t) => t.title },
  { id: 'artist', label: 'Artist', kind: 'text', get: (t) => t.artist },
  { id: 'album', label: 'Album', kind: 'text', get: (t) => t.album },
  { id: 'albumArtist', label: 'Album artist', kind: 'text', get: (t) => t.album_artist },
  { id: 'genre', label: 'Genre', kind: 'text', get: (t) => t.genre },
  { id: 'path', label: 'Folder or file name', kind: 'text', get: (t) => t.path },
  { id: 'year', label: 'Year', kind: 'number', get: (t) => t.year ?? null, hint: '1994' },
  { id: 'trackNo', label: 'Track number', kind: 'number', get: (t) => t.track_no ?? null },
  { id: 'discNo', label: 'Disc number', kind: 'number', get: (t) => t.disc_no ?? null },
  { id: 'length', label: 'Length', kind: 'number', get: (t) => t.duration_secs ?? null, parse: 'time', hint: '3:30', tol: 0.5 },
  { id: 'format', label: 'File type', kind: 'choice', get: (t) => String(t.format ?? '').toUpperCase() },
  { id: 'quality', label: 'Quality', kind: 'choice', fixed: QUALITY, test: (t, v) => qualityOf(t, v) },
  { id: 'sampleRate', label: 'Sample rate (kHz)', kind: 'number', get: (t) => (t.sample_rate ? t.sample_rate / 1000 : null), hint: '44.1', tol: 0.001 },
  { id: 'bitDepth', label: 'Bit depth', kind: 'number', get: (t) => t.bit_depth ?? null, hint: '24' },
  { id: 'bitrate', label: 'Bitrate (kbps)', kind: 'number', get: (t) => t.bitrate_kbps ?? null, hint: '320' },
  { id: 'channels', label: 'Channels', kind: 'number', get: (t) => t.channels ?? null, hint: '2' },
  { id: 'size', label: 'File size (MB)', kind: 'number', get: (t) => (t.size ? t.size / 1048576 : null), hint: '30', tol: 0.5 },
  { id: 'gain', label: 'ReplayGain (dB)', kind: 'number', get: (t) => t.replaygain_track_db ?? t.replaygain_album_db ?? null, hint: '-6.5', signed: true, tol: 0.05 },
  { id: 'lyrics', label: 'Has lyrics in its tags', kind: 'bool', get: (t) => !!t.has_embedded_lyrics },
  { id: 'added', label: 'Date added', kind: 'date', get: (t) => t.added || t.modified || 0 },
  { id: 'modified', label: 'File last changed', kind: 'date', get: (t) => t.modified || 0 },
  { id: 'lastPlayed', label: 'Last played', kind: 'date', get: (t, ctx) => stat(ctx, t).last_played || 0, never: true },
  { id: 'plays', label: 'Times played', kind: 'number', get: (t, ctx) => stat(ctx, t).plays, hint: '5', always: true },
  { id: 'favourite', label: 'Favourite', kind: 'bool', get: (t, ctx) => !!ctx.favs?.has(t.path) },
  { id: 'playlist', label: 'Playlist', kind: 'playlist' },
];
export const fieldById = (id) => FIELDS.find((f) => f.id === id);

export const OPS = {
  text: [['contains', 'contains'], ['not', 'does not contain'], ['is', 'is exactly'], ['starts', 'starts with'], ['ends', 'ends with'], ['empty', 'is empty'], ['filled', 'is not empty']],
  number: [['eq', 'is'], ['ge', 'is at least'], ['le', 'is at most'], ['between', 'is between'], ['empty', 'is not set'], ['filled', 'is set']],
  date: [['last', 'is within the last'], ['before', 'is before'], ['after', 'is after'], ['on', 'is on'], ['between', 'is between'], ['never', 'never happened'], ['ever', 'has a date']],
  choice: [['any', 'is any of'], ['none', 'is none of']],
  bool: [['yes', 'yes'], ['no', 'no']],
  playlist: [['in', 'is in'], ['notin', 'is not in']],
};
/** The operators that make sense for a field (a count is never "not set"; only some dates can be "never"). */
export const opsFor = (f) => OPS[f.kind].filter(([op]) => !(f.always && (op === 'empty' || op === 'filled')) && !(f.kind === 'date' && !f.never && (op === 'never' || op === 'ever')));
export const DATE_UNITS = [['days', 'days'], ['weeks', 'weeks'], ['months', 'months'], ['years', 'years']];

/** Operators that need no value. */
const NO_VALUE = new Set(['empty', 'filled', 'never', 'ever', 'yes', 'no']);

export const newRule = (field = 'artist') => {
  const f = fieldById(field) ?? fieldById('artist');
  return { id: Math.random().toString(36).slice(2, 9), field, op: opsFor(f)[0][0], a: f.kind === 'choice' || f.kind === 'playlist' ? [] : '', b: '', unit: 'days' };
};
export const emptyFilter = () => ({ text: '', match: 'all', rules: [] });

/** `3:30`, `1:02:03`, `3.5` (minutes) or `210s` as seconds. */
export function parseTime(s) {
  const v = String(s ?? '').trim().toLowerCase();
  if (!v) return null;
  if (v.endsWith('s') && /^\d+(\.\d+)?s$/.test(v)) return parseFloat(v);
  if (v.includes(':')) {
    const parts = v.split(':').map((p) => p.trim());
    if (parts.some((p) => !/^\d+(\.\d+)?$/.test(p))) return null;
    return parts.reduce((acc, p) => acc * 60 + parseFloat(p), 0);
  }
  return /^\d+(\.\d+)?$/.test(v) ? parseFloat(v) * 60 : null;
}

function parseNumber(f, s) {
  if (f.parse === 'time') return parseTime(s);
  const v = String(s ?? '').trim().replace(',', '.');
  if (!v || !/^-?\d+(\.\d+)?$/.test(v)) return null;
  const n = parseFloat(v);
  return f.signed || n >= 0 ? n : null;
}

/** A date from an `<input type="date">` value (yyyy-mm-dd) as seconds, at the start of that local day. */
export function dayStart(s) {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(String(s ?? ''));
  return m ? new Date(+m[1], +m[2] - 1, +m[3]).getTime() / 1000 : null;
}

/** Seconds since the epoch for "N units ago", counted back from `now` (seconds). */
export function ago(n, unit, now) {
  if (unit === 'days') return now - n * DAY;
  if (unit === 'weeks') return now - n * 7 * DAY;
  const d = new Date(now * 1000);
  if (unit === 'months') d.setMonth(d.getMonth() - n); else d.setFullYear(d.getFullYear() - n);
  return d.getTime() / 1000;
}

/** Whether the rule has what it needs to be applied. */
export function ruleActive(r) {
  const f = fieldById(r.field);
  if (!f) return false;
  if (NO_VALUE.has(r.op)) return true;
  switch (f.kind) {
    case 'text': return String(r.a ?? '').trim() !== '';
    case 'number': return parseNumber(f, r.a) !== null && (r.op !== 'between' || parseNumber(f, r.b) !== null);
    case 'date':
      if (r.op === 'last') return /^\d+$/.test(String(r.a).trim()) && +r.a > 0;
      return dayStart(r.a) !== null && (r.op !== 'between' || dayStart(r.b) !== null);
    case 'choice': case 'playlist': return Array.isArray(r.a) && r.a.length > 0;
    default: return true;
  }
}

/** A function that says whether a track passes the rule. `ctx` = { favs, stats, playlists, now }. */
function matcher(r, ctx) {
  const f = fieldById(r.field);
  const now = ctx.now ?? Date.now() / 1000;
  switch (f.kind) {
    case 'text': {
      const needle = fold(r.a).trim();
      const words = needle.split(/\s+/).filter(Boolean);
      const val = (t) => fold(f.id === 'path' ? t.path : f.get(t, ctx));
      return {
        contains: (t) => { const v = val(t); return words.every((w) => v.includes(w)); },
        not: (t) => { const v = val(t); return !words.every((w) => v.includes(w)); },
        is: (t) => val(t).trim() === needle,
        starts: (t) => (f.id === 'path' ? fold(baseName(t.path)) : val(t)).startsWith(needle),
        ends: (t) => (f.id === 'path' ? fold(baseName(t.path)).replace(/\.[^.]+$/, '') : val(t)).endsWith(needle),
        empty: (t) => val(t).trim() === '',
        filled: (t) => val(t).trim() !== '',
      }[r.op];
    }
    case 'number': {
      const a = parseNumber(f, r.a), b = parseNumber(f, r.b), tol = f.tol ?? 1e-9;
      // Zero means "not in the tags" for sizes, rates and the like; only counts and gain keep a real zero.
      const get = (t) => { const v = f.get(t, ctx); return v === null || v === undefined || (v === 0 && !f.always && f.id !== 'gain') ? null : v; };
      const lo = a !== null && b !== null ? Math.min(a, b) : a, hi = a !== null && b !== null ? Math.max(a, b) : b;
      return {
        eq: (t) => { const v = get(t); return v !== null && Math.abs(v - a) <= tol; },
        ge: (t) => { const v = get(t); return v !== null && v >= a - 1e-9; },
        le: (t) => { const v = get(t); return v !== null && v <= a + 1e-9; },
        between: (t) => { const v = get(t); return v !== null && v >= lo - 1e-9 && v <= hi + 1e-9; },
        empty: (t) => get(t) === null,
        filled: (t) => get(t) !== null,
      }[r.op];
    }
    case 'date': {
      const a = r.op === 'last' ? ago(+r.a, r.unit, now) : dayStart(r.a);
      const b = r.op === 'between' ? dayStart(r.b) : null;
      const lo = b !== null ? Math.min(a, b) : a, hi = b !== null ? Math.max(a, b) + DAY : null;
      const get = (t) => f.get(t, ctx) || 0;
      return {
        last: (t) => { const v = get(t); return v > 0 && v >= a; },
        before: (t) => { const v = get(t); return v > 0 && v < a; },
        after: (t) => { const v = get(t); return v >= a + DAY; },
        on: (t) => { const v = get(t); return v >= a && v < a + DAY; },
        between: (t) => { const v = get(t); return v >= lo && v < hi; },
        never: (t) => get(t) === 0,
        ever: (t) => get(t) > 0,
      }[r.op];
    }
    case 'choice': {
      const chosen = new Set(r.a);
      const hit = f.test ? (t) => [...chosen].some((q) => f.test(t, q)) : (t) => chosen.has(f.get(t, ctx));
      return r.op === 'none' ? (t) => !hit(t) : hit;
    }
    case 'bool': return r.op === 'yes' ? (t) => !!f.get(t, ctx) : (t) => !f.get(t, ctx);
    case 'playlist': {
      const sets = ctx.playlists ?? new Map();
      const ids = r.a.includes('*') ? [...sets.keys()] : r.a;
      const hit = (t) => ids.some((id) => sets.get(id)?.has(t.path));
      return r.op === 'notin' ? (t) => !hit(t) : hit;
    }
    default: return null;
  }
}

/** The ids of the playlists a filter needs the songs of (the caller loads them into `ctx.playlists`). */
export function playlistsNeeded(filter) {
  const ids = new Set();
  for (const r of filter.rules) if (r.field === 'playlist' && ruleActive(r)) r.a.forEach((id) => ids.add(id));
  return ids;
}

export const activeRules = (filter) => filter.rules.filter(ruleActive);
export const isFiltering = (filter) => !!filter.text.trim() || activeRules(filter).length > 0;

const hayCache = new WeakMap();
const haystack = (t) => {
  let h = hayCache.get(t);
  if (h === undefined) { h = fold(`${t.title}\n${t.artist}\n${t.album}\n${t.album_artist}\n${t.genre}\n${baseName(t.path)}`); hayCache.set(t, h); }
  return h;
};

/** The tracks that pass the filter, in their original order. */
export function applyFilter(tracks, filter, ctx = {}) {
  const words = fold(filter.text).split(/\s+/).filter(Boolean);
  const rules = activeRules(filter).map((r) => matcher(r, ctx)).filter(Boolean);
  if (!words.length && !rules.length) return tracks.slice();
  const any = filter.match === 'any';
  return tracks.filter((t) => {
    if (words.length) { const hay = haystack(t); if (!words.every((w) => hay.includes(w))) return false; }
    if (!rules.length) return true;
    return any ? rules.some((m) => m(t)) : rules.every((m) => m(t));
  });
}

/** Ways to order the results. Ties keep the library order (artist, album, disc, track). */
export const RESULT_SORTS = [
  ['library', 'Artist, album'], ['title', 'Title'], ['artist', 'Artist'], ['album', 'Album'],
  ['year-new', 'Year, newest first'], ['year-old', 'Year, oldest first'], ['length-long', 'Longest first'], ['length-short', 'Shortest first'],
  ['added', 'Recently added'], ['plays', 'Most played'], ['played', 'Recently played'], ['size', 'Largest files'], ['quality', 'Highest quality'],
];
const text = (a, b) => String(a).localeCompare(String(b), undefined, { numeric: true, sensitivity: 'base' });
const quality = (t) => (isHiRes(t) ? 3 : isLossless(t) ? 2 : 1) * 1e6 + (t.bit_depth ?? 0) * 1e3 + (t.bitrate_kbps ?? 0);
export function sortResults(list, sort, ctx = {}) {
  const key = {
    title: (a, b) => text(a.title, b.title), artist: (a, b) => text(a.artist, b.artist), album: (a, b) => text(a.album, b.album),
    'year-new': (a, b) => (b.year ?? 0) - (a.year ?? 0), 'year-old': (a, b) => (a.year || 9999) - (b.year || 9999),
    'length-long': (a, b) => b.duration_secs - a.duration_secs, 'length-short': (a, b) => a.duration_secs - b.duration_secs,
    added: (a, b) => (b.added || b.modified) - (a.added || a.modified), plays: (a, b) => stat(ctx, b).plays - stat(ctx, a).plays,
    played: (a, b) => stat(ctx, b).last_played - stat(ctx, a).last_played, size: (a, b) => (b.size ?? 0) - (a.size ?? 0), quality: (a, b) => quality(b) - quality(a),
  }[sort];
  if (!key) return list;
  // Array.prototype.sort is stable, so ties stay in library order.
  return [...list].sort(key);
}

/** The file types in the library, for the "File type" choices. */
export const formatsIn = (tracks) => [...new Set(tracks.map((t) => String(t.format ?? '').toUpperCase()).filter(Boolean))].sort();

/** A short line about a file, for the results list: `FLAC · 24-bit / 96 kHz` or `MP3 · 320 kbps`. */
export function detailLine(t) {
  const fmt = String(t.format ?? '').toUpperCase();
  const rate = t.sample_rate ? `${+(t.sample_rate / 1000).toFixed(1)} kHz` : '';
  if (isLossless(t)) return [fmt, [t.bit_depth ? `${t.bit_depth}-bit` : '', rate].filter(Boolean).join(' / ')].filter(Boolean).join(' · ');
  return [fmt, t.bitrate_kbps ? `${t.bitrate_kbps} kbps` : rate].filter(Boolean).join(' · ');
}

/** Quick filters offered as one-click chips. */
export const PRESETS = [
  { id: 'lossless', label: 'Lossless', rules: [{ field: 'quality', op: 'any', a: ['lossless'] }] },
  { id: 'hires', label: 'Hi-res', rules: [{ field: 'quality', op: 'any', a: ['hires'] }] },
  { id: 'lossy', label: 'Lossy', rules: [{ field: 'quality', op: 'any', a: ['lossy'] }] },
  { id: 'fav', label: 'Favourites', rules: [{ field: 'favourite', op: 'yes' }] },
  { id: 'unplayed', label: 'Never played', rules: [{ field: 'plays', op: 'eq', a: '0' }] },
  { id: 'month', label: 'Added this month', rules: [{ field: 'added', op: 'last', a: '30', unit: 'days' }] },
  { id: 'nolist', label: 'In no playlist', rules: [{ field: 'playlist', op: 'notin', a: ['*'] }] },
  { id: 'nolyrics', label: 'No lyrics in tags', rules: [{ field: 'lyrics', op: 'no' }] },
  { id: 'nogenre', label: 'No genre', rules: [{ field: 'genre', op: 'empty' }] },
  { id: 'noyear', label: 'No year', rules: [{ field: 'year', op: 'empty' }] },
];
