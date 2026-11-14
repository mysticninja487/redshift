// A stand-in for the Tauri backend so the interface runs in a plain browser (for development,
// screenshots and tests). It fakes a small library of invented albums, a player that keeps time,
// synthetic spectrum frames, lyrics, an equaliser and playlists. Nothing here is real audio.
// Loaded only when window.__TAURI__ is missing; see js/main.js.
//
// URL switches: ?empty (no library), ?seed=N (different synthetic music), ?paused (start paused),
// ?big=N (N extra invented tracks, for long queues), ?latency=ms (delay every command, like real IPC).

const params = new URLSearchParams(location.search);
const rnd = (seed) => () => { seed = (seed * 1664525 + 1013904223) >>> 0; return seed / 4294967296; };
const hash = (s) => { let h = 2166136261; for (const c of s) h = Math.imul(h ^ c.charCodeAt(0), 16777619); return h >>> 0; };
const SEED = Number(params.get('seed') ?? 7);
const LATENCY = Number(params.get('latency') ?? 0) || 0;

// ---------------------------------------------------------------- library
const ALBUMS = [
  { artist: 'Halden Vey', title: 'Low Sun Over Water', year: 2021, hue: 18, style: 'sun', codec: 'flac', rate: 96000, bits: 24, titles: ['Harbour Light', 'Slow Tide', 'Copper Hour', 'Salt on the Glass', 'We Left the Radio On', 'Long Way Round', 'Lowlands', 'Last Boat Home'] },
  { artist: 'Mira Solano', title: 'Night Shift Symphony', year: 2019, hue: 265, style: 'rings', codec: 'flac', rate: 44100, bits: 16, titles: ['Neon Corridor', 'Seventh Floor', 'Static Bloom', 'The Quiet Elevator', 'Overtime', 'City Asleep', 'Blue Hour Radio'] },
  { artist: 'The Lantern Corps', title: 'Paper Maps', year: 2016, hue: 48, style: 'stripes', codec: 'opus', rate: 48000, bits: null, titles: ['Fold Lines', 'North Is Relative', 'Compass Rose', 'Ink and Orbit', 'A Door in Lisbon', 'Cartographer', 'Waypoint', 'Unmarked Road', 'Return Address'] },
  { artist: 'Okonkwo & Reyes', title: 'Glass Garden', year: 2023, hue: 160, style: 'blocks', codec: 'flac', rate: 48000, bits: 24, titles: ['Greenhouse', 'Condensation', 'Fern Logic', 'Pollen Count', 'Dew Point', 'After the Rain Machine'] },
  { artist: 'Paper Satellites', title: 'Signal & Noise', year: 2012, hue: 0, style: 'grey', codec: 'opus', rate: 48000, bits: null, titles: ['Carrier Wave', 'Ghost Frequency', 'Dial Tone', 'Interference', 'Sign Off', 'Test Pattern'] },
  { artist: 'Vesper Lines', title: 'Afterglow Index', year: 2024, hue: 330, style: 'none', codec: 'flac', rate: 192000, bits: 24, titles: ['Pink Noise', 'Gradient', 'Index Finger', 'Late Bloomer', 'Soft Reset', 'Afterglow', 'Dimmer'] },
];
const LYRIC_WORDS = ['light', 'water', 'hold', 'slow', 'home', 'again', 'tonight', 'quiet', 'open', 'signal', 'rain', 'wire', 'gold', 'turn', 'stay', 'wake', 'glass', 'river', 'shadow', 'blue', 'engine', 'morning', 'paper', 'drift'];

const NOW = Math.floor(Date.now() / 1000);
function makeLibrary() {
  const tracks = [];
  for (const [ai, a] of ALBUMS.entries()) {
    a.titles.forEach((title, i) => {
      const r = rnd(hash(a.title + title) + SEED);
      const path = `/music/${a.artist}/${a.title}/${String(i + 1).padStart(2, '0')} ${title}.${a.codec}`;
      tracks.push({
        path, title, artist: a.artist, album: a.title, album_artist: a.artist, genre: ['Indie', 'Electronic', 'Folk', 'Ambient'][ai % 4], year: a.year,
        track_no: i + 1, disc_no: 1, duration_secs: Math.round(150 + r() * 140), sample_rate: a.rate, bit_depth: a.bits, channels: 2,
        bitrate_kbps: a.codec === 'opus' ? 128 : null, format: a.codec, replaygain_track_db: +(-3 - r() * 6).toFixed(1), replaygain_album_db: -5.2,
        replaygain_track_peak: 0.97, has_embedded_lyrics: false, modified: 1, size: 1e7, added: NOW - Math.floor((ai * 9 + i * 0.7) * 86400 * 0.6 + r() * 3600), _album: ai,
      });
    });
  }
  // ?big=N adds N more tracks across invented albums, to test long queues and libraries.
  const extra = Math.max(0, Math.min(20000, Number(params.get('big') ?? 0) || 0));
  for (let i = 0; i < extra; i++) {
    const a = ALBUMS[i % ALBUMS.length], album = Math.floor(i / 12), r = rnd(hash(`big${i}`) + SEED);
    const title = `${a.titles[i % a.titles.length]} ${i}`;
    tracks.push({
      path: `/music/${a.artist}/Mix ${album}/${String(i % 12 + 1).padStart(2, '0')} ${title}.${a.codec}`, title, artist: a.artist, album: `Mix ${album}`, album_artist: a.artist,
      genre: 'Mixed', year: 2000 + (i % 24), track_no: i % 12 + 1, disc_no: 1, duration_secs: Math.round(120 + r() * 200), sample_rate: a.rate, bit_depth: a.bits, channels: 2,
      bitrate_kbps: null, format: a.codec, replaygain_track_db: -6, replaygain_album_db: -6, replaygain_track_peak: 0.9, has_embedded_lyrics: false, modified: 1, size: 1e7, added: NOW - i * 60, _album: i % ALBUMS.length,
    });
  }
  return tracks;
}
let tracks = params.has('empty') ? [] : makeLibrary();
let folders = params.has('empty') ? [] : ['/music'];
const byPath = () => new Map(makeLibrary().map((t) => [t.path, t]));
const pub = (t) => { const { _album, ...rest } = t; return rest; };
const find = (path) => tracks.find((t) => t.path === path) ?? byPath().get(path);

function albums() {
  const map = new Map();
  for (const t of tracks) { const k = `${t.album_artist}\u0000${t.album}`; (map.get(k) ?? map.set(k, []).get(k)).push(t); }
  return [...map.values()].map((ts) => ({ title: ts[0].album, artist: ts[0].album_artist, year: ts[0].year, track_count: ts.length, duration_secs: ts.reduce((s, t) => s + t.duration_secs, 0), tracks: ts.sort((a, b) => a.track_no - b.track_no).map((t) => t.path) }))
    .sort((a, b) => a.artist.localeCompare(b.artist) || a.title.localeCompare(b.title));
}
const artists = () => {
  const m = new Map();
  for (const a of albums()) { const e = m.get(a.artist) ?? { name: a.artist, album_count: 0, track_count: 0 }; e.album_count++; e.track_count += a.track_count; m.set(a.artist, e); }
  return [...m.values()].sort((a, b) => a.name.localeCompare(b.name));
};

// ---------------------------------------------------------------- covers
const coverCache = new Map();
window.__REDSHIFT_FONT__ = (file) => `fonts/${file}`;
window.__REDSHIFT_COVER__ = (path) => {
  const t = find(decodeURIComponent(path));
  if (!t) return 'data:,';
  const ec = EDITS.get(t.path)?.cover;
  if (ec === 'none') return 'missing://no-cover.png';
  if (ec) return ec;
  const a = ALBUMS[t._album ?? 0];
  if (a.style === 'none') return 'missing://no-cover.png'; // exercises the "no cover" fallback
  if (coverCache.has(a.title)) return coverCache.get(a.title);
  const c = document.createElement('canvas'); c.width = c.height = 400;
  const x = c.getContext('2d'), r = rnd(hash(a.title)), H = a.hue;
  const col = (h, s, l) => (a.style === 'grey' ? `hsl(0 0% ${l}%)` : `hsl(${h} ${s}% ${l}%)`);
  const g = x.createLinearGradient(0, 0, 400, 400);
  g.addColorStop(0, col(H, 55, 22)); g.addColorStop(1, col(H + 40, 60, 55));
  x.fillStyle = g; x.fillRect(0, 0, 400, 400);
  if (a.style === 'sun') { x.fillStyle = col(H + 12, 90, 62); x.beginPath(); x.arc(200, 230, 110, 0, 7); x.fill(); x.fillStyle = col(H, 50, 14); x.fillRect(0, 262, 400, 140); for (let i = 0; i < 6; i++) { x.fillStyle = col(H + 12, 80, 60 - i * 6); x.fillRect(70 + i * 18, 290 + i * 14, 260 - i * 36, 4); } }
  if (a.style === 'rings') { for (let i = 9; i > 0; i--) { x.strokeStyle = col(H + i * 9, 80, 40 + i * 5); x.lineWidth = 8; x.beginPath(); x.arc(200, 200, i * 20, 0, 7); x.stroke(); } x.fillStyle = col(H + 60, 90, 70); x.beginPath(); x.arc(200, 200, 14, 0, 7); x.fill(); }
  if (a.style === 'stripes') { for (let i = -8; i < 14; i++) { x.fillStyle = col(H + (i % 3) * 25, 75, 30 + (i % 4) * 14); x.beginPath(); x.moveTo(i * 36, 0); x.lineTo(i * 36 + 20, 0); x.lineTo(i * 36 + 220, 400); x.lineTo(i * 36 + 200, 400); x.fill(); } }
  if (a.style === 'blocks') { for (let i = 0; i < 28; i++) { x.fillStyle = col(H + r() * 70, 60, 25 + r() * 45); x.fillRect(r() * 340, r() * 340, 40 + r() * 90, 40 + r() * 90); } }
  if (a.style === 'grey') { for (let i = 0; i < 40; i++) { x.fillStyle = `hsl(0 0% ${15 + r() * 70}%)`; x.fillRect(0, i * 10, 400, 4 + r() * 5); } }
  const url = c.toDataURL('image/png');
  coverCache.set(a.title, url);
  return url;
};

// ---------------------------------------------------------------- lyrics
const EDITS = new Map(); // path -> { comment, composer, track_total, disc_total, lyrics (text | null), cover ('none' | data URL) }
const edit = (path) => EDITS.get(path) ?? EDITS.set(path, {}).get(path);
function parseMockLyrics(text) {
  const lines = [];
  for (const raw of text.split(/\r?\n/)) {
    const m = /^\[(\d+):(\d+)(?:\.(\d+))?\](.*)$/.exec(raw.trim());
    if (m) lines.push({ time_ms: (+m[1] * 60 + +m[2]) * 1000 + Math.round(+(`0.${m[3] ?? 0}`) * 1000), text: m[4].trim(), words: [] });
    else if (raw.trim() && !/^\[[a-z]+:/i.test(raw.trim())) lines.push({ time_ms: 0, text: raw.trim(), words: [] });
  }
  return { synced: lines.some((l) => l.time_ms > 0), lines, offset_ms: 0, source: 'embedded' };
}
function lyricsFor(path) {
  const e = EDITS.get(path);
  if (e && 'lyrics' in e) return e.lyrics ? parseMockLyrics(e.lyrics) : null;
  const t = find(path);
  if (!t || t._album === 4) return null; // one album has none
  const r = rnd(hash(path) + SEED), lines = [], n = Math.round(t.duration_secs / 6);
  const synced = t._album !== 3;
  const start = 9000, span = (t.duration_secs - 20) * 1000 / n;
  for (let i = 0; i < n; i++) {
    if (i % 8 === 7) { continue; }
    const words = Array.from({ length: 3 + Math.floor(r() * 4) }, () => LYRIC_WORDS[Math.floor(r() * LYRIC_WORDS.length)]);
    const time = Math.round(start + i * span);
    const ws = words.map((w, k) => ({ time_ms: time + Math.round((k / words.length) * span * 0.8), text: (k ? ' ' : '') + w }));
    lines.push({ time_ms: synced ? time : 0, text: words.join(' ').replace(/^./, (c) => c.toUpperCase()), words: t._album === 1 ? ws : [] });
  }
  return { synced, lines, offset_ms: 0, source: synced ? 'lrc' : 'embedded' };
}

// ---------------------------------------------------------------- player
const listeners = {};
const emit = (name, payload) => (listeners[name] ?? []).forEach((cb) => setTimeout(() => cb({ payload })));
const S = {
  entries: [], seq: 1, cur: null, status: 'stopped', pos: 0, t0: 0, volume: 0.8, muted: false, shuffle: false, repeat: 'off',
  crossfade: 4, rg: 'track', rgPre: 0, device: null,
  eq: { enabled: true, preamp_db: 0, gains_db: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0] },
};
const EQ_FREQS = [31, 62, 125, 250, 500, 1000, 2000, 4000, 8000, 16000];
const CUSTOM_PRESETS = [];
let SLEEP = null;
const sleepStatus = () => (!SLEEP ? { active: false, kind: null, remaining_secs: null, fading: false } : SLEEP.kind === 'song' ? { active: true, kind: 'song', remaining_secs: null, fading: false } : { active: true, kind: 'time', remaining_secs: Math.max(0, (SLEEP.end - performance.now()) / 1000), fading: false });
const TRAY = { minimize_to_tray: false, close_to_tray: false };
const FONTS = [];
const FAVS = []; // most recent first
const STATS = new Map(); // path -> { plays, last_played }
const PRESETS = [['Flat', [0, 0, 0, 0, 0, 0, 0, 0, 0, 0]], ['Bass Boost', [6, 5, 4, 2, 0.5, 0, 0, 0, 0, 0]], ['Bass Reducer', [-6, -5, -4, -2, -0.5, 0, 0, 0, 0, 0]], ['Treble Boost', [0, 0, 0, 0, 0, 0.5, 2, 4, 5, 6]], ['Vocal', [-2, -2, -1, 1, 3, 3.5, 3, 1.5, 0, -1]], ['Rock', [4.5, 3.5, 2, -1, -2, -0.5, 2, 3.5, 4.5, 4.5]], ['Pop', [-1.5, 1, 3, 4, 3, 0.5, -1, -1.5, -1.5, -1.5]], ['Jazz', [3, 2, 1, 2, -1.5, -1.5, 0, 1, 2, 3]], ['Classical', [3.5, 2.5, 2, 1.5, -1, -1, 0, 2, 2.5, 3]], ['Electronic', [5, 4, 1, 0, -2, 2, 1, 1.5, 4, 5]], ['Loudness', [6, 4, 0, 0, -1, 0, -0.5, 1, 4, 5]], ['Acoustic', [3, 3, 2, 0.5, 1, 1, 2, 2.5, 3, 2]], ['Hip-Hop', [5, 4.5, 2, 3, -1, -0.5, 1.5, -0.5, 2, 3]], ['R&B', [2.5, 6, 5, 1.5, -2, -1.5, 2, 2.5, 2.5, 3]], ['Dance', [6, 4.5, 2.5, 0, 0, -2.5, -3, -3, 0, 0]], ['Metal', [4, 3, 0, -2, -1, 1, 3.5, 4, 3, 2]], ['Piano', [1.5, 0.5, 0, 1.5, 2.5, 2.5, 3, 2.5, 3, 2.5]], ['Lounge', [-3, -1.5, 0, 1.5, 3, 2, 0, -1.5, 2, 1]], ['Live', [-2.5, 0, 2, 2.5, 3, 3, 3, 2, 1.5, 1.5]], ['Small Speakers', [6, 5, 4, 3, 2, 0, -1, -2, -2.5, -3]], ['Late Night', [4, 3, 0, 0, 1, 2, 1.5, -1, -2, -3]], ['Headphones', [4, 3, 1.5, -1, -1, 0.5, 1.5, 3, 3.5, 3]], ['Spoken Word', [-5, -4, -2, 1, 3, 4, 4, 3, 1, -2]]];

const position = () => (S.status === 'playing' ? S.pos + (performance.now() - S.t0) / 1000 : S.pos);
// The queue as the player plays it: the list order, or the shuffled order when shuffle is on.
const shown = () => {
  if (!S.shuffle) return S.entries;
  const have = new Map(S.entries.map((e) => [e.id, e]));
  S.order = (S.order ?? []).filter((id) => have.has(id));
  const seen = new Set(S.order);
  for (const e of S.entries) if (!seen.has(e.id)) S.order.push(e.id);
  return S.order.map((id) => have.get(id));
};
const reshuffle = () => {
  const ids = S.entries.map((e) => e.id).filter((id) => id !== S.cur);
  for (let i = ids.length - 1; i > 0; i--) { const j = Math.floor(Math.random() * (i + 1)); [ids[i], ids[j]] = [ids[j], ids[i]]; }
  S.order = S.cur != null ? [S.cur, ...ids] : ids;
};
const curEntry = () => S.entries.find((e) => e.id === S.cur) ?? null;
const curTrack = () => { const e = curEntry(); return e ? find(e.path) : null; };
function tick() {
  const t = curTrack();
  if (S.status === 'playing' && t && position() >= t.duration_secs) {
    if (S.repeat === 'one') { S.pos = 0; S.t0 = performance.now(); return; }
    go(1, true);
  }
}
setInterval(tick, 250);

function noteStart(path) {
  if (!path) return;
  const st = STATS.get(path) ?? { plays: 0, last_played: 0 };
  st.last_played = Math.floor(Date.now() / 1000);
  STATS.set(path, st);
}
function noteCounted(path) { const st = STATS.get(path); if (st) st.plays++; }
setInterval(() => { if (S.status === 'playing' && position() > 5) { const e = curEntry(); if (e && !S.counted?.has(e.id)) { (S.counted ??= new Set()).add(e.id); noteCounted(e.path); } } }, 1000);

function go(dir, auto = false) {
  if (!S.entries.length) return;
  const list = shown();
  let i = list.findIndex((e) => e.id === S.cur) + dir;
  if (i >= list.length) { if (S.repeat === 'all') i = 0; else if (auto) { S.status = 'stopped'; S.pos = 0; emit('player-event', { type: 'ended' }); return; } else return; }
  if (i < 0) { if (position() > 3) { S.pos = 0; S.t0 = performance.now(); return; } i = 0; }
  S.cur = list[i].id; S.pos = 0; S.t0 = performance.now(); S.status = 'playing';
  noteStart(curEntry()?.path);
  emit('player-event', { type: 'track_changed', entry_id: S.cur });
}
const queueView = () => {
  const t = curTrack();
  return {
    entries: shown().map((e) => ({ id: e.id, path: e.path })), current_entry: S.cur, shuffle: S.shuffle, repeat: S.repeat,
    current: t ? { entry_id: S.cur, path: t.path, codec: t.format, source_rate: t.sample_rate, source_channels: 2, bits_per_sample: t.bit_depth, duration_secs: t.duration_secs } : null,
  };
};
const playerState = () => ({ status: S.status, position_secs: Math.min(position(), curTrack()?.duration_secs ?? 0), volume: S.volume, muted: S.muted, crossfade_secs: S.crossfade, replaygain: S.rg, replaygain_preamp_db: S.rgPre, device_name: 'Speakers (mock)', sample_rate: 48000 });

// ---------------------------------------------------------------- analysis
function vizFrame() {
  const playing = S.status === 'playing', now = performance.now() / 1000;
  const k = playing ? 1 : 0, t = curTrack(), seed = t ? hash(t.path) % 100 : 0;
  const beat = (now * (1.9 + (seed % 7) * 0.05)) % 1, kick = playing ? Math.pow(1 - beat, 3) : 0;
  const bars = Array.from({ length: 64 }, (_, i) => {
    const f = i / 63, base = (1 - f) ** 1.6 * 0.75 + 0.12;
    const mod = 0.5 + 0.5 * Math.sin(now * (2 + f * 9) + i * 1.7 + seed) * Math.sin(now * 0.7 + i * 0.3);
    return Math.max(0, Math.min(1, k * (base * (0.45 + 0.55 * mod) + (i < 8 ? kick * 0.35 : 0))));
  });
  S.peaks = (S.peaks ?? bars.map(() => 0)).map((p, i) => Math.max(bars[i], p - 0.012));
  const scope = (ph) => Array.from({ length: 256 }, (_, i) => k * (0.5 * Math.sin(i * 0.12 + now * 9 + ph) + 0.28 * Math.sin(i * 0.37 + now * 4) + 0.12 * Math.sin(i * 0.9 + now * 17)) * (0.6 + kick * 0.5));
  const l = k * (0.35 + 0.4 * Math.abs(Math.sin(now * 3.1 + seed))) * (0.6 + kick * 0.4);
  return { bars, peaks: S.peaks, scope_left: scope(0), scope_right: scope(1.3), rms_left: l * 0.7, rms_right: l * 0.66, peak_left: Math.min(1, l * 1.2), peak_right: Math.min(1, l * 1.1), bass: kick * k };
}
function waveform(path, buckets) {
  const t = find(path), r = rnd(hash(path)), peaks = [], rms = [];
  let env = 0.5;
  for (let i = 0; i < buckets; i++) {
    const f = i / buckets, sect = 0.45 + 0.4 * Math.sin(f * Math.PI * 6 + hash(path) % 5) ** 2;
    env += (r() - 0.5) * 0.25; env = Math.max(0.2, Math.min(1, env));
    const p = Math.min(1, sect * env * (0.6 + r() * 0.5) * (f < 0.03 || f > 0.97 ? 0.4 : 1));
    peaks.push(+p.toFixed(3)); rms.push(+(p * (0.45 + r() * 0.2)).toFixed(3));
  }
  return { peaks, rms, duration_secs: t?.duration_secs ?? 200 };
}
function eqCurve(points) {
  return Array.from({ length: points }, (_, i) => {
    const f = 20 * 1000 ** (i / (points - 1));
    let db = S.eq.enabled ? S.eq.preamp_db : 0;
    if (S.eq.enabled) EQ_FREQS.forEach((c, b) => { const x = Math.log2(f / c); db += S.eq.gains_db[b] * Math.exp(-(x * x) / 0.9); });
    return [f, db];
  });
}

// ---------------------------------------------------------------- playlists, scan
const PL = { seq: 4, list: [
  { id: 1, name: 'Evening drive', tracks: tracks.filter((t) => t._album === 0 || t._album === 5).slice(0, 7).map((t) => t.path) },
  { id: 2, name: 'Focus', tracks: tracks.filter((t) => t._album === 3).map((t) => t.path) },
  { id: 3, name: 'driving down the roads of tokyo', tracks: [...tracks.filter((t) => t._album === 1).slice(0, 3).map((t) => t.path), '/storage/emulated/0/Music/Gone Band/Lost Album/01 Missing Song.opus', '/storage/emulated/0/Music/Gone Band/Lost Album/02 Another One.opus'] },
] };
const plSummary = () => PL.list.map((p) => ({ id: p.id, name: p.name, track_count: p.tracks.length, modified: 0 }));
if (params.has('empty')) PL.list = [];

let scanning = false;
async function scan() {
  scanning = true;
  const total = 120;
  for (let i = 0; i <= total; i += 8) { emit('scan-progress', { phase: i < 24 ? 'discovering' : 'reading', done: i, total, current: null }); await new Promise((r) => setTimeout(r, 40)); }
  if (!tracks.length && folders.length) tracks = makeLibrary();
  scanning = false;
  return { added: tracks.length, updated: 0, removed: 0, unchanged: 0, failed: [] };
}

if (!params.has('empty') && !params.has('fresh')) {
  tracks.slice(0, 18).forEach((t, i) => STATS.set(t.path, { plays: Math.max(0, 14 - i * 2 + (i % 3)), last_played: NOW - i * 5400 - (i % 4) * 777 }));
  STATS.get(tracks[2].path).plays = 21;
  FAVS.push(tracks[0].path, tracks[9].path, tracks[15].path, tracks[24].path);
}

// ---------------------------------------------------------------- commands
const ui = () => { try { return JSON.parse(localStorage.getItem('redshift-mock-ui') ?? 'null'); } catch { return null; } };
// ---------------------------------------------------------------- listening history (invented)
// ?nohistory starts with none. The real thing is computed in Rust (insights.rs); this only gives the screen something to show.
let HISTORY = null;
function history() {
  if (HISTORY) return HISTORY;
  HISTORY = [];
  if (params.has('nohistory') || params.has('empty')) return HISTORY;
  const r = rnd(hash('history') + SEED), mid = new Date(); mid.setHours(0, 0, 0, 0);
  const liked = tracks.filter((_, i) => i % 3 === 0), hours = [7, 8, 13, 18, 19, 20, 21, 22, 22, 23];
  const add = (t, path, dur) => { const secs = Math.round(dur * (0.4 + r() * 0.6)); HISTORY.push({ t, path, secs, plays: secs >= dur / 2 ? 1 : 0 }); };
  for (let d = 0; d < 400; d++) {
    if (d > 0 && r() < 0.18) continue;
    for (let s = 0, n = 1 + Math.floor(r() * 3); s < n; s++) {
      let t = Math.floor(mid.getTime() / 1000) - d * 86400 + hours[Math.floor(r() * hours.length)] * 3600 + Math.floor(r() * 3000);
      for (let k = 0, m = 3 + Math.floor(r() * 8); k < m; k++) {
        const pool = r() < 0.65 ? liked : tracks, tr = pool[Math.floor(r() * pool.length)];
        if (t < NOW - 60) add(t, tr.path, tr.duration_secs);
        t += tr.duration_secs;
      }
    }
  }
  if (!HISTORY.some((x) => x.t > Math.floor(mid.getTime() / 1000))) for (let i = 0; i < 4; i++) add(NOW - 600 - i * 400, tracks[i].path, tracks[i].duration_secs);
  return HISTORY;
}
const heardIn = (from, to) => history().filter((x) => x.t >= from && x.t < to);
function listeningReport(from, to, tz) {
  const rows = heardIn(from, to), per = new Map(), first = new Map(), hours = new Array(24).fill(0);
  for (const x of history()) first.set(x.path, Math.min(first.get(x.path) ?? Infinity, x.t));
  for (const x of rows) {
    const e = per.get(x.path) ?? per.set(x.path, { secs: 0, plays: 0 }).get(x.path); e.secs += x.secs; e.plays += x.plays;
    hours[Math.floor((x.t + tz * 60) / 3600) % 24 < 0 ? 0 : Math.floor((x.t + tz * 60) / 3600) % 24] += x.secs;
  }
  const lib = new Map(tracks.map((t) => [t.path, t]));
  const art = new Map(), alb = new Map(), gen = new Map(), dec = new Map(), list = [];
  let newT = 0;
  for (const [path, h] of per) {
    if (first.get(path) >= from && first.get(path) < to) newT++;
    if (h.plays === 0) continue; // only flicked past: counts as time, not as a played song
    const t = lib.get(path); if (!t) continue;
    list.push({ path, title: t.title, artist: t.artist, album: t.album, secs: h.secs, plays: h.plays });
    const a = art.get(t.artist.toLowerCase()) ?? art.set(t.artist.toLowerCase(), { name: t.artist, secs: 0, plays: 0, tracks: 0, path, best: -1 }).get(t.artist.toLowerCase());
    a.secs += h.secs; a.plays += h.plays; a.tracks++; if (h.secs > a.best) { a.best = h.secs; a.path = path; }
    const k = `${t.album}|${t.album_artist}`, b = alb.get(k) ?? alb.set(k, { album: t.album, artist: t.album_artist, secs: 0, plays: 0, path, best: -1 }).get(k);
    b.secs += h.secs; b.plays += h.plays; if (h.secs > b.best) { b.best = h.secs; b.path = path; }
    const gs = (t.genre || 'Unknown').split(';').map((g) => g.trim()).filter(Boolean);
    for (const g of gs) gen.set(g, (gen.get(g) ?? 0) + h.secs / gs.length);
    if (t.year) dec.set(Math.floor(t.year / 10) * 10, (dec.get(Math.floor(t.year / 10) * 10) ?? 0) + h.secs);
  }
  list.sort((a, b) => b.plays - a.plays || b.secs - a.secs);
  const strip = ({ best, ...x }) => x;
  return {
    secs: rows.reduce((n, x) => n + x.secs, 0), plays: rows.reduce((n, x) => n + x.plays, 0), tracks_heard: per.size, artists_heard: art.size, new_tracks: newT,
    top_tracks: list.slice(0, 10), top_artists: [...art.values()].sort((a, b) => b.secs - a.secs).slice(0, 10).map(strip), top_albums: [...alb.values()].sort((a, b) => b.secs - a.secs).slice(0, 10).map(strip),
    genres: [...gen].map(([name, secs]) => ({ name, secs })).sort((a, b) => b.secs - a.secs), hours, decades: [...dec].map(([decade, secs]) => ({ decade, secs })).sort((a, b) => a.decade - b.decade),
  };
}
function mockMixes() {
  const rep = listeningReport(NOW - 30 * 86400, NOW + 1, 0), out = [];
  const shuffle = (a) => { const r = rnd(hash(`mix${Math.floor(NOW / 86400)}`)); return [...a].sort(() => r() - 0.5); };
  if (rep.top_tracks.length >= 6) out.push({ id: 'repeat', kind: 'repeat', name: 'On repeat', blurb: 'What you\'ve played most in the last month.', tracks: rep.top_tracks.map((t) => t.path) });
  const g = rep.genres.find((x) => x.name !== 'Unknown');
  if (g) { const of = tracks.filter((t) => t.genre?.split(';').map((x) => x.trim()).includes(g.name)); if (of.length >= 6) out.push({ id: `genre-${g.name.toLowerCase()}`, kind: 'genre', name: `${g.name} mix`, blurb: `The ${g.name} you play, with a few you haven't heard yet.`, tracks: shuffle(of).slice(0, 40).map((t) => t.path) }); }
  const a = rep.top_artists[0];
  if (a) { const of = tracks.filter((t) => t.artist === a.name); if (of.length >= 3) out.push({ id: `artist-${a.name.toLowerCase().replace(/\W+/g, '-')}`, kind: 'artist', name: `${a.name} & more`, blurb: `${a.name}, with songs from the same world.`, tracks: shuffle(tracks).filter((t, i) => t.artist === a.name || i % 4 === 0).slice(0, 40).map((t) => t.path) }); }
  const heard = new Set(history().map((x) => x.path)), fresh = tracks.filter((t) => !heard.has(t.path));
  if (fresh.length >= 6) out.push({ id: 'fresh', kind: 'fresh', name: 'Fresh picks', blurb: 'Songs in your library you haven\'t played yet, picked from what you like.', tracks: shuffle(fresh).slice(0, 40).map((t) => t.path) });
  const old = [...new Set(history().filter((x) => x.t < NOW - 45 * 86400).map((x) => x.path))].filter((p) => !history().some((x) => x.path === p && x.t >= NOW - 45 * 86400));
  if (old.length >= 6) out.push({ id: 'rediscover', kind: 'rediscover', name: 'Take me back', blurb: 'Songs you\'ve loved, that you haven\'t played for a while.', tracks: shuffle(old).slice(0, 40) });
  if (FAVS.length >= 6) out.push({ id: 'favourites', kind: 'favourites', name: 'Your favourites', blurb: 'Songs you\'ve hearted, in a fresh order.', tracks: shuffle(FAVS).slice(0, 40) });
  return history().length ? out : out.filter((m) => m.kind === 'favourites');
}

const settings = () => ({ volume: S.volume, muted: S.muted, shuffle: S.shuffle, repeat: S.repeat, crossfade_secs: S.crossfade, replaygain: S.rg, replaygain_preamp_db: S.rgPre, eq: S.eq, device: S.device, viz: {}, custom_eq_presets: CUSTOM_PRESETS, tray: TRAY, restore_session: S.restore !== false, watch_folders: S.watch, record_history: S.record !== false, count_percent: S.countPercent ?? 50, queue_no_duplicates: S.noDup ?? false, keep_shuffle: S.keepShuffle ?? true, shuffle_whole_queue: S.wholeQueue ?? true, notify_on_track: S.notify ?? false, ui: ui() });
const addEntries = (paths, at = S.entries.length) => S.entries.splice(at, 0, ...paths.map((path) => ({ id: S.seq++, path })));
const cmds = {
  set_queue: ({ paths, start, autoplay }) => { S.entries = []; addEntries(paths); S.cur = S.entries[start]?.id ?? null; S.pos = 0; S.t0 = performance.now(); S.status = autoplay && S.cur ? 'playing' : 'stopped'; if (S.status === 'playing') noteStart(curEntry()?.path); emit('player-event', { type: 'queue_changed' }); emit('player-event', { type: 'track_changed', entry_id: S.cur }); },
  play_entry: ({ id }) => { S.cur = id; noteStart(curEntry()?.path); S.pos = 0; S.t0 = performance.now(); S.status = 'playing'; emit('player-event', { type: 'track_changed', entry_id: id }); },
  pause: () => { S.pos = position(); S.status = 'paused'; }, resume: () => { S.t0 = performance.now(); S.status = 'playing'; },
  toggle_mini_player: () => true, mini_size: () => {}, mini_on_top: () => {},
  toggle_pause: () => { if (S.status === 'playing') cmds.pause(); else if (S.status === 'paused') cmds.resume(); else if (S.entries.length) { S.cur ??= S.entries[0].id; S.pos = 0; S.t0 = performance.now(); S.status = 'playing'; emit('player-event', { type: 'track_changed', entry_id: S.cur }); } },
  stop: () => { S.status = 'stopped'; S.pos = 0; }, next: () => go(1), previous: () => go(-1),
  seek: ({ secs }) => { S.pos = secs; S.t0 = performance.now(); },
  queue_append: ({ paths }) => { addEntries(paths); S.cur ??= S.entries[0]?.id ?? null; emit('player-event', { type: 'queue_changed' }); },
  queue_play_next: ({ paths }) => { const i = S.entries.findIndex((e) => e.id === S.cur); addEntries(paths, i + 1); emit('player-event', { type: 'queue_changed' }); },
  queue_remove: ({ id }) => { const i = S.entries.findIndex((e) => e.id === id); if (i < 0) return; S.entries.splice(i, 1); if (S.cur === id) { S.cur = S.entries[i]?.id ?? S.entries[i - 1]?.id ?? null; S.pos = 0; S.t0 = performance.now(); if (!S.cur) S.status = 'stopped'; emit('player-event', { type: 'track_changed', entry_id: S.cur }); } emit('player-event', { type: 'queue_changed' }); },
  queue_move: ({ from, to }) => { if (S.shuffle) { shown(); const [id] = S.order.splice(from, 1); S.order.splice(to, 0, id); } else { const [e] = S.entries.splice(from, 1); S.entries.splice(to, 0, e); } emit('player-event', { type: 'queue_changed' }); },
  queue_clear: () => { S.entries = []; S.cur = null; S.status = 'stopped'; S.pos = 0; emit('player-event', { type: 'queue_changed' }); emit('player-event', { type: 'track_changed', entry_id: null }); },
  player_state: playerState, queue_view: queueView, get_settings: settings,
  set_volume: ({ volume }) => { S.volume = volume; }, set_muted: ({ muted }) => { S.muted = muted; },
  set_shuffle: ({ on }) => { S.shuffle = on; if (on) reshuffle(); emit('player-event', { type: 'queue_changed' }); }, set_repeat: ({ mode }) => { S.repeat = mode; },
  set_crossfade_secs: ({ secs }) => { S.crossfade = secs; }, set_replaygain: ({ mode, preampDb }) => { S.rg = mode; S.rgPre = preampDb; },
  output_devices: () => [{ id: 'spk', name: 'Speakers (Mock Audio)', is_default: true }, { id: 'hp', name: 'Headphones (Mock USB DAC)', is_default: false }],
  set_output_device: ({ id }) => { S.device = id; },
  eq_info: () => ({ settings: S.eq, band_freqs_hz: EQ_FREQS, presets: PRESETS, custom_presets: CUSTOM_PRESETS }),
  set_eq_enabled: ({ on }) => { S.eq.enabled = on; }, set_eq_preamp_db: ({ db }) => { S.eq.preamp_db = db; },
  set_eq_band_db: ({ band, db }) => { S.eq.gains_db[band] = db; }, set_eq_gains_db: ({ gains }) => { S.eq.gains_db = gains; },
  apply_eq_preset: ({ name }) => {
    const c = CUSTOM_PRESETS.find((x) => x.name.toLowerCase() === name.toLowerCase());
    if (c) { S.eq.gains_db = [...c.gains_db]; S.eq.preamp_db = c.preamp_db; return true; }
    const p = PRESETS.find((x) => x[0].toLowerCase() === name.toLowerCase()); if (p) S.eq.gains_db = [...p[1]]; return !!p;
  },
  save_eq_preset: ({ name }) => {
    const n = name.trim().replace(/\s+/g, ' ');
    if (!n) throw new Error('give the preset a name');
    if (PRESETS.some((p) => p[0].toLowerCase() === n.toLowerCase())) throw new Error(`"${n}" is a built-in preset name; pick another`);
    const i = CUSTOM_PRESETS.findIndex((x) => x.name.toLowerCase() === n.toLowerCase());
    const preset = { name: n, preamp_db: S.eq.preamp_db, gains_db: [...S.eq.gains_db] };
    if (i >= 0) CUSTOM_PRESETS[i] = preset; else CUSTOM_PRESETS.push(preset);
    return n;
  },
  delete_eq_preset: ({ name }) => { const i = CUSTOM_PRESETS.findIndex((x) => x.name.toLowerCase() === name.toLowerCase()); if (i >= 0) CUSTOM_PRESETS.splice(i, 1); return i >= 0; },
  set_sleep_timer: ({ minutes }) => { SLEEP = minutes == null ? { kind: 'song' } : minutes ? { kind: 'time', end: performance.now() + minutes * 60000 } : null; return sleepStatus(); },
  cancel_sleep_timer: () => { SLEEP = null; },
  sleep_status: () => sleepStatus(),
  listening_totals: ({ ranges }) => ranges.map(([a, b]) => { const rows = heardIn(a, b); return { secs: rows.reduce((n, x) => n + x.secs, 0), plays: rows.reduce((n, x) => n + x.plays, 0) }; }),
  listening_report: ({ from, to, tzOffsetMin }) => listeningReport(from, to, tzOffsetMin),
  app_info: () => ({ version: '0.5.4', data_dir: 'C:\\Users\\you\\AppData\\Roaming\\com.redshift.player', os: 'windows' }),
  open_data_folder: () => null,
  song_info: ({ path }) => {
    const t = tracks.find((x) => x.path === path); if (!t) throw new Error('That file isn\u2019t there any more');
    const h = history().filter((x) => x.path === path);
    return { meta: { ...t, size: 31_400_000, modified: NOW - 86400 * 40, added: NOW - 86400 * 30, replaygain_track_db: -6.4, replaygain_album_db: null, has_embedded_lyrics: false, bitrate_kbps: 962, bit_depth: 16, channels: 2, sample_rate: 44100, format: t.format ?? 'flac' },
      in_library: true, favourite: false, plays: h.reduce((n, x) => n + x.plays, 0), last_played: h.length ? Math.max(...h.map((x) => x.t)) : 0,
      listened_secs: h.reduce((n, x) => n + x.secs, 0), first_heard: h.length ? Math.min(...h.map((x) => x.t)) : null, lyrics: 'none' };
  },
  listening_streak: ({ tzOffsetMin }) => {
    const day = (t) => Math.floor((t + tzOffsetMin * 60) / 86400), today = day(NOW);
    const days = [...new Set(history().filter((x) => x.plays > 0).map((x) => day(x.t)))].sort((a, b) => a - b);
    let run = 0, longest = 0, prev = null;
    for (const d of days) { run = prev === d - 1 ? run + 1 : 1; longest = Math.max(longest, run); prev = d; }
    const playedToday = days.includes(today); let d = playedToday ? today : today - 1, current = 0;
    while (days.includes(d)) { current++; d--; }
    return { current, longest, total_days: days.length, played_today: playedToday, today, recent: days.filter((x) => x > today - 14 && x <= today) };
  },
  mixes: () => mockMixes(),
  clear_history: () => { HISTORY = []; },
  set_count_percent: ({ percent }) => { S.countPercent = Math.max(1, Math.min(100, percent)); },
  set_record_history: ({ on }) => { S.record = on; },
  set_watch_folders: ({ on }) => { S.watch = on; },
  set_queue_options: ({ noDuplicates, keepShuffle, shuffleWholeQueue }) => { S.noDup = noDuplicates; S.keepShuffle = keepShuffle; S.wholeQueue = shuffleWholeQueue; },
  set_notify_on_track: ({ on }) => { S.notify = on; },
  launch_on_startup: () => !!S.login,
  set_launch_on_startup: ({ on }) => { S.login = on; return on; },
  set_restore_session: ({ on }) => { S.restore = on; },
  set_tray: ({ minimizeToTray, closeToTray }) => { TRAY.minimize_to_tray = minimizeToTray; TRAY.close_to_tray = closeToTray; },
  favourite_paths: () => [...FAVS],
  set_favourite: ({ paths, on }) => { let n = 0; for (const p of paths) { const i = FAVS.indexOf(p); if (on && i < 0) { FAVS.unshift(p); n++; } else if (!on && i >= 0) { FAVS.splice(i, 1); n++; } } return n; },
  play_stats: () => [...STATS].map(([path, v]) => ({ path, ...v })),
  reset_play_stats: ({ path }) => { if (path) STATS.delete(path); else STATS.clear(); },
  fonts: () => FONTS,
  import_font: ({ path }) => { const file = path.split('/').pop(); const e = { file, family: file.replace(/\.[^.]+$/, '').replace(/[_-]/g, ' ') }; if (!FONTS.some((f) => f.file === file)) FONTS.push(e); return e; },
  remove_font: ({ file }) => { const i = FONTS.findIndex((f) => f.file === file); if (i >= 0) FONTS.splice(i, 1); return i >= 0; },
  paths_exist: ({ paths }) => paths.map((p) => !!find(p)),
  relink_playlist: ({ id }) => { const p = PL.list.find((x) => x.id === id); let relinked = 0, still = 0; p.tracks = p.tracks.map((t) => { if (find(t)) return t; const hit = tracks.find((x) => x.path.split('/').pop().toLowerCase() === t.split('/').pop().toLowerCase()); if (hit) { relinked++; return hit.path; } still++; return t; }); return { relinked, still_missing: still }; },
  eq_curve: ({ points }) => eqCurve(points),
  viz_frame: vizFrame, configure_viz: () => {},
  ui_state: ui, set_ui_state: ({ value }) => { localStorage.setItem('redshift-mock-ui', JSON.stringify(value)); },
  library_folders: () => folders, add_library_folder: ({ folder }) => { if (!folders.includes(folder)) folders.push(folder); }, remove_library_folder: ({ folder }) => { folders = folders.filter((f) => f !== folder); if (!folders.length) tracks = []; },
  scan_library: scan, is_scanning: () => scanning,
  library_tracks: () => tracks.map(pub), search_library: ({ query }) => tracks.filter((t) => `${t.title} ${t.artist} ${t.album}`.toLowerCase().includes(query.toLowerCase())).map(pub),
  library_albums: albums, library_artists: artists, recently_added: ({ limit }) => tracks.slice(-limit).map(pub),
  track_meta: ({ path }) => { const t = find(path); if (!t) throw new Error('unreadable'); return pub(t); },
  tag_form: ({ path }) => {
    const t = find(path); if (!t) throw new Error('unreadable'); const e = EDITS.get(path) ?? {};
    const ly = lyricsFor(path);
    const text = 'lyrics' in e ? (e.lyrics ?? '') : '';
    return { path, format: t.format, title: t.title, artist: t.artist, album: t.album, album_artist: t.album_artist, genre: t.genre, composer: e.composer ?? '', comment: e.comment ?? '',
      year: t.year, track_no: t.track_no, track_total: e.track_total ?? null, disc_no: t.disc_no, disc_total: e.disc_total ?? null, lyrics: text, has_cover: e.cover ? e.cover !== 'none' : ALBUMS[t._album ?? 0].style !== 'none', writable: !t.path.includes('Readonly'), _ly: !!ly };
  },
  write_tags: ({ path, edit: ed }) => {
    const t = tracks.find((x) => x.path === path); if (!t) throw new Error('unreadable');
    if (window.__mockFailWrite) throw new Error('couldn\'t replace the file (is something else using it?)');
    const e = edit(path);
    for (const k of ['title', 'artist', 'album', 'album_artist', 'genre', 'year', 'track_no', 'disc_no']) if (k in ed) t[k] = ed[k] ?? (k === 'title' ? path.split('/').pop() : k.includes('no') || k === 'year' ? null : '');
    for (const k of ['composer', 'comment', 'track_total', 'disc_total', 'lyrics']) if (k in ed) e[k] = ed[k];
    if (ed.cover === 'remove') e.cover = 'none';
    else if (ed.cover?.file) e.cover = window.__mockCover ?? e.cover;
    t.has_embedded_lyrics = !!(e.lyrics);
    emit('tags-changed', path);
    return pub(t);
  },
  write_look_file: ({ path, text }) => { (window.__mockFiles ??= {})[path] = text; },
  read_look_file: ({ path }) => { const t = (window.__mockFiles ??= {})[path]; if (t == null) throw new Error('no such file'); return t; },
  save_lyrics_file: ({ path }) => path.replace(/\.[^.]+$/, '.lrc'),
  image_preview: async () => { const c = document.createElement('canvas'); c.width = c.height = 120; const x = c.getContext('2d'); x.fillStyle = '#c0392b'; x.fillRect(0, 0, 120, 120); x.fillStyle = '#f1c40f'; x.fillRect(30, 30, 60, 60); return await new Promise((r) => c.toBlob(async (b) => r(await b.arrayBuffer()), 'image/png')); },
  lyrics: ({ path }) => lyricsFor(path), waveform: ({ path, buckets }) => waveform(path, buckets),
  playlists: plSummary, playlist: ({ id }) => PL.list.find((p) => p.id === id) ?? null,
  create_playlist: ({ name, tracks: ts }) => { const id = PL.seq++; PL.list.push({ id, name, tracks: ts ?? [] }); return id; },
  rename_playlist: ({ id, name }) => { PL.list.find((p) => p.id === id).name = name; }, delete_playlist: ({ id }) => { PL.list = PL.list.filter((p) => p.id !== id); },
  move_playlist: () => {}, add_to_playlist: ({ id, tracks: ts }) => { PL.list.find((p) => p.id === id).tracks.push(...ts); },
  remove_from_playlist: ({ id, positions }) => { const p = PL.list.find((x) => x.id === id); p.tracks = p.tracks.filter((_, i) => !positions.includes(i)); },
  move_in_playlist: ({ id, from, to }) => { const p = PL.list.find((x) => x.id === id); const [t] = p.tracks.splice(from, 1); p.tracks.splice(to, 0, t); },
  import_m3u: () => { const id = PL.seq++; PL.list.push({ id, name: 'playlist', tracks: [tracks[0].path, tracks[3].path, '/storage/emulated/0/Music/Gone Band/Lost Album/01 Missing.opus'] }); return { id, name: 'playlist', total: 3, matched: 2, relinked: 0, missing_count: 1, missing: ['/storage/emulated/0/Music/Gone Band/Lost Album/01 Missing.opus'] }; }, export_m3u: () => {}, play_playlist: ({ id, start }) => { const p = PL.list.find((x) => x.id === id); const ok = p.tracks.filter((t) => find(t)); if (!ok.length) throw new Error('none of the tracks in this playlist could be found on this PC'); cmds.set_queue({ paths: ok, start: Math.min(start, ok.length - 1), autoplay: true }); },
};

window.__TAURI__ = {
  core: { invoke: async (cmd, args = {}) => {
    const f = cmds[cmd]; if (!f) throw new Error(`mock: unknown command ${cmd}`);
    if (LATENCY && cmd !== 'viz_frame') await new Promise((r) => setTimeout(r, LATENCY * (0.5 + Math.random())));
    return f(args);
  } },
  event: { listen: async (name, cb) => { (listeners[name] ??= []).push(cb); return () => { listeners[name] = listeners[name].filter((x) => x !== cb); }; } },
  dialog: { open: async ({ directory } = {}) => window.__mockPick ?? (directory ? '/music/More Music' : '/music/playlist.m3u8'), save: async ({ defaultPath } = {}) => defaultPath ?? 'out' },
};

// Start with something playing so screenshots have content.
if (!params.has('empty')) {
  const first = albums()[0];
  window.__mockStart = () => cmds.set_queue({ paths: albums().flatMap((a) => a.tracks), start: 3, autoplay: !params.has('paused') });
  cmds.set_queue({ paths: albums().flatMap((a) => a.tracks), start: 3, autoplay: !params.has('paused') });
  S.pos = 38; S.t0 = performance.now(); void first;
}
