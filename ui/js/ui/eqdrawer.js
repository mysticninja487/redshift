// The equaliser as a drawer on the Now Playing screen, so it's one click away in any skin.
// It hosts the same panel as the layout's "Equaliser" panel (presets, bands, pre-amp).

import { on } from '../bus.js';
import { setView } from '../controller.js';
import { state } from '../state.js';
import { h } from '../util.js';
import { mount as mountEq } from '../panels/eq.js';
import { iconButton } from './controls.js';

let drawer = null;
let opener = null;
const listeners = new Set();

export const eqDrawerOpen = () => !!drawer?.classList.contains('open');
export const onEqDrawer = (fn) => { listeners.add(fn); return () => listeners.delete(fn); };

function build() {
  const host = h('div', { class: 'panel eq-host', dataset: { panel: 'eq', slot: 'right' } });
  drawer = h('aside', { class: 'eq-drawer', role: 'dialog', 'aria-label': 'Equaliser', 'aria-hidden': 'true' },
    h('div', { class: 'eq-drawer-head' }, h('span', null, 'Equaliser'), iconButton('close', 'Close equaliser', () => setEqDrawer(false), { size: 18 })), host);
  document.getElementById('app').append(drawer);
  mountEq(host); // mounted once, the first time the drawer opens
  addEventListener('keydown', (e) => {
    if (e.key === 'Escape' && eqDrawerOpen() && !document.querySelector('.dialog-back,.menu')) { e.stopPropagation(); setEqDrawer(false); opener?.focus?.(); }
  }, true);
  on('view', (v) => { if (v !== 'now') setEqDrawer(false); });
}

export function setEqDrawer(open, from) {
  if (open && !drawer) build();
  if (!drawer) return;
  if (open) opener = from ?? document.activeElement;
  drawer.classList.toggle('open', open);
  drawer.setAttribute('aria-hidden', String(!open));
  drawer.inert = !open;
  if (open && state.view !== 'now') setView('now');
  if (open) drawer.querySelector('.select, button')?.focus?.({ preventScroll: true });
  listeners.forEach((fn) => fn(open));
}
export const toggleEqDrawer = (from) => setEqDrawer(!eqDrawerOpen(), from);
