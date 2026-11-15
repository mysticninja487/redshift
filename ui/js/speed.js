// The speed and pitch button in the top bar. It opens a small panel with the usual choices; while
// something other than normal is set, the button says so.

import { on } from './bus.js';
import { MAX_PITCH, MAX_SPEED, MIN_SPEED, SPEEDS, formatPitch, formatSpeed, getRate, isNormal, resetRate, setRate } from './rate.js';
import { button, iconButton, slider, toggle } from './ui/controls.js';
import { h } from './util.js';

let open = null;

function closePanel() {
  if (!open) return;
  open.el.remove();
  document.removeEventListener('pointerdown', open.away, true);
  document.removeEventListener('keydown', open.key, true);
  open.off?.();
  open = null;
}

export function speedButton() {
  const badge = h('span', { class: 'sleep-left', hidden: true });
  const btn = iconButton('gauge', 'Speed and pitch', (e) => (open ? closePanel() : openPanel(e.currentTarget)), { cls: 'sleep-btn speed-btn', size: 20 });
  btn.append(badge);

  function paint() {
    const r = getRate();
    const normal = isNormal(r);
    btn.setAttribute('aria-pressed', String(!normal));
    badge.hidden = normal;
    badge.textContent = Math.abs(r.speed - 1) > 0.001 ? formatSpeed(r.speed) : formatPitch(r.pitch_semitones);
    btn.title = btn.ariaLabel = normal ? 'Speed and pitch' : `Speed and pitch: ${formatSpeed(r.speed)} · ${formatPitch(r.pitch_semitones)}`;
  }
  on('rate', paint);
  on('ui', paint);
  paint();
  return btn;
}

function openPanel(anchor) {
  closePanel();
  const r = getRate();
  const chips = h('div', { class: 'rate-chips', role: 'group', 'aria-label': 'Speed presets' }, SPEEDS.map((v) =>
    h('button', { class: 'rate-chip', type: 'button', dataset: { v: String(v) }, onclick: () => setRate({ speed: v }) }, formatSpeed(v))));
  const speedOut = h('output', { class: 'set-val' }, formatSpeed(r.speed));
  const pitchOut = h('output', { class: 'set-val' }, formatPitch(r.pitch_semitones));
  const speed = slider({ min: MIN_SPEED, max: MAX_SPEED, step: 0.05, value: r.speed, reset: 1, label: 'Speed', onInput: (v) => setRate({ speed: v }) });
  const pitch = slider({ min: -MAX_PITCH, max: MAX_PITCH, step: 0.5, value: r.pitch_semitones, center: true, reset: 0, label: 'Pitch in semitones', onInput: (v) => setRate({ pitch_semitones: v }) });
  const keep = toggle({ value: r.keep_pitch, label: 'Keep pitch when changing speed', onChange: (v) => setRate({ keep_pitch: v }) });
  const reset = button('Reset', () => resetRate(), { cls: 'rate-reset' });

  const el = h('div', { class: 'rate-pop', role: 'dialog', 'aria-label': 'Speed and pitch' },
    h('div', { class: 'rate-row' }, h('span', { class: 'rate-label' }, 'Speed'), speedOut),
    chips, speed,
    h('div', { class: 'rate-row' }, h('span', { class: 'rate-label' }, 'Pitch'), pitchOut), pitch,
    h('div', { class: 'rate-row rate-keep' }, h('span', { class: 'rate-label' }, 'Keep pitch when changing speed'), keep),
    h('div', { class: 'rate-row rate-foot' }, h('small', null, 'Tape style (off) changes pitch and speed together, like a record.'), reset));

  const sync = (cur) => {
    speedOut.textContent = formatSpeed(cur.speed);
    pitchOut.textContent = formatPitch(cur.pitch_semitones);
    speed.set(cur.speed); pitch.set(cur.pitch_semitones);
    keep.set?.(cur.keep_pitch);
    el.querySelectorAll('.rate-chip').forEach((c) => c.setAttribute('aria-pressed', String(Math.abs(+c.dataset.v - cur.speed) < 0.001)));
    reset.disabled = isNormal(cur);
  };
  document.body.append(el);
  const a = anchor.getBoundingClientRect(), w = el.getBoundingClientRect();
  el.style.left = `${Math.max(8, Math.min(a.left, innerWidth - w.width - 8))}px`;
  el.style.top = `${Math.max(8, Math.min(a.bottom + 8, innerHeight - w.height - 8))}px`;
  const away = (e) => { if (!el.contains(e.target) && !anchor.contains(e.target)) closePanel(); };
  const key = (e) => { if (e.key === 'Escape') { closePanel(); anchor.focus(); } };
  setTimeout(() => { document.addEventListener('pointerdown', away, true); document.addEventListener('keydown', key, true); });
  open = { el, away, key, off: on('rate', sync) };
  sync(r);
}
