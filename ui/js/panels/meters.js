// Left and right level meters.

import { h } from '../util.js';
import { mountViz } from '../viz/engine.js';

export function mount(el) {
  const cv = h('canvas', { class: 'viz-canvas' });
  el.append(h('div', { class: 'meters' }, h('div', { class: 'm-labels' }, h('span', null, 'L'), h('span', null, 'R')), cv));
  const handle = mountViz(cv, { id: () => 'vu', opts: () => ({ vertical: false }) });
  return { destroy: () => handle.destroy() };
}
