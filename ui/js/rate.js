// Play speed and pitch: the one place that knows the current setting and changes it. The top-bar
// popover, the Settings page, the keyboard shortcuts and the mini player all go through here.

import { api } from './api.js';
import { emit } from './bus.js';
import { state } from './state.js';

export const SPEEDS = [0.5, 0.75, 1, 1.25, 1.5, 2];
export const MIN_SPEED = 0.5, MAX_SPEED = 2, MAX_PITCH = 12;

export const getRate = () => state.player.rate ?? { speed: 1, pitch_semitones: 0, keep_pitch: true };
export const isNormal = (r = getRate()) => Math.abs(r.speed - 1) < 0.001 && Math.abs(r.pitch_semitones) < 0.001;

const clampTo = (v, lo, hi) => Math.min(hi, Math.max(lo, Number.isFinite(v) ? v : 0));
const round = (v, step) => Math.round(v / step) * step;

/** "1.25×" */
export const formatSpeed = (v) => `${+Number(v).toFixed(2)}×`;
/** "+3 st", "−2 st", "0 st" (semitones) */
export const formatPitch = (v) => { const n = +Number(v).toFixed(1); return `${n > 0 ? '+' : n < 0 ? '−' : ''}${Math.abs(n)} st`; };

/** Changes any of speed, pitch_semitones and keep_pitch. Takes effect on what is playing, at once. */
export async function setRate(patch) {
  const cur = getRate();
  const next = { ...cur, ...patch };
  next.speed = clampTo(round(next.speed, 0.01), MIN_SPEED, MAX_SPEED);
  next.pitch_semitones = clampTo(round(next.pitch_semitones, 0.1), -MAX_PITCH, MAX_PITCH);
  next.keep_pitch = !!next.keep_pitch;
  state.player.rate = next;
  emit('rate', next);
  try { await api.setRate(next.speed, next.pitch_semitones, next.keep_pitch); } catch { /* the engine keeps the last good value */ }
  return next;
}

/** A step up or down in speed, snapping to the 0.05 grid. */
export const nudgeSpeed = (dir) => setRate({ speed: round(getRate().speed + dir * 0.05, 0.05) });
export const resetRate = () => setRate({ speed: 1, pitch_semitones: 0 });
