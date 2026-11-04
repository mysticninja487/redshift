// A visualiser of its own, separate from the hero. Hover for previous/next arrows.

import { get, set } from '../prefs.js';
import { h } from '../util.js';
import { iconButton } from '../ui/controls.js';
import { mountViz } from '../viz/engine.js';
import { getViz, VIZ } from '../viz/index.js';
import { lifecycle } from './common.js';

export function mount(el) {
  const L = lifecycle();
  const cv = h('canvas', { class: 'viz-canvas' });
  const name = h('span', { class: 'viz-name' });
  const step = (d) => { const i = VIZ.findIndex((v) => v.id === get('viz.side')); set('viz.side', VIZ[(i + d + VIZ.length) % VIZ.length].id); };
  const picker = h('div', { class: 'viz-picker' }, iconButton('chevronLeft', 'Previous visualiser', () => step(-1), { size: 16 }), name, iconButton('chevronRight', 'Next visualiser', () => step(1), { size: 16 }));
  el.append(h('div', { class: 'vizpanel' }, cv, picker));
  const handle = mountViz(cv, { id: () => get('viz.side') });
  const label = () => { name.textContent = getViz(get('viz.side'))?.name ?? ''; };
  L.on('ui', label); label();
  return { destroy: () => { handle.destroy(); L.destroy(); } };
}
