// Puts panels into the skin's slots. Panels are mounted once and moved, not rebuilt, so dragging
// the lyrics from one side to the other keeps its scroll position and any running animation.

import { MODULES } from './panels/index.js';
import { cfg } from './prefs.js';
import { PANELS, SLOTS } from './skins.js';
import { h } from './util.js';
import { on } from './bus.js';

export function mountLayout(root) {
  const slotEls = {};
  for (const id of SLOTS) {
    slotEls[id] = h('section', { class: 'slot', dataset: { slot: id }, 'aria-label': id });
    root.append(slotEls[id]);
  }
  const live = new Map(); // panel id -> { wrap, inst }

  function apply() {
    const assignment = cfg().layout.slots;
    const wanted = new Set();
    for (const slot of SLOTS) {
      const el = slotEls[slot];
      const order = [];
      for (const id of assignment[slot] ?? []) {
        if (!MODULES[id] || wanted.has(id)) continue; // unknown, or already placed elsewhere
        wanted.add(id);
        let p = live.get(id);
        if (!p) {
          const wrap = h('div', { class: 'panel', dataset: { panel: id, slot }, 'aria-label': PANELS.find((x) => x.id === id)?.name });
          p = { wrap, inst: null };
          live.set(id, p);
          try { p.inst = MODULES[id].mount(wrap); } catch (e) { console.error(`panel "${id}" failed to start`, e); wrap.append(h('div', { class: 'empty' }, `The ${id} panel failed to load.`)); }
        }
        p.wrap.dataset.slot = slot;
        order.push(p.wrap);
      }
      // Reorder only when needed, so moving an unrelated panel doesn't disturb this slot.
      const same = order.length === el.children.length && order.every((n, i) => el.children[i] === n);
      if (!same) el.replaceChildren(...order);
      el.dataset.empty = String(order.length === 0);
      el.dataset.count = String(order.length);
    }
    for (const [id, p] of live) {
      if (wanted.has(id)) continue;
      try { p.inst?.destroy?.(); } catch (e) { console.error(e); }
      p.wrap.remove(); live.delete(id);
    }
  }
  on('ui', apply);
  apply();
  return { apply };
}
