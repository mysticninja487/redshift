// Connects the backend to `state`: initial load, the player's events, position polling, and
// loading the details (tags, cover, lyrics, waveform) of whatever starts playing.

import { api, bumpCover, COVER, coverUrl, listen } from './api.js';
import { asleep as hostAsleep } from './host.js';
import { emit } from './bus.js';
import { state } from './state.js';
import { updateArt } from './theme.js';
import { toast } from './util.js';

const metaCache = new Map();

/** Tags for any path, cached. Resolves to null when the file can't be read. */
export function getMeta(path) {
  if (!metaCache.has(path)) metaCache.set(path, api.trackMeta(path).catch(() => null));
  return metaCache.get(path);
}
export function forgetMeta() { metaCache.clear(); }

/** Runs a backend call, showing any error as a toast instead of leaving it unhandled. */
export async function act(promise) {
  try { return await promise; } catch (e) { toast(String(e?.message ?? e), 'error'); return undefined; }
}

let loadToken = 0;

async function loadTrack(entryId) {
  const mine = ++loadToken;
  const path = state.queue.entries.find((e) => e.id === entryId)?.path ?? state.queue.current?.path ?? null;
  if (!path) {
    Object.assign(state, { track: null, trackPath: null, coverUrl: null, lyrics: null, waveform: null });
    emit('track'); emit('lyrics'); emit('waveform');
    updateArt(null);
    return;
  }
  state.trackPath = path;
  state.lyrics = null; state.waveform = null;
  state.coverUrl = coverUrl(path, COVER.big);
  state.track = (await getMeta(path)) ?? { path, title: path.split(/[\\/]/).pop(), artist: '', album: '' };
  if (mine !== loadToken) return;
  emit('track'); emit('lyrics'); emit('waveform');
  // The rest can arrive later; each is dropped if another track has started since.
  api.lyrics(path).then((l) => { if (mine === loadToken) { state.lyrics = l ?? null; emit('lyrics'); } }).catch(() => {});
  api.waveform(path, 900).then((w) => { if (mine === loadToken) { state.waveform = w; emit('waveform'); } }).catch(() => {});
  coverLoads(state.coverUrl).then((ok) => { if (mine === loadToken) { if (!ok) state.coverUrl = null; emit('track'); updateArt(ok ? state.coverUrl : null); } });
}

/** True if the cover image exists (the app answers 404 for tracks without one). */
function coverLoads(url) {
  return new Promise((resolve) => {
    const img = new Image();
    img.onload = () => resolve(true);
    img.onerror = () => resolve(false);
    img.src = url;
  });
}

// A track change and a queue change usually arrive together; one fetch of the (possibly very
// long) queue serves both, and a request that comes in mid-fetch triggers exactly one more.
let queueFetch = null, queueAgain = false;
export function refreshQueue() {
  if (queueFetch) { queueAgain = true; return queueFetch; }
  queueFetch = (async () => {
    do {
      queueAgain = false;
      const prev = state.queue.current_entry;
      state.queue = await api.queueView();
      emit('queue');
      if (state.queue.current_entry !== prev) await loadTrack(state.queue.current_entry);
    } while (queueAgain);
  })().finally(() => { queueFetch = null; });
  return queueFetch;
}

export async function refreshPlayer() {
  const p = await api.playerState();
  const wasStatus = state.player.status;
  state.player = p;
  state.stamp = performance.now();
  emit('player');
  if (wasStatus !== p.status) emit('status');
}

export async function refreshSettings() {
  state.settings = await api.getSettings();
  emit('settings');
}

export async function initController() {
  await Promise.all([refreshPlayer(), refreshQueue(), refreshSettings(), refreshFavs().catch(() => {}), refreshStats().catch(() => {})]);
  await listen('player-event', async (ev) => {
    if (ev.type === 'track_changed' || ev.type === 'queue_changed') await refreshQueue();
    if (ev.type === 'error') reportPlayError(ev.message);
    if (ev.type === 'track_changed') refreshStats().catch(() => {});
    if (ev.type === 'device_changed') await refreshPlayer();
    if (ev.type === 'ended') await refreshPlayer();
    emit('player-event', ev);
  });
  // Tags were edited (here or in the other window): drop what was remembered, refresh the lists and,
  // if it is the song playing, its details, cover and lyrics. Many songs saved at once refresh once.
  let tagTimer = 0; const edited = new Set();
  await listen('tags-changed', (path) => {
    edited.add(path); bumpCover(path); forgetMeta();
    clearTimeout(tagTimer);
    tagTimer = setTimeout(() => {
      const playing = state.trackPath && edited.has(state.trackPath);
      edited.clear();
      emit('library');
      if (playing) loadTrack(state.queue.current_entry).catch(() => {});
    }, 200);
  });
  await listen('scan-progress', (p) => { state.scan = { ...p, running: true }; emit('scan'); });
  // Position and volume come from a light poll; the interpolation in position() fills the gaps.
  // It slows down while paused or hidden, when nothing on screen is moving.
  let busy = false;
  const poll = async () => {
    if (!busy) {
      busy = true;
      try { await refreshPlayer(); } catch { /* the backend may be busy; try again next tick */ } finally { busy = false; }
    }
    setTimeout(poll, hostAsleep() ? 4000 : state.player.status === 'playing' ? 250 : 600);
  };
  setTimeout(poll, 250);
  document.addEventListener('visibilitychange', () => { if (!hostAsleep()) refreshPlayer().catch(() => {}); });
}

// A queue with many missing files would otherwise raise one error per file; gather those that
// arrive together into a single message.
let errBuf = [], errTimer = 0;
function reportPlayError(message) {
  errBuf.push(message);
  clearTimeout(errTimer);
  errTimer = setTimeout(() => {
    const [first] = errBuf, n = errBuf.length;
    errBuf = [];
    toast(n === 1 ? first : `${n} tracks couldn't be played. First: ${first}`, 'error');
  }, 400);
}

export async function refreshFavs() {
  state.favOrder = await api.favouritePaths();
  state.favs = new Set(state.favOrder);
  emit('favs');
}
export async function refreshStats() {
  const rows = await api.playStats();
  state.stats = new Map(rows.map((r) => [r.path, r]));
  emit('stats');
}
/** Adds `paths` to the favourites, or removes them if they all are favourites already. */
export async function toggleFavourite(paths) {
  const list = [...new Set(paths)];
  if (!list.length) return;
  const remove = list.every((p) => state.favs.has(p));
  await act(api.setFavourite(list, !remove));
  await refreshFavs();
  toast(remove ? 'Removed from favourites' : list.length > 1 ? `Added ${list.length} tracks to favourites` : 'Added to favourites');
}

export function setView(view) {
  if (state.view === view) return;
  state.view = view;
  emit('view', view);
}

// ---- actions the panels share ----

export const playPaths = (paths, start = 0) => act(api.setQueue(paths, start, true));
/** Plays the given tracks in random order (turning shuffle on). */
export async function shufflePlay(paths) {
  if (!paths.length) return;
  await act(api.setShuffle(true));
  return playPaths(paths, Math.floor(Math.random() * paths.length));
}
export const queueNext = (paths) => act(api.queuePlayNext(paths)).then(() => toast('Added to play next'));
export const queueAdd = (paths) => act(api.queueAppend(paths)).then(() => toast(paths.length > 1 ? `Added ${paths.length} tracks to the queue` : 'Added to the queue'));

export async function rescanLibrary() {
  if (state.scan?.running) return;
  state.scan = { running: true, done: 0, total: 0, phase: 'discovering' };
  emit('scan');
  try {
    const report = await api.scanLibrary();
    state.scan = null;
    forgetMeta();
    emit('library');
    emit('scan');
    const bits = [];
    if (report.added) bits.push(`${report.added} new`);
    if (report.updated) bits.push(`${report.updated} updated`);
    if (report.removed) bits.push(`${report.removed} removed`);
    toast(bits.length ? `Library scanned: ${bits.join(', ')}` : 'Library is up to date');
    if (report.failed?.length) toast(`${report.failed.length} files couldn't be read`, 'error');
  } catch (e) {
    state.scan = null; emit('scan');
    toast(String(e?.message ?? e), 'error');
  }
}
