// Reusable controls. Plain DOM, styled by css/components.css and skin CSS through variables.

import { clamp, drag, h } from '../util.js';
import { icon } from './icons.js';

/** Icon button. `label` is the accessible name and tooltip. */
export function iconButton(name, label, onclick, { size = 20, cls = '', pressed } = {}) {
  const b = h('button', { class: `btn icon-btn ${cls}`.trim(), type: 'button', title: label, 'aria-label': label, onclick }, icon(name, size));
  if (pressed !== undefined) b.setAttribute('aria-pressed', String(pressed));
  return b;
}

export function button(text, onclick, { cls = '', iconName } = {}) {
  return h('button', { class: `btn ${cls}`.trim(), type: 'button', onclick }, iconName && icon(iconName, 16), text);
}

/**
 * A slider that works horizontally or vertically and answers to the keyboard. Returns the element
 * with `.set(v)` to move it from outside. `onInput(v)` fires while dragging, `onChange(v)` at the end.
 * Double-click resets to `reset` when given.
 */
export function slider({ min = 0, max = 1, step = 0.01, value = 0, vertical = false, label = '', onInput, onChange, reset, format, center = false }) {
  const fill = h('div', { class: 'sl-fill' });
  const thumb = h('div', { class: 'sl-thumb' });
  const el = h('div', { class: `slider ${vertical ? 'sl-v' : 'sl-h'}${center ? ' sl-center' : ''}`, role: 'slider', tabindex: '0', 'aria-label': label, 'aria-orientation': vertical ? 'vertical' : 'horizontal', 'aria-valuemin': String(min), 'aria-valuemax': String(max) },
    h('div', { class: 'sl-track' }, fill), thumb);
  let v = value;
  const snap = (x) => clamp(Math.round(x / step) * step, min, max);
  const paint = () => {
    const f = (v - min) / (max - min);
    if (center) {
      const c = (0 - min) / (max - min), a = Math.min(f, c), b = Math.max(f, c);
      if (vertical) { fill.style.bottom = `${a * 100}%`; fill.style.height = `${(b - a) * 100}%`; } else { fill.style.left = `${a * 100}%`; fill.style.width = `${(b - a) * 100}%`; }
    } else if (vertical) { fill.style.height = `${f * 100}%`; } else { fill.style.width = `${f * 100}%`; }
    el.style.setProperty('--f', String(f));
    el.setAttribute('aria-valuenow', String(+v.toFixed(3)));
    el.setAttribute('aria-valuetext', format ? format(v) : String(+v.toFixed(2)));
  };
  const fromEvent = (ev) => {
    const r = el.getBoundingClientRect();
    const f = vertical ? 1 - (ev.clientY - r.top) / r.height : (ev.clientX - r.left) / r.width;
    return snap(min + clamp(f, 0, 1) * (max - min));
  };
  const apply = (nv, fire = 'onInput') => { if (nv === v) return; v = nv; paint(); (fire === 'onInput' ? onInput : onChange)?.(v); };
  drag(el, {
    start: (ev) => { el.classList.add('dragging'); apply(fromEvent(ev)); },
    move: (_dx, _dy, ev) => apply(fromEvent(ev)),
    end: () => { el.classList.remove('dragging'); onChange?.(v); },
  });
  el.addEventListener('keydown', (ev) => {
    const big = (max - min) / 10;
    const d = { ArrowRight: step, ArrowUp: step, ArrowLeft: -step, ArrowDown: -step, PageUp: big, PageDown: -big }[ev.key];
    if (d !== undefined) { ev.preventDefault(); apply(snap(v + d)); onChange?.(v); }
    else if (ev.key === 'Home') { apply(min); onChange?.(v); } else if (ev.key === 'End') { apply(max); onChange?.(v); }
  });
  if (reset !== undefined) el.addEventListener('dblclick', () => { apply(snap(reset)); onChange?.(v); });
  el.set = (nv) => { if (!el.classList.contains('dragging') && nv !== v) { v = clamp(nv, min, max); paint(); } };
  paint();
  return el;
}

/**
 * A rotary knob: drag up/down (or scroll, or use the arrow keys) to change it. The pointer sweeps
 * 270 degrees from bottom-left to bottom-right.
 */
export function knob({ min = 0, max = 1, step = 0.01, value = 0, label = '', onInput, onChange, reset, size = 64, format }) {
  const ind = h('div', { class: 'kn-ind' });
  const el = h('div', { class: 'knob', role: 'slider', tabindex: '0', 'aria-label': label, 'aria-valuemin': String(min), 'aria-valuemax': String(max), style: { '--size': `${size}px` } },
    h('div', { class: 'kn-ticks' }), h('div', { class: 'kn-cap' }, ind));
  let v = value;
  const snap = (x) => clamp(Math.round(x / step) * step, min, max);
  const paint = () => {
    const f = (v - min) / (max - min);
    el.style.setProperty('--angle', `${-135 + f * 270}deg`);
    el.style.setProperty('--f', String(f));
    el.setAttribute('aria-valuenow', String(+v.toFixed(3)));
    el.setAttribute('aria-valuetext', format ? format(v) : String(+v.toFixed(2)));
  };
  const apply = (nv) => { if (nv !== v) { v = nv; paint(); onInput?.(v); } };
  drag(el, {
    start: () => { el.classList.add('dragging'); return { v0: v }; },
    move: (_dx, dy, ev, c) => apply(snap(c.v0 + (-dy / (ev.shiftKey ? 800 : 160)) * (max - min))),
    end: () => { el.classList.remove('dragging'); onChange?.(v); },
  });
  el.addEventListener('wheel', (ev) => { ev.preventDefault(); apply(snap(v - Math.sign(ev.deltaY) * step * 3)); onChange?.(v); }, { passive: false });
  el.addEventListener('keydown', (ev) => {
    const d = { ArrowRight: step, ArrowUp: step, ArrowLeft: -step, ArrowDown: -step }[ev.key];
    if (d !== undefined) { ev.preventDefault(); apply(snap(v + d)); onChange?.(v); }
  });
  if (reset !== undefined) el.addEventListener('dblclick', () => { apply(snap(reset)); onChange?.(v); });
  el.set = (nv) => { if (!el.classList.contains('dragging') && nv !== v) { v = clamp(nv, min, max); paint(); } };
  paint();
  return el;
}

/** On/off switch. */
export function toggle({ value = false, label = '', onChange }) {
  const el = h('button', { class: 'toggle', type: 'button', role: 'switch', 'aria-checked': String(value), 'aria-label': label },
    h('span', { class: 'tg-knob' }));
  el.addEventListener('click', () => { const nv = el.getAttribute('aria-checked') !== 'true'; el.setAttribute('aria-checked', String(nv)); onChange?.(nv); });
  el.set = (nv) => el.setAttribute('aria-checked', String(!!nv));
  return el;
}

/** Row of mutually exclusive choices: options = [{value, label}]. */
export function segmented({ options, value, label = '', onChange }) {
  const el = h('div', { class: 'segmented', role: 'radiogroup', 'aria-label': label });
  const buttons = options.map((o) => {
    const b = h('button', { class: 'seg', type: 'button', role: 'radio', 'aria-checked': String(o.value === value), onclick: () => { select(o.value); onChange?.(o.value); } }, o.label);
    el.append(b);
    return [o.value, b];
  });
  function select(v) { for (const [val, b] of buttons) b.setAttribute('aria-checked', String(val === v)); }
  el.set = select;
  return el;
}

export function select({ options, value, label = '', onChange }) {
  const el = h('select', { class: 'select', 'aria-label': label, onchange: (e) => onChange?.(e.target.value) },
    options.map((o) => h('option', { value: o.value, selected: o.value === value }, o.label)));
  el.set = (v) => { el.value = v; };
  return el;
}

export function colorInput({ value, label = '', onInput, onChange }) {
  const el = h('input', { class: 'color', type: 'color', value, 'aria-label': label });
  el.addEventListener('input', () => onInput?.(el.value));
  el.addEventListener('change', () => onChange?.(el.value));
  el.set = (v) => { el.value = v; };
  return el;
}

/** Shows an element in a modal-like popover anchored near (x, y); closes on outside click or Escape. */
export function popover(content, x, y) {
  const el = h('div', { class: 'popover', role: 'dialog' }, content);
  document.body.append(el);
  const r = el.getBoundingClientRect();
  el.style.left = `${clamp(x, 8, innerWidth - r.width - 8)}px`;
  el.style.top = `${clamp(y, 8, innerHeight - r.height - 8)}px`;
  const close = () => { el.remove(); document.removeEventListener('pointerdown', away, true); document.removeEventListener('keydown', esc, true); };
  const away = (e) => { if (!el.contains(e.target)) close(); };
  const esc = (e) => { if (e.key === 'Escape') { e.stopPropagation(); close(); } };
  setTimeout(() => { document.addEventListener('pointerdown', away, true); document.addEventListener('keydown', esc, true); });
  el.close = close;
  return el;
}
