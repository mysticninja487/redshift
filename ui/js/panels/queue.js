// The play queue: click to jump, drag the grip to reorder, remove with the cross.

import { api } from '../api.js';
import { act, getMeta } from '../controller.js';
import { state } from '../state.js';
import { clear, fmtTime, h } from '../util.js';
import { iconButton } from '../ui/controls.js';
import { icon } from '../ui/icons.js';
import { showMenu } from '../ui/menu.js';
import { trackMenuItems } from '../ui/trackmenu.js';
import { virtualList } from '../ui/vlist.js';
import { lifecycle } from './common.js';

const ROW = { compact: 36, cozy: 46, roomy: 58 };

export function mount(el) {
  const L = lifecycle();
  const title = h('div', { class: 'q-title' }, 'Up next');
  const count = h('div', { class: 'q-count' });
  const head = h('div', { class: 'panel-head' }, title, count, iconButton('trash', 'Clear queue', () => act(api.queueClear()), { size: 16, cls: 'q-clear' }));
  const empty = h('div', { class: 'empty' }, icon('queue', 28), h('p', null, 'The queue is empty. Play an album or add tracks from the library.'));
  const meta = new Map(); // path -> tags, filled in as rows come into view
  let dragging = null;

  const rowH = () => ROW[document.documentElement.dataset.density] ?? 46;
  const list = virtualList({ rowHeight: rowH(), cls: 'q-list', render: (i) => row(i) });
  el.append(h('div', { class: 'queue' }, head, list, empty));

  function row(i) {
    const e = state.queue.entries[i];
    const m = meta.get(e.path);
    if (m === undefined) getMeta(e.path).then((t) => { meta.set(e.path, t ?? null); list.refresh(); });
    const isCur = e.id === state.queue.current_entry;
    const playing = isCur && state.player.status === 'playing';
    const grip = h('span', { class: 'q-grip', title: 'Drag to reorder' }, icon('grip', 14));
    const r = h('div', { class: `q-row${isCur ? ' current' : ''}${playing ? ' playing' : ''}`, role: 'listitem', dataset: { id: e.id }, ondblclick: () => act(api.playEntry(e.id)) },
      h('span', { class: 'q-no' }, isCur ? h('span', { class: 'bars-anim', 'aria-label': 'Playing' }, h('i'), h('i'), h('i')) : String(i + 1)),
      h('button', { class: 'q-main', type: 'button', onclick: () => act(api.playEntry(e.id)) },
        h('span', { class: 'q-name' }, m?.title ?? e.path.split(/[\\/]/).pop()),
        h('span', { class: 'q-artist' }, m?.artist ?? '')),
      h('span', { class: 'q-dur' }, m?.duration_secs ? fmtTime(m.duration_secs) : ''),
      grip,
      iconButton('close', 'Remove from queue', () => act(api.queueRemove(e.id)), { size: 14, cls: 'q-remove' }));
    r.addEventListener('contextmenu', async (ev) => {
      ev.preventDefault();
      const items = await trackMenuItems([e.path], [{ label: 'Remove from queue', onClick: () => act(api.queueRemove(e.id)) }, { sep: true }]);
      showMenu(items, ev.clientX, ev.clientY);
    });
    // Reordering: drag the grip; the drop slot shows as a line.
    grip.addEventListener('pointerdown', (ev) => {
      if (ev.button !== 0) return;
      ev.preventDefault();
      grip.setPointerCapture(ev.pointerId);
      dragging = { from: i, to: i };
      r.classList.add('dragging');
      const mark = h('div', { class: 'q-drop' });
      list.append(mark);
      const move = (e2) => {
        const lr = list.getBoundingClientRect();
        const idx = Math.max(0, Math.min(state.queue.entries.length - 1, Math.floor((e2.clientY - lr.top + list.scrollTop) / rowH())));
        dragging.to = idx;
        mark.style.top = `${(idx + (idx > dragging.from ? 1 : 0)) * rowH() - list.scrollTop}px`;
        // Scroll when dragging near the edges.
        if (e2.clientY < lr.top + 24) list.scrollTop -= 12; else if (e2.clientY > lr.bottom - 24) list.scrollTop += 12;
      };
      const up = () => {
        grip.removeEventListener('pointermove', move); grip.removeEventListener('pointerup', up); grip.removeEventListener('pointercancel', up);
        mark.remove(); r.classList.remove('dragging');
        if (dragging && dragging.from !== dragging.to) act(api.queueMove(dragging.from, dragging.to));
        dragging = null;
      };
      grip.addEventListener('pointermove', move); grip.addEventListener('pointerup', up); grip.addEventListener('pointercancel', up);
    });
    return r;
  }

  function render() {
    const n = state.queue.entries.length;
    list.setCount(n);
    empty.hidden = n > 0; list.hidden = n === 0;
    const total = state.queue.entries.reduce((s, e) => s + (meta.get(e.path)?.duration_secs ?? 0), 0);
    count.textContent = n ? `${n} ${n === 1 ? 'track' : 'tracks'}${total ? `, ${fmtTime(total)}` : ''}` : '';
    const cur = state.queue.entries.findIndex((e) => e.id === state.queue.current_entry);
    if (cur >= 0 && !dragging) list.scrollToIndex(cur, false);
  }
  L.on(['queue', 'status', 'ui'], render);
  render();
  return { destroy: () => L.destroy() };
}
