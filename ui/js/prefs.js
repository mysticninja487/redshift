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

export const DEFAULT_PREFS = {
  v: 1,
  skin: 'turntable',
  skins: {},
  keys: {},
  library: { tab: 'albums', sort: 'default', albumSize: 190, playMode: 'single' },
  // On/off switches that apply to every skin.
  features: { lyrics: true, viz: true },
  introSeen: false,
};

let prefs = clone(DEFAULT_PREFS);
const save = debounce(() => api.setUiState(prefs).catch((e) => console.warn('saving UI settings failed', e)), 500);

export async function loadPrefs() {
  let saved = null;
  try { saved = await api.uiState(); } catch (e) { console.warn('loading UI settings failed', e); }
  prefs = deepMerge(clone(DEFAULT_PREFS), saved && typeof saved === 'object' && saved.v === 1 ? saved : {});
  return prefs;
}
export const getPrefs = () => prefs;
/** True if the named feature ('lyrics' or 'viz') is switched on (they're on unless turned off). */
export const featureOn = (name) => prefs.features?.[name] !== false;

/** The settings in force for the current skin (or `skinId`). */
export function cfg(skinId = prefs.skin) {
  const skin = getSkin(skinId);
  return deepMerge(deepMerge(BASE, skin.defaults), prefs.skins[skin.id] ?? {});
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

function changed() {
  save();
  emit('ui');
}
export const flushPrefs = () => save.flush();
