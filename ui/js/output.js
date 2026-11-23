// What the sound path is doing right now, in words: where it ends up, and whether the file's own
// samples get there untouched. One place so Settings, Now Playing and Song info say the same thing.

import { fmtKhz } from './util.js';

const STAGES = { eq: 'EQ', speed: 'Speed and pitch', volume: 'Volume', gain: 'ReplayGain', limiter: 'Limiter', crossfade: 'Crossfade' };

/** "Speakers · 96 kHz · 24-bit · Exclusive" */
export function outputLine(p) {
  const o = p?.output;
  if (!o || !o.sample_rate) return '';
  const depth = o.float ? '32-bit float' : `${o.bits}-bit`;
  return [o.device_name, fmtKhz(o.sample_rate), depth, o.exclusive ? 'Exclusive' : 'Shared'].filter(Boolean).join(' · ');
}

/**
 * Whether the song that is playing reaches the device unchanged.
 * `kind` is 'perfect' or 'altered'; `reasons` says what changes it.
 */
export function pathVerdict(p, cur) {
  if (!p?.output || !cur) return null;
  if (p.bit_perfect) return { kind: 'perfect', reasons: [] };
  const o = p.output;
  const reasons = [];
  if (!o.exclusive) reasons.push(p.exclusive ? `Exclusive mode didn’t start${o.fallback ? ` (${o.fallback})` : ''}` : 'Shared mode: Windows mixes the sound');
  if (cur.source_rate && o.sample_rate && cur.source_rate !== o.sample_rate) reasons.push(`Resampled from ${fmtKhz(cur.source_rate)} to ${fmtKhz(o.sample_rate)}`);
  if (cur.bits_per_sample && !o.float && cur.bits_per_sample > o.bits) reasons.push(`${cur.bits_per_sample}-bit reduced to ${o.bits}-bit`);
  const stages = (p.processing ?? []).map((s) => STAGES[s] ?? s);
  if (stages.length) reasons.push(`Processing: ${stages.join(', ')}`);
  if (!cur.bits_per_sample) reasons.push('Compressed file, decoded to samples');
  return { kind: 'altered', reasons };
}

/** The short chip for Now Playing, or null when there is nothing worth saying. */
export function pathChip(p, cur) {
  const v = pathVerdict(p, cur);
  if (!v) return null;
  if (v.kind === 'perfect') return { text: 'Bit-perfect', tone: 'perfect' };
  if (p.output?.exclusive && cur?.source_rate === p.output.sample_rate) return { text: 'Exclusive', tone: 'exclusive' };
  if (cur?.source_rate && p.output?.sample_rate && cur.source_rate !== p.output.sample_rate) return { text: `→ ${fmtKhz(p.output.sample_rate)}`, tone: 'resampled' };
  return null;
}
