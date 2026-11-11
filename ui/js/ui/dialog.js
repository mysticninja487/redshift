// Modal dialogs: ask for a line of text, or confirm something destructive.

import { h } from '../util.js';
import { button } from './controls.js';

/**
 * Opens a dialog built by `build(box, done)`; `done(value)` closes it. With `guard`, Escape and a click
 * outside ask `guard()` first (it can return a promise) and only close when it says true.
 */
export function modal(build, { guard } = {}) {
  return new Promise((resolve) => {
    const prev = document.activeElement;
    // Focus goes back after the key press has finished, or the Enter that confirmed a dialog
    // would also "click" the button that opened it.
    const done = (v) => { back.remove(); setTimeout(() => prev?.focus?.()); resolve(v); };
    const box = h('div', { class: 'dialog', role: 'dialog', 'aria-modal': 'true' });
    const cancel = async () => { if (!guard || await guard()) done(null); };
    const back = h('div', { class: 'dialog-back', onpointerdown: (e) => { if (e.target === back) cancel(); } }, box);
    build(box, done, cancel);
    back.addEventListener('keydown', (e) => { if (e.key === 'Escape') { e.stopPropagation(); cancel(); } });
    document.body.append(back);
    (box.querySelector('input') ?? box.querySelector('button.primary'))?.focus();
  });
}

/** Resolves to the entered text, or null if cancelled. */
export function promptText(title, { value = '', ok = 'Save', placeholder = '' } = {}) {
  return modal((box, done) => {
    const input = h('input', { class: 'text-input', type: 'text', value, placeholder, 'aria-label': title, onkeydown: (e) => { if (e.key === 'Enter') { e.preventDefault(); done(input.value.trim() || null); } } });
    box.append(h('h3', { class: 'dialog-title' }, title), input,
      h('div', { class: 'dialog-actions' }, button('Cancel', () => done(null)), button(ok, () => done(input.value.trim() || null), { cls: 'primary' })));
    setTimeout(() => input.select());
  });
}

/** Resolves to true if confirmed. */
export function confirmDialog(title, text, { ok = 'Delete' } = {}) {
  return modal((box, done) => {
    box.append(h('h3', { class: 'dialog-title' }, title), h('p', { class: 'dialog-text' }, text),
      h('div', { class: 'dialog-actions' }, button('Cancel', () => done(false)), button(ok, () => done(true), { cls: 'primary danger' })));
  }).then((v) => v === true);
}
