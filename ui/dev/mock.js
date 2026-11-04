// A stand-in for the Tauri backend so the interface runs in a plain browser (for development,
// screenshots and tests). It fakes a small library of invented albums, a player that keeps time,
// synthetic spectrum frames, lyrics, an equaliser and playlists. Nothing here is real audio.
// Loaded only when window.__TAURI__ is missing; see js/main.js.
//
// URL switches: ?empty (no library), ?seed=N (different synthetic music), ?paused (start paused).

const params = new URLSearchParams(location.search);
const rnd = (seed) => () => { seed = (seed * 1664525 + 1013904223) >>> 0; return seed / 4294967296; };
const hash = (s) => { let h = 2166136261; for (const c of s) h = Math.imul(h ^ c.charCodeAt(0), 16777619); return h >>> 0; };
const SEED = Number(params.get('seed') ?? 7);

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
        replaygain_track_peak: 0.97, has_embedded_lyrics: false, modified: 1, size: 1e7, _album: ai,
      });
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
window.__SONARA_COVER__ = (path) => {
  const t = find(decodeURIComponent(path));
  if (!t) return 'data:,';
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
function lyricsFor(path) {
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
const PRESETS = [['Flat', [0, 0, 0, 0, 0, 0, 0, 0, 0, 0]], ['Bass boost', [6, 5, 4, 2, 0, 0, 0, 0, 0, 0]], ['Vocal', [-2, -2, 0, 2, 4, 4, 3, 1, 0, -1]], ['Treble', [0, 0, 0, 0, 0, 1, 3, 5, 6, 6]], ['Loudness', [5, 3, 0, -1, -1, 0, 1, 3, 5, 4]], ['Acoustic', [3, 3, 2, 1, 1, 1, 2, 3, 3, 2]]];

const position = () => (S.status === 'playing' ? S.pos + (performance.now() - S.t0) / 1000 : S.pos);
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

function go(dir, auto = false) {
  if (!S.entries.length) return;
  let i = S.entries.findIndex((e) => e.id === S.cur);
  if (S.shuffle && dir > 0) i = Math.floor(Math.random() * S.entries.length);
  else i += dir;
  if (i >= S.entries.length) { if (S.repeat === 'all') i = 0; else if (auto) { S.status = 'stopped'; S.pos = 0; emit('player-event', { type: 'ended' }); return; } else return; }
  if (i < 0) { if (position() > 3) { S.pos = 0; S.t0 = performance.now(); return; } i = 0; }
  S.cur = S.entries[i].id; S.pos = 0; S.t0 = performance.now(); S.status = 'playing';
  emit('player-event', { type: 'track_changed', entry_id: S.cur });
}
const queueView = () => {
  const t = curTrack();
  return {
    entries: S.entries.map((e) => ({ id: e.id, path: e.path })), current_entry: S.cur, shuffle: S.shuffle, repeat: S.repeat,
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
const PL = { seq: 3, list: [
  { id: 1, name: 'Evening drive', tracks: tracks.filter((t) => t._album === 0 || t._album === 5).slice(0, 7).map((t) => t.path) },
  { id: 2, name: 'Focus', tracks: tracks.filter((t) => t._album === 3).map((t) => t.path) },
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

// ---------------------------------------------------------------- commands
const ui = () => { try { return JSON.parse(localStorage.getItem('sonara-mock-ui') ?? 'null'); } catch { return null; } };
const settings = () => ({ volume: S.volume, muted: S.muted, shuffle: S.shuffle, repeat: S.repeat, crossfade_secs: S.crossfade, replaygain: S.rg, replaygain_preamp_db: S.rgPre, eq: S.eq, device: S.device, viz: {}, ui: ui() });
const addEntries = (paths, at = S.entries.length) => S.entries.splice(at, 0, ...paths.map((path) => ({ id: S.seq++, path })));
const cmds = {
  set_queue: ({ paths, start, autoplay }) => { S.entries = []; addEntries(paths); S.cur = S.entries[start]?.id ?? null; S.pos = 0; S.t0 = performance.now(); S.status = autoplay && S.cur ? 'playing' : 'stopped'; emit('player-event', { type: 'queue_changed' }); emit('player-event', { type: 'track_changed', entry_id: S.cur }); },
  play_entry: ({ id }) => { S.cur = id; S.pos = 0; S.t0 = performance.now(); S.status = 'playing'; emit('player-event', { type: 'track_changed', entry_id: id }); },
  pause: () => { S.pos = position(); S.status = 'paused'; }, resume: () => { S.t0 = performance.now(); S.status = 'playing'; },
  toggle_pause: () => { if (S.status === 'playing') cmds.pause(); else if (S.status === 'paused') cmds.resume(); else if (S.entries.length) { S.cur ??= S.entries[0].id; S.pos = 0; S.t0 = performance.now(); S.status = 'playing'; emit('player-event', { type: 'track_changed', entry_id: S.cur }); } },
  stop: () => { S.status = 'stopped'; S.pos = 0; }, next: () => go(1), previous: () => go(-1),
  seek: ({ secs }) => { S.pos = secs; S.t0 = performance.now(); },
  queue_append: ({ paths }) => { addEntries(paths); S.cur ??= S.entries[0]?.id ?? null; emit('player-event', { type: 'queue_changed' }); },
  queue_play_next: ({ paths }) => { const i = S.entries.findIndex((e) => e.id === S.cur); addEntries(paths, i + 1); emit('player-event', { type: 'queue_changed' }); },
  queue_remove: ({ id }) => { const i = S.entries.findIndex((e) => e.id === id); if (i < 0) return; S.entries.splice(i, 1); if (S.cur === id) { S.cur = S.entries[i]?.id ?? S.entries[i - 1]?.id ?? null; S.pos = 0; S.t0 = performance.now(); if (!S.cur) S.status = 'stopped'; emit('player-event', { type: 'track_changed', entry_id: S.cur }); } emit('player-event', { type: 'queue_changed' }); },
  queue_move: ({ from, to }) => { const [e] = S.entries.splice(from, 1); S.entries.splice(to, 0, e); emit('player-event', { type: 'queue_changed' }); },
  queue_clear: () => { S.entries = []; S.cur = null; S.status = 'stopped'; S.pos = 0; emit('player-event', { type: 'queue_changed' }); emit('player-event', { type: 'track_changed', entry_id: null }); },
  player_state: playerState, queue_view: queueView, get_settings: settings,
  set_volume: ({ volume }) => { S.volume = volume; }, set_muted: ({ muted }) => { S.muted = muted; },
  set_shuffle: ({ on }) => { S.shuffle = on; }, set_repeat: ({ mode }) => { S.repeat = mode; },
  set_crossfade_secs: ({ secs }) => { S.crossfade = secs; }, set_replaygain: ({ mode, preampDb }) => { S.rg = mode; S.rgPre = preampDb; },
  output_devices: () => [{ id: 'spk', name: 'Speakers (Mock Audio)', is_default: true }, { id: 'hp', name: 'Headphones (Mock USB DAC)', is_default: false }],
  set_output_device: ({ id }) => { S.device = id; },
  eq_info: () => ({ settings: S.eq, band_freqs_hz: EQ_FREQS, presets: PRESETS }),
  set_eq_enabled: ({ on }) => { S.eq.enabled = on; }, set_eq_preamp_db: ({ db }) => { S.eq.preamp_db = db; },
  set_eq_band_db: ({ band, db }) => { S.eq.gains_db[band] = db; }, set_eq_gains_db: ({ gains }) => { S.eq.gains_db = gains; },
  apply_eq_preset: ({ name }) => { const p = PRESETS.find((x) => x[0] === name); if (p) S.eq.gains_db = [...p[1]]; return S.eq; },
  eq_curve: ({ points }) => eqCurve(points),
  viz_frame: vizFrame, configure_viz: () => {},
  ui_state: ui, set_ui_state: ({ value }) => { localStorage.setItem('sonara-mock-ui', JSON.stringify(value)); },
  library_folders: () => folders, add_library_folder: ({ folder }) => { if (!folders.includes(folder)) folders.push(folder); }, remove_library_folder: ({ folder }) => { folders = folders.filter((f) => f !== folder); if (!folders.length) tracks = []; },
  scan_library: scan, is_scanning: () => scanning,
  library_tracks: () => tracks.map(pub), search_library: ({ query }) => tracks.filter((t) => `${t.title} ${t.artist} ${t.album}`.toLowerCase().includes(query.toLowerCase())).map(pub),
  library_albums: albums, library_artists: artists, recently_added: ({ limit }) => tracks.slice(-limit).map(pub),
  track_meta: ({ path }) => { const t = find(path); if (!t) throw new Error('unreadable'); return pub(t); },
  lyrics: ({ path }) => lyricsFor(path), waveform: ({ path, buckets }) => waveform(path, buckets),
  playlists: plSummary, playlist: ({ id }) => PL.list.find((p) => p.id === id) ?? null,
  create_playlist: ({ name, tracks: ts }) => { const id = PL.seq++; PL.list.push({ id, name, tracks: ts ?? [] }); return id; },
  rename_playlist: ({ id, name }) => { PL.list.find((p) => p.id === id).name = name; }, delete_playlist: ({ id }) => { PL.list = PL.list.filter((p) => p.id !== id); },
  move_playlist: () => {}, add_to_playlist: ({ id, tracks: ts }) => { PL.list.find((p) => p.id === id).tracks.push(...ts); },
  remove_from_playlist: ({ id, positions }) => { const p = PL.list.find((x) => x.id === id); p.tracks = p.tracks.filter((_, i) => !positions.includes(i)); },
  move_in_playlist: ({ id, from, to }) => { const p = PL.list.find((x) => x.id === id); const [t] = p.tracks.splice(from, 1); p.tracks.splice(to, 0, t); },
  import_m3u: () => [PL.seq, 0], export_m3u: () => {}, play_playlist: ({ id, start }) => { const p = PL.list.find((x) => x.id === id); cmds.set_queue({ paths: p.tracks, start, autoplay: true }); },
};

window.__TAURI__ = {
  core: { invoke: async (cmd, args = {}) => { const f = cmds[cmd]; if (!f) throw new Error(`mock: unknown command ${cmd}`); return f(args); } },
  event: { listen: async (name, cb) => { (listeners[name] ??= []).push(cb); return () => { listeners[name] = listeners[name].filter((x) => x !== cb); }; } },
  dialog: { open: async ({ directory } = {}) => (directory ? '/music/More Music' : '/music/playlist.m3u8'), save: async ({ defaultPath } = {}) => defaultPath ?? 'out' },
};

// Start with something playing so screenshots have content.
if (!params.has('empty')) {
  const first = albums()[0];
  window.__mockStart = () => cmds.set_queue({ paths: albums().flatMap((a) => a.tracks), start: 3, autoplay: !params.has('paused') });
  cmds.set_queue({ paths: albums().flatMap((a) => a.tracks), start: 3, autoplay: !params.has('paused') });
  S.pos = 38; S.t0 = performance.now(); void first;
}
