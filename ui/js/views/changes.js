// Works out which settings differ from their defaults, and says so in words. Settings uses this for
// the "Changes" tab, the marks on changed rows, and the undo chip after each change.

import { ACTIONS } from '../keys.js';
import { BASE, DEFAULT_PREFS } from '../prefs.js';
import { FONTS, getSkin } from '../skins.js';
import { clone, deepMerge, getPath, isObj } from '../util.js';
import { getViz } from '../viz/index.js';

// Which tab a setting lives on, so a change can link to it.
const TAB_OF = [['color', 'appearance'], ['type', 'appearance'], ['shape', 'appearance'], ['motion', 'appearance'], ['decor', 'appearance'],
  ['hero', 'visual'], ['viz', 'visual'], ['perf', 'visual'], ['layout', 'layout'], ['lyrics', 'lyrics'], ['info', 'lyrics'], ['seek', 'lyrics'], ['transport', 'lyrics'],
  ['features.lyrics', 'lyrics'], ['features.viz', 'visual'], ['features.queue', 'layout'], ['mini', 'mini'], ['ambient', 'ambient'], ['keys', 'keys']];
export const tabOf = (path) => TAB_OF.find(([p]) => path === p || path.startsWith(`${p}.`))?.[1] ?? 'appearance';

// path -> [name, section]
const NAMES = {
  'color.scheme': ['Light or dark', 'Colour'], 'color.mode': ['Accent from', 'Colour'], 'color.preset': ['Accent preset', 'Colour'], 'color.accent': ['Accent colour', 'Colour'],
  'color.bg': ['Background colour', 'Colour'], 'color.fg': ['Text colour', 'Colour'], 'color.artMix': ['Cover strength', 'Colour'], 'color.glow': ['Glow', 'Colour'],
  'type.display': ['Display font', 'Type'], 'type.body': ['Body font', 'Type'], 'type.mono': ['Mono font', 'Type'], 'type.scale': ['Text size', 'Type'], 'type.tracking': ['Letter spacing', 'Type'],
  'shape.radius': ['Corner radius', 'Shape & surface'], 'shape.density': ['Spacing', 'Shape & surface'], 'shape.border': ['Line weight', 'Shape & surface'], 'shape.shadow': ['Shadows', 'Shape & surface'],
  'shape.glass': ['Frosted glass', 'Shape & surface'], 'shape.grain': ['Film grain', 'Shape & surface'], 'decor': ['Decoration', 'Shape & surface'], 'motion.reduce': ['Reduce motion', 'Shape & surface'],
  'motion.spin': ['Spinning record', 'Centrepiece'], 'hero.mode': ['Centrepiece shows', 'Centrepiece'], 'hero.viz': ['Centrepiece visualiser', 'Centrepiece'], 'hero.tonearm': ['Tonearm', 'Centrepiece'],
  'hero.plinth': ['Plinth', 'Centrepiece'], 'hero.label': ['Record label', 'Centrepiece'], 'hero.glitch': ['Glitch on the beat', 'Centrepiece'], 'hero.halftone': ['Halftone cover', 'Centrepiece'], 'hero.sun': ['Red sun', 'Centrepiece'],
  'viz.main': ['Main visualiser', 'Visualisers'], 'viz.side': ['Visualiser panel', 'Visualisers'], 'viz.color': ['Visualiser colour', 'Visualisers'], 'viz.fps': ['Visualiser frame rate', 'Visualisers'],
  'lyrics.size': ['Lyrics text size', 'Lyrics & info'], 'lyrics.align': ['Lyrics alignment', 'Lyrics & info'], 'lyrics.dim': ['Fade other lines', 'Lyrics & info'], 'lyrics.blur': ['Blur distant lines', 'Lyrics & info'], 'lyrics.words': ['Word-by-word highlight', 'Lyrics & info'],
  'info.format': ['Technical details', 'Now playing'], 'info.replaygain': ['ReplayGain note', 'Now playing'], 'info.cover': ['Small cover', 'Now playing'],
  'seek.style': ['Progress bar style', 'Now playing'], 'seek.times': ['Show times', 'Now playing'], 'transport.volume': ['Volume control', 'Now playing'], 'transport.mute': ['Mute button', 'Now playing'],
  'layout.slots': ['Where the panels sit', 'Layout'], 'layout.flip': ['Mirror left and right', 'Layout'],
  'layout.sizes.left': ['Left column width', 'Layout'], 'layout.sizes.right': ['Right column width', 'Layout'], 'layout.sizes.top': ['Top strip height', 'Layout'], 'layout.sizes.bottom': ['Bottom strip height', 'Layout'],
  'features.lyrics': ['Show lyrics', 'Panels'], 'features.viz': ['Show visualisers', 'Panels'], 'features.queue': ['Show queue', 'Panels'],
  'perf.lite': ['Reduce effects', 'Performance'], 'perf.background': ['Slow down in the background', 'Performance'],
  'mini.onTop': ['Keep on top', 'Mini player'], 'mini.open': ['Opens as', 'Mini player'], 'mini.list': ['Expanded list', 'Mini player'], 'mini.bg': ['Background', 'Mini player'], 'mini.opacity': ['Opacity', 'Mini player'],
  'mini.radius': ['Corner radius', 'Mini player'], 'mini.cover': ['Cover in the compact player', 'Mini player'], 'mini.artist': ['Artist and album', 'Mini player'], 'mini.progress': ['Progress line in the compact player', 'Mini player'],
  'ambient.mode': ['Opens as', 'Ambient mode'], 'ambient.viz': ['Visualiser', 'Ambient mode'], 'ambient.bg': ['Background', 'Ambient mode'], 'ambient.fullscreen': ['Fullscreen', 'Ambient mode'],
  'ambient.hideAfter': ['Hide controls after', 'Ambient mode'], 'ambient.title': ['Track title', 'Ambient mode'], 'ambient.lyricsSize': ['Lyrics size', 'Ambient mode'],
};
export const SECTION_ORDER = ['Colour', 'Type', 'Shape & surface', 'Centrepiece', 'Visualisers', 'Lyrics & info', 'Now playing', 'Layout', 'Panels', 'Performance', 'Mini player', 'Ambient mode', 'Shortcuts', 'Other'];

const cap = (s) => s.replace(/([a-z])([A-Z])/g, '$1 $2').replace(/[-_.]/g, ' ').replace(/^./, (c) => c.toUpperCase());

/** [name, section] for a setting path (`viz.opts.<viz>.<option>` and `keys.<action>` are looked up). */
export function nameOf(path) {
  if (NAMES[path]) return NAMES[path];
  let m = /^viz\.opts\.([^.]+)\.(.+)$/.exec(path);
  if (m) { const v = getViz(m[1]); const o = v?.options?.find((x) => x.key === m[2]); return [`${v?.name ?? cap(m[1])}: ${o?.label ?? cap(m[2])}`, 'Visualisers']; }
  m = /^keys\.(.+)$/.exec(path);
  if (m) return [ACTIONS.find((a) => a.id === m[1])?.label ?? cap(m[1]), 'Shortcuts'];
  return [cap(path.split('.').slice(-1)[0]), 'Other'];
}

const PCT = (v) => `${Math.round(v * 100)}%`;
const FORMAT = {
  'color.glow': PCT, 'color.artMix': PCT, 'shape.glass': PCT, 'shape.grain': PCT, 'shape.shadow': PCT, 'type.scale': PCT, 'lyrics.size': PCT, 'lyrics.dim': PCT, 'ambient.lyricsSize': PCT,
  'shape.radius': (v) => `${v}px`, 'shape.border': (v) => `${v}px`, 'mini.radius': (v) => `${v}px`, 'mini.opacity': (v) => `${v}%`, 'lyrics.blur': (v) => `${v}px`,
  'type.tracking': (v) => `${+Number(v).toFixed(3)} em`, 'viz.fps': (v) => `${v} fps`, 'ambient.hideAfter': (v) => (v ? `${v} s` : 'Never'),
  'layout.sizes.left': (v) => `${v}px`, 'layout.sizes.right': (v) => `${v}px`, 'layout.sizes.top': (v) => `${v}px`, 'layout.sizes.bottom': (v) => `${v}px`,
};
const WORDS = { skin: 'Skin default', art: 'Album art', fg: 'Text colour', last: 'Last used', theme: 'Skin colour', cover: 'Blurred cover', accent: 'Accent tint', auto: 'Same as the skin', controls: 'With controls', viz: 'Visualiser', queue: 'Up next' };

/** A value as a short phrase: "On", "120%", "Wavelength". `undefined`/`null` means the skin decides. */
export function describe(path, v) {
  if (v === undefined || v === null) return 'Skin default';
  if (path === 'layout.slots') return 'Custom arrangement';
  if (typeof v === 'boolean') return v ? 'On' : 'Off';
  if (typeof v === 'number') return (FORMAT[path] ?? ((x) => String(+x.toFixed(3))))(v);
  if (typeof v === 'string') {
    if (/^type\.(display|body|mono)$/.test(path)) return FONTS.find((f) => f.id === v)?.label ?? v;
    if (path === 'ambient.viz' || /^(hero|viz)\.(viz|main|side)$/.test(path)) return getViz(v)?.name ?? (WORDS[v] ?? cap(v));
    if (/^keys\./.test(path)) return v;
    if (/^color\.(accent|bg|fg)$/.test(path)) return v;
    return WORDS[v] ?? cap(v);
  }
  return 'Customised';
}

/** Every leaf of `obj` as [path, value]. `layout.slots` and arrays count as one value. */
export function leaves(obj, prefix = '', out = []) {
  for (const [k, v] of Object.entries(obj ?? {})) {
    const p = prefix ? `${prefix}.${k}` : k;
    if (isObj(v) && p !== 'layout.slots') leaves(v, p, out); else out.push([p, v]);
  }
  return out;
}

const SHARED = ['features', 'perf', 'mini', 'ambient'];
const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);

/** What a skin setting goes back to: the skin's default, or a visualiser option's own default. */
function defaultOf(path, base) {
  const m = /^viz\.opts\.([^.]+)\.(.+)$/.exec(path);
  if (m) return getViz(m[1])?.options?.find((o) => o.key === m[2])?.def;
  return getPath(base, path);
}

/** The default of a setting as `diff` reports it, for showing what a change moved away from. */
export function defaultValue(scope, path, prefs) {
  if (scope === 'skin') return defaultOf(path, skinDefaults(prefs.skin));
  if (path === 'skin') return undefined;
  return getPath(DEFAULT_PREFS, path);
}

/** The skin's own defaults (what "Reset" goes back to). */
export const skinDefaults = (skinId) => deepMerge(clone(BASE), getSkin(skinId).defaults ?? {});

/**
 * Settings that differ from their defaults: this skin's, then the ones every skin shares. Each item is
 * { scope: 'skin' | 'shared', path, name, section, from, to, key }.
 */
export function changesIn(prefs, skinId = prefs.skin) {
  const out = [];
  const base = skinDefaults(skinId);
  for (const [path, to] of leaves(prefs.skins?.[skinId])) {
    const from = defaultOf(path, base);
    if (to === undefined || same(from, to)) continue;
    const [name, section] = nameOf(path);
    out.push({ scope: 'skin', path, name, section, from, to, key: `skin:${path}` });
  }
  for (const root of SHARED) {
    for (const [path, to] of leaves(prefs[root], root)) {
      const from = getPath(DEFAULT_PREFS, path);
      if (to === undefined || same(from, to)) continue;
      const [name, section] = nameOf(path);
      out.push({ scope: 'shared', path, name, section, from, to, key: `shared:${path}` });
    }
  }
  for (const [id, combo] of Object.entries(prefs.keys ?? {})) {
    const from = ACTIONS.find((a) => a.id === id)?.key ?? '';
    if (combo === from) continue;
    out.push({ scope: 'shared', path: `keys.${id}`, name: nameOf(`keys.${id}`)[0], section: 'Shortcuts', from, to: combo, key: `shared:keys.${id}` });
  }
  return out.sort((a, b) => SECTION_ORDER.indexOf(a.section) - SECTION_ORDER.indexOf(b.section));
}

/** Changes made to other skins, as { skin, count }. */
export function otherSkins(prefs, skinId = prefs.skin) {
  return Object.keys(prefs.skins ?? {}).filter((id) => id !== skinId).map((id) => ({ id, name: getSkin(id).name, count: changesIn(prefs, id).filter((c) => c.scope === 'skin').length })).filter((s) => s.count);
}

/**
 * What differs between two snapshots of the saved settings, as [{ scope, path, from, to }], looking at
 * the current skin's overrides and the shared settings. Used to say what just changed.
 */
export function diff(before, after) {
  const out = [];
  const id = after.skin;
  const grab = (p) => { const m = new Map(); leaves(p?.skins?.[id]).forEach(([k, v]) => m.set(`skin:${k}`, v)); for (const r of SHARED) leaves(p?.[r], r).forEach(([k, v]) => m.set(`shared:${k}`, v)); m.set('shared:skin', p?.skin); return m; };
  const a = grab(before), b = grab(after);
  for (const key of new Set([...a.keys(), ...b.keys()])) {
    const x = a.get(key), y = b.get(key);
    if (same(x, y)) continue;
    const [scope, path] = [key.slice(0, key.indexOf(':')), key.slice(key.indexOf(':') + 1)];
    out.push({ scope, path, from: x, to: y });
  }
  return out;
}
