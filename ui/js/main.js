// Starts the app: loads saved settings, builds the page, wires the views.

import { on } from './bus.js';
import { initController, setView } from './controller.js';
import { initKeys } from './keys.js';
import { mountLayout } from './layout.js';
import { cfg, flushPrefs, getPrefs, loadPrefs, set, setSkin } from './prefs.js';
import { skins } from './skins.js';
import { state } from './state.js';
import { applyTheme } from './theme.js';
import { h, toast } from './util.js';
import { iconButton, select } from './ui/controls.js';
import { icon } from './ui/icons.js';

const NAV = [['now', 'Now playing', 'record'], ['library', 'Library', 'library'], ['settings', 'Settings', 'settings']];

function buildChrome() {
  const nav = h('nav', { class: 'nav', 'aria-label': 'Sections' }, NAV.map(([id, label, ic]) =>
    h('button', { class: 'nav-btn', type: 'button', dataset: { view: id }, onclick: () => setView(id) }, icon(ic, 16), h('span', null, label))));
  const skinSel = select({ label: 'Skin', value: getPrefs().skin, options: skins.map((s) => ({ value: s.id, label: s.name })), onChange: (v) => setSkin(v) });
  skinSel.classList.add('skin-select');
  const scheme = iconButton('sliders', 'Switch light or dark', () => set('color.scheme', document.documentElement.dataset.scheme === 'dark' ? 'light' : 'dark'), { cls: 'scheme-btn' });
  const full = iconButton('fullscreen', 'Fullscreen', () => (document.fullscreenElement ? document.exitFullscreen() : document.documentElement.requestFullscreen?.().catch(() => {})), { cls: 'full-btn' });
  on('ui', () => skinSel.set(getPrefs().skin));
  on('view', (v) => nav.querySelectorAll('.nav-btn').forEach((b) => b.setAttribute('aria-current', String(b.dataset.view === v))));
  nav.querySelectorAll('.nav-btn').forEach((b) => b.setAttribute('aria-current', String(b.dataset.view === state.view)));
  return h('header', { class: 'chrome' },
    h('div', { class: 'brand' }, h('span', { class: 'brand-mark', 'aria-hidden': 'true' }), h('span', { class: 'brand-name' }, 'Sonara')),
    nav, h('div', { class: 'chrome-tools' }, skinSel, scheme, full));
}

/** Background layers that skins show or hide with CSS: ghost type, sun, rings, side labels. */
function buildDecor() {
  const title = h('span', { class: 'd-title' }), artist = h('span', { class: 'd-artist' });
  const side = h('span', { class: 'd-side' }, 'SONARA · LOCAL PLAYER');
  const el = h('div', { class: 'decor', 'aria-hidden': 'true' },
    h('i', { class: 'd-split' }), h('i', { class: 'd-sun' }), h('i', { class: 'd-ring' }), h('i', { class: 'd-ring d-ring2' }), h('i', { class: 'd-record' }),
    h('i', { class: 'd-ticks' }), title, artist, side, h('i', { class: 'd-corner tl' }), h('i', { class: 'd-corner tr' }), h('i', { class: 'd-corner bl' }), h('i', { class: 'd-corner br' }));
  const render = () => {
    const t = state.track;
    title.textContent = t?.title ?? 'Sonara';
    artist.textContent = t?.artist ?? '';
    document.title = t ? `${t.title}${t.artist ? ` · ${t.artist}` : ''}` : 'Sonara';
  };
  on('track', render); render();
  return el;
}

function buildViews(app) {
  const now = h('div', { id: 'now', class: 'view stage-grid' });
  const library = h('div', { id: 'library', class: 'view', hidden: true });
  const settings = h('div', { id: 'settings', class: 'view', hidden: true });
  const main = h('main', { class: 'views' }, now, library, settings);
  app.append(main);
  mountLayout(now);
  const mounted = {};
  const loaders = { library: () => import('./views/library.js').then((m) => m.mountLibrary(library)), settings: () => import('./views/settings.js').then((m) => m.mountSettings(settings)) };
  const show = async (v) => {
    document.documentElement.dataset.view = v;
    now.hidden = v !== 'now'; library.hidden = v !== 'library'; settings.hidden = v !== 'settings';
    if (v !== 'now' && !mounted[v]) {
      mounted[v] = true;
      try { mounted[v] = await loaders[v](); } catch (e) { console.error(e); toast(`Could not open ${v}`, 'error'); mounted[v] = false; }
    } else if (v === 'library') mounted.library?.reload?.();
  };
  on('view', show);
  show(state.view);
  addEventListener('keydown', (e) => { if (e.key === 'Escape' && state.view !== 'now' && !document.querySelector('.dialog-back,.popover,.menu')) setView('now'); });
}

async function boot() {
  if (!window.__TAURI__) await import('../dev/mock.js');
  await loadPrefs();
  applyTheme();
  const app = document.getElementById('app');
  app.replaceChildren(h('div', { class: 'bg' }), buildDecor(), buildChrome());
  buildViews(app);
  app.append(h('div', { class: 'grain' }));
  initKeys();
  try { await initController(); } catch (e) { console.error(e); toast('Could not reach the audio engine', 'error'); }
  document.documentElement.dataset.ready = 'true';
  addEventListener('pagehide', flushPrefs);
  addEventListener('beforeunload', flushPrefs);
  // Pointer-down on a bare panel area drops focus from buttons so Space always means play/pause.
  window.__sonara = { cfg, state };
}

boot().catch((e) => { console.error(e); document.body.append(h('pre', { class: 'fatal' }, `Sonara could not start.\n${e?.stack ?? e}`)); });
