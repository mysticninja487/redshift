// The UI's own settings. Saved through the backend's `ui_state` (inside settings.json), so they
// survive restarts and travel with the app's data folder.
//
// Shape: { v, skin, skins: { <skinId>: { ...overrides } }, keys: {...}, library: {...} }.
// A skin's effective config is BASE, then the skin's own defaults, then the person's overrides for
// that skin. So each skin remembers its own colours, fonts, layout and visualiser separately.

import { api } from './api.js';
import { emit } from './bus.js';
import { getSkin } from './skins.js';
import { clone, debounce, deepMerge, getPath, setPath } from './util.js';

export const BASE = {
  color: { mode: 'skin', preset: 'crimson', accent: null, bg: null, fg: null, scheme: 'skin', artMix: 0.7, glow: 0.5 },
  type: { display: null, body: null, mono: null, scale: 1, tracking: 0 },
  shape: { radius: null, density: 'cozy', border: 1, shadow: 1, glass: 0, grain: 0 },
  motion: { reduce: 'system', spin: true },
  hero: { mode: 'art', viz: 'bars', tonearm: true, plinth: false, label: 'art', glitch: false, halftone: false, sun: false },
  viz: { main: 'bars', side: 'bars', color: 'accent', fps: 60, opts: {} },
  seek: { style: 'line', times: true },
  lyrics: { size: 1, align: 'left', dim: 0.4, blur: 0, words: true },
  info: { format: true, replaygain: true, cover: false },
  transport: { volume: 'slider', mute: true },
  decor: true,
  layout: { slots: { top: [], left: [], stage: ['hero'], right: [], bottom: [] }, sizes: { left: 320, right: 340, top: 0, bottom: 0 }, flip: false },
};

export const DEFAULT_MINI = { onTop: true, open: 'last', bg: 'cover', opacity: 100, radius: 16, cover: true, artist: true, progress: true, list: 'queue' };

// Ambient mode. `viz` is 'auto' (what the skin shows) or a visualiser id; `title` is 'always', 'controls' (with the controls and for a few seconds when a track starts) or 'never'.
export const DEFAULT_AMBIENT = { mode: 'viz', viz: 'auto', bg: 'cover', fullscreen: true, hideAfter: 3, title: 'controls', lyricsSize: 1 };

export const DEFAULT_PREFS = {
  v: 1,
  skin: 'classic',
  layoutVersion: 2,
  skins: {},
  keys: {},
  library: { tab: 'albums', sort: 'default', albumSize: 190, playMode: 'single' },
  // On/off switches that apply to every skin.
  features: { lyrics: true, viz: true, queue: true },
  // Cost of looks: `lite` drops blur, blend and grain effects; `background` slows animation while the window isn't focused.
  perf: { lite: false, background: true },
  // The mini player. `open` is which size it starts at ('last' remembers the one you used); `list` what the expanded card shows beside the cover.
  mini: { ...DEFAULT_MINI },
  ambient: { ...DEFAULT_AMBIENT },
  introSeen: false,
};

let prefs = clone(DEFAULT_PREFS);
let migrated = false;
/** True once, right after an older install was moved to the Classic layout. */
export const takeMigrated = () => { const m = migrated; migrated = false; return m; };
let unsaved = false;
const save = debounce(() => { unsaved = false; return api.setUiState(prefs).catch((e) => console.warn('saving UI settings failed', e)); }, 500);

export async function loadPrefs() {
  let saved = null;
  try { saved = await api.uiState(); } catch (e) { console.warn('loading UI settings failed', e); }
  const had = saved && typeof saved === 'object' && saved.v === 1;
  prefs = deepMerge(clone(DEFAULT_PREFS), had ? saved : {});
  // People who set Redshift up before the Classic layout existed move to it once. Their own
  // per-skin choices are kept, and the other skins are one click away.
  if (had && (saved.layoutVersion ?? 0) < 2) { prefs.skin = 'classic'; prefs.layoutVersion = 2; migrated = true; }
  return prefs;
}
export const getPrefs = () => prefs;
/** True if the named feature ('lyrics' or 'viz') is switched on (they're on unless turned off). */
export const featureOn = (name) => prefs.features?.[name] !== false;

/** The settings in force for the current skin (or `skinId`). */
export function cfg(skinId = prefs.skin) {
  const skin = getSkin(skinId);
  return deepMerge(deepMerge(BASE, skin.defaults), comparing ? {} : prefs.skins[skin.id] ?? {});
}

// While Settings' "Hold to compare" is pressed, the skin shows as if you had changed nothing.
let comparing = false;
export const isComparing = () => comparing;
export function setComparing(on) {
  if (comparing === on) return;
  comparing = on;
  emit('compare', on);
  emit('ui');
}
export const get = (path) => getPath(cfg(), path);

/** Sets one value for the current skin. Passing `undefined` goes back to the skin's default. */
export function set(path, value) {
  const id = prefs.skin;
  prefs = { ...prefs, skins: { ...prefs.skins, [id]: setPath(prefs.skins[id] ?? {}, path, value) } };
  changed();
}
/** Forgets every personal choice for the current skin. */
export function resetSkin() {
  const { [prefs.skin]: _gone, ...rest } = prefs.skins;
  prefs = { ...prefs, skins: rest };
  changed();
}
export const isOverridden = (path) => getPath(prefs.skins[prefs.skin] ?? {}, path) !== undefined;

export function setSkin(id) {
  prefs = { ...prefs, skin: getSkin(id).id };
  changed();
}
export function setPref(path, value) {
  prefs = setPath(prefs, path, value);
  changed();
}
/** Replaces the current skin's overrides wholesale (used when importing a theme). */
export function importSkinOverrides(obj) {
  prefs = { ...prefs, skins: { ...prefs.skins, [prefs.skin]: obj } };
  changed();
}
export const exportSkinOverrides = () => clone(prefs.skins[prefs.skin] ?? {});

/** Follows changes saved by the other window (main or mini), so a switch in Settings reaches an open mini player. */
export function followOtherWindows(listen) {
  listen('ui-state-changed', (v) => {
    if (unsaved || !v || typeof v !== 'object' || v.v !== 1) return;
    if (JSON.stringify(v) === JSON.stringify(prefs)) return; // our own save coming back
    prefs = deepMerge(clone(DEFAULT_PREFS), v);
    emit('ui');
  });
}

function changed() {
  unsaved = true;
  save();
  emit('ui');
}
export const flushPrefs = () => save.flush();
