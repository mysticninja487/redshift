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

/** Keeps the drawer off the progress bar and the controls, wherever the skin and the layout put them. */
function fit() {
  if (!drawer) return;
  const width = Math.min(560, innerWidth - 24), left = innerWidth - 12 - width;
  let top = 64, bottom = 12;
  for (const id of ['seek', 'transport']) {
    const el = document.querySelector(`.slot:not([data-empty="true"]) > .panel[data-panel="${id}"]`);
    if (!el || el.offsetParent === null) continue;
    // Skins draw the bar as a card around its panels (the strip itself), so the strip is what to clear.
    const b = el.getBoundingClientRect(), strip = el.parentElement;
    let r = { left: b.left, right: b.right, top: b.top, bottom: b.bottom, width: b.width, height: b.height };
    if (strip && (strip.dataset.slot === 'bottom' || strip.dataset.slot === 'top')) {
      const s = strip.getBoundingClientRect();
      r = { ...r, left: Math.min(r.left, s.left), right: Math.max(r.right, s.right), top: Math.min(r.top, s.top), bottom: Math.max(r.bottom, s.bottom) };
    }
    if (!r.width || !r.height || r.right <= left || r.left >= innerWidth) continue; // not under the drawer
    if (r.top > innerHeight / 2) bottom = Math.max(bottom, innerHeight - r.top + 12);
    else top = Math.max(top, r.bottom + 12);
  }
  // Leave room to be useful: if the bars squeeze it too far, it still never covers them.
  drawer.style.top = `${top}px`; drawer.style.bottom = `${bottom}px`;
}

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
  addEventListener('resize', () => { if (eqDrawerOpen()) fit(); });
  on('ui', () => { if (eqDrawerOpen()) requestAnimationFrame(fit); });
}

export function setEqDrawer(open, from) {
  if (open && !drawer) build();
  if (!drawer) return;
  if (open) opener = from ?? document.activeElement;
  if (open) fit();
  drawer.classList.toggle('open', open);
  drawer.setAttribute('aria-hidden', String(!open));
  drawer.inert = !open;
  if (open && state.view !== 'now') setView('now');
  if (open) drawer.querySelector('.select, button')?.focus?.({ preventScroll: true });
  listeners.forEach((fn) => fn(open));
}
export const toggleEqDrawer = (from) => setEqDrawer(!eqDrawerOpen(), from);
