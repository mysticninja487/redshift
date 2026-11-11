// Which settings differ from their defaults, so Settings can mark them and offer a Reset.

import { BASE, DEFAULT_PREFS } from '../prefs.js';
import { getSkin } from '../skins.js';
import { clone, deepMerge, getPath, isObj } from '../util.js';
import { getViz } from '../viz/index.js';

/** Every leaf of `obj` as [path, value]. `layout.slots` and arrays count as one value. */
function leaves(obj, prefix = '', out = []) {
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

/** Keys (`skin:<path>` for the current skin, `shared:<path>` for the rest) of settings that differ from their defaults. */
export function changedKeys(prefs) {
  const out = [];
  const base = deepMerge(clone(BASE), getSkin(prefs.skin).defaults ?? {});
  for (const [path, to] of leaves(prefs.skins?.[prefs.skin])) if (to !== undefined && !same(defaultOf(path, base), to)) out.push(`skin:${path}`);
  for (const root of SHARED) for (const [path, to] of leaves(prefs[root], root)) if (to !== undefined && !same(getPath(DEFAULT_PREFS, path), to)) out.push(`shared:${path}`);
  return out;
}
