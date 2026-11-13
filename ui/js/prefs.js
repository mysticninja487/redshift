// The UI's own settings. Saved through the backend's `ui_state` (inside settings.json), so they
// survive restarts and travel with the app's data folder.
//
// Shape: { v, skin, skins: { <skinId>: { ...overrides } }, keys: {...}, library: {...} }.
// A skin's effective config is BASE, then the skin's own defaults, then the person's overrides for
// that skin. So each skin remembers its own colours, fonts, layout and visualiser separately.

import { api } from './api.js';
import { emit } from './bus.js';
import { builtinSkins, getSkin, isBuiltinSkin, setCustomSkins } from './skins.js';
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
  // The person's own skins: [{ id, name, base, settings }]. Each shows up as a skin called `custom:<id>`.
  looks: [],
  keys: {},
  library: { tab: 'albums', sort: 'default', albumSize: 190, playMode: 'single' },
  // On/off switches that apply to every skin.
  features: { lyrics: true, viz: true, queue: true, timeLeft: true },
  // Whether the right-hand time shows time left (a click on it flips this; the switch above allows the click).
  timeLeft: false,
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
  syncLooks();
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
    syncLooks();
    emit('ui');
  });
}

function changed() {
  unsaved = true;
  save();
  emit('ui');
}
export const flushPrefs = () => save.flush();

// ---------- Your own skins ----------
// A look is a skin of your own: one of the built-in skins plus the settings you saved on it. It
// shows in the skin lists like any other, and still keeps its own changes on top.

const LOOK_NAME_MAX = 40;
const LOOK_MAX = 40;
const LOOK_BYTES_MAX = 64 * 1024;
const LOOK_KEYS = Object.keys(BASE);
const UNSAFE_KEYS = new Set(['__proto__', 'constructor', 'prototype']);

/** `v` as plain data, without anything that could change how objects behave. */
function plain(v, depth = 0) {
  if (depth > 12) return undefined;
  if (Array.isArray(v)) return v.slice(0, 64).map((x) => plain(x, depth + 1));
  if (v && typeof v === 'object') {
    const out = {};
    for (const [k, x] of Object.entries(v)) if (!UNSAFE_KEYS.has(k)) { const y = plain(x, depth + 1); if (y !== undefined) out[k] = y; }
    return out;
  }
  return ['string', 'number', 'boolean'].includes(typeof v) || v === null ? v : undefined;
}
/** Checks settings from a file or the clipboard: only the sections Redshift knows, and not too large. Returns null if unusable. */
export function cleanLookSettings(settings) {
  if (!settings || typeof settings !== 'object' || Array.isArray(settings)) return null;
  const out = {};
  for (const k of LOOK_KEYS) if (k in settings) { const y = plain(settings[k]); if (y !== undefined) out[k] = y; }
  return JSON.stringify(out).length > LOOK_BYTES_MAX ? null : out;
}
const cleanName = (n, fallback) => String(n ?? '').replace(/[\u0000-\u001f]/g, ' ').trim().slice(0, LOOK_NAME_MAX) || fallback;
const newLookId = () => Math.random().toString(36).slice(2, 10);

function cleanLooks(list) {
  const seen = new Set();
  return (Array.isArray(list) ? list : []).slice(0, LOOK_MAX).flatMap((l) => {
    const id = typeof l?.id === 'string' && /^[a-z0-9]{3,16}$/.test(l.id) ? l.id : null;
    const settings = cleanLookSettings(l?.settings);
    if (!id || seen.has(id) || !settings) return [];
    seen.add(id);
    return [{ id, name: cleanName(l.name, 'My skin'), base: isBuiltinSkin(l.base) ? l.base : builtinSkins[0].id, settings }];
  });
}
/** Makes the skin list match the saved looks, and falls back to a built-in skin if the current one is gone. */
function syncLooks() {
  prefs.looks = cleanLooks(prefs.looks);
  setCustomSkins(prefs.looks);
  if (getSkin(prefs.skin).id !== prefs.skin) prefs.skin = getSkin(prefs.skin).id;
}
export const getLooks = () => prefs.looks ?? [];
export const isCustomSkin = (id = prefs.skin) => typeof id === 'string' && id.startsWith('custom:');
const lookKey = (id) => id.replace(/^custom:/, '');

/** Saves the current skin, with your changes, as a new skin and switches to it. Returns it, or null when there are too many. */
export function saveLook(name) {
  if ((prefs.looks?.length ?? 0) >= LOOK_MAX) return null;
  const here = getSkin(prefs.skin);
  const settings = cleanLookSettings(exportSkinOverrides()) ?? {};
  // Saving from a custom skin builds on that skin's own base, with its saved settings underneath.
  const under = isCustomSkin() ? deepMerge(clone(getLooks().find((l) => l.id === lookKey(here.id))?.settings ?? {}), settings) : settings;
  const look = { id: newLookId(), name: cleanName(name, `${here.name} copy`), base: here.baseId ?? here.id, settings: under };
  prefs = { ...prefs, looks: [...(prefs.looks ?? []), look] };
  syncLooks();
  prefs = { ...prefs, skin: `custom:${look.id}` };
  changed();
  return look;
}
export function renameLook(id, name) {
  prefs = { ...prefs, looks: getLooks().map((l) => (l.id === lookKey(id) ? { ...l, name: cleanName(name, l.name) } : l)) };
  syncLooks();
  changed();
}
/** Deletes a look. If it was the current skin, goes back to the skin it was built on. */
export function deleteLook(id) {
  const key = lookKey(id), gone = getLooks().find((l) => l.id === key);
  if (!gone) return;
  const { [`custom:${key}`]: _o, ...rest } = prefs.skins;
  prefs = { ...prefs, looks: getLooks().filter((l) => l.id !== key), skins: rest, skin: prefs.skin === `custom:${key}` ? gone.base : prefs.skin };
  syncLooks();
  changed();
}
/** What a look looks like as a file. */
export function lookFile(id = prefs.skin) {
  const skin = getSkin(id);
  const look = getLooks().find((l) => `custom:${l.id}` === skin.id);
  const mine = cleanLookSettings(exportSkinOverrides()) ?? {};
  return { redshiftSkin: 1, name: skin.name, base: skin.baseId ?? skin.id, settings: look ? deepMerge(clone(look.settings), mine) : mine };
}
/** Adds a skin from a file's contents (a parsed look). Returns the new look, or null if it isn't one. */
export function importLook(data) {
  const settings = cleanLookSettings(data?.settings);
  if (!settings || (prefs.looks?.length ?? 0) >= LOOK_MAX) return null;
  const base = isBuiltinSkin(data.base) ? data.base : isBuiltinSkin(data.skin) ? data.skin : builtinSkins[0].id;
  const look = { id: newLookId(), name: cleanName(data.name, 'Imported skin'), base, settings };
  prefs = { ...prefs, looks: [...(prefs.looks ?? []), look] };
  syncLooks();
  prefs = { ...prefs, skin: `custom:${look.id}` };
  changed();
  return look;
}
