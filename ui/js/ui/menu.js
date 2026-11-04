// A small context menu. items: [{label, onClick, disabled}] or {sep: true} or {label, submenu: [...]}.

import { h } from '../util.js';

let open = null;

export function showMenu(items, x, y) {
  closeMenu();
  const el = h('div', { class: 'menu', role: 'menu' });
  const build = (list, host) => {
    for (const it of list) {
      if (it.sep) { host.append(h('div', { class: 'menu-sep', role: 'separator' })); continue; }
      const row = h('button', { class: 'menu-item', type: 'button', role: 'menuitem', disabled: it.disabled }, it.label, it.submenu && h('span', { class: 'menu-arrow' }, '›'));
      if (it.submenu) {
        const sub = h('div', { class: 'menu menu-sub', role: 'menu' });
        build(it.submenu, sub);
        row.append(sub);
        row.addEventListener('pointerenter', () => { const r = row.getBoundingClientRect(); sub.style.left = r.right + sub.offsetWidth > innerWidth ? `${-sub.offsetWidth}px` : '100%'; });
      } else row.addEventListener('click', () => { closeMenu(); it.onClick?.(); });
      host.append(row);
    }
  };
  build(items, el);
  document.body.append(el);
  const r = el.getBoundingClientRect();
  el.style.left = `${Math.max(4, Math.min(x, innerWidth - r.width - 4))}px`;
  el.style.top = `${Math.max(4, Math.min(y, innerHeight - r.height - 4))}px`;
  const away = (e) => { if (!el.contains(e.target)) closeMenu(); };
  const key = (e) => { if (e.key === 'Escape') closeMenu(); };
  setTimeout(() => { document.addEventListener('pointerdown', away, true); document.addEventListener('keydown', key, true); });
  open = { el, away, key };
  el.querySelector('button:not([disabled])')?.focus();
}

export function closeMenu() {
  if (!open) return;
  open.el.remove();
  document.removeEventListener('pointerdown', open.away, true);
  document.removeEventListener('keydown', open.key, true);
  open = null;
}
