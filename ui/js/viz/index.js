// The visualiser registry. Every visualiser is { id, name, blurb, options, draw(ctx, env, opts) }.
// `options` describes the controls the settings screen builds for it, with defaults.

import { orb, particles } from './fx.js';
import { bars, dots, ridges, ring, spectrogram, vu } from './spectrum.js';
import { blur, lines, lissajous, wave } from './wave.js';
import { cfg } from '../prefs.js';

export const VIZ = [bars, lines, wave, ring, blur, orb, particles, spectrogram, lissajous, dots, ridges, vu];

export const getViz = (id) => VIZ.find((v) => v.id === id) ?? null;

/** A visualiser's options: its defaults, then the person's saved values, then `extra`. */
export function vizOptions(id, extra = {}) {
  const v = getViz(id);
  const out = {};
  for (const o of v?.options ?? []) out[o.key] = o.def;
  return { ...out, ...(cfg().viz.opts[id] ?? {}), ...extra };
}
