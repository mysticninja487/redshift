// A virtual list: only the rows in view exist in the DOM, so a queue or library of tens of
// thousands of tracks scrolls smoothly. Rows have one fixed height.

import { h } from '../util.js';

/**
 * @param {object} o
 * @param {number} o.rowHeight  CSS pixels per row
 * @param {(index:number)=>HTMLElement} o.render  builds row `index`
 * @param {number} [o.overscan]
 * Returns an element with `.setCount(n)`, `.refresh()` and `.scrollToIndex(i, center)`.
 */
export function virtualList({ rowHeight, render, overscan = 6, cls = '' }) {
  const spacer = h('div', { class: 'vl-spacer' });
  const rows = h('div', { class: 'vl-rows' });
  const el = h('div', { class: `vlist ${cls}`.trim(), role: 'list' }, spacer, rows);
  let count = 0, first = -1, last = -1;
  const paint = (force = false) => {
    const top = el.scrollTop, vh = el.clientHeight || 400;
    const a = Math.max(0, Math.floor(top / rowHeight) - overscan);
    const b = Math.min(count - 1, Math.ceil((top + vh) / rowHeight) + overscan);
    if (!force && a === first && b === last) return;
    first = a; last = b;
    rows.style.transform = `translateY(${a * rowHeight}px)`;
    const frag = document.createDocumentFragment();
    for (let i = a; i <= b; i++) { const r = render(i); r.style.height = `${rowHeight}px`; r.dataset.index = i; frag.append(r); }
    rows.replaceChildren(frag);
  };
  el.addEventListener('scroll', () => paint(), { passive: true });
  new ResizeObserver(() => paint(true)).observe(el);
  el.setCount = (n) => { count = n; spacer.style.height = `${n * rowHeight}px`; paint(true); };
  el.refresh = () => paint(true);
  el.scrollToIndex = (i, center = false) => {
    const y = i * rowHeight;
    if (center) el.scrollTop = Math.max(0, y - el.clientHeight / 2 + rowHeight / 2);
    else if (y < el.scrollTop) el.scrollTop = y;
    else if (y + rowHeight > el.scrollTop + el.clientHeight) el.scrollTop = y + rowHeight - el.clientHeight;
  };
  return el;
}
