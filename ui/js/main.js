// Starts the app: loads saved settings, builds the page, wires the views.

import { on } from './bus.js';
import { initController, setView } from './controller.js';
import { loadImportedFonts } from './fonts.js';
import { eqDrawerOpen, onEqDrawer, toggleEqDrawer } from './ui/eqdrawer.js';
import { initKeys } from './keys.js';
import { mountLayout } from './layout.js';
import { cfg, featureOn, flushPrefs, getPrefs, loadPrefs, set, setPref, setSkin, takeMigrated } from './prefs.js';
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
  // Shows the mode a click switches to: a sun while it's dark, a moon while it's light.
  const scheme = h('button', { class: 'btn icon-btn scheme-btn', type: 'button', onclick: () => set('color.scheme', document.documentElement.dataset.scheme === 'dark' ? 'light' : 'dark') });
  const paintScheme = () => {
    const dark = document.documentElement.dataset.scheme === 'dark';
    const label = dark ? 'Switch to light mode' : 'Switch to dark mode';
    scheme.title = label;
    scheme.setAttribute('aria-label', label);
    scheme.dataset.mode = dark ? 'dark' : 'light';
    scheme.replaceChildren(icon(dark ? 'sun' : 'moon', 20));
  };
  on('theme', paintScheme);
  paintScheme();

  // Quick switches for the Now Playing screen: lyrics, visualisers and the equaliser drawer.
  const lyricsBtn = iconButton('lyrics', 'Lyrics', () => setPref('features.lyrics', !featureOn('lyrics')), { cls: 'tool-btn tool-lyrics' });
  const vizBtn = iconButton('viz', 'Visualisers', () => setPref('features.viz', !featureOn('viz')), { cls: 'tool-btn tool-viz' });
  const eqBtn = iconButton('eq', 'Equaliser', (e) => toggleEqDrawer(e.currentTarget), { cls: 'tool-btn tool-eq' });
  const paintTools = () => {
    const l = featureOn('lyrics'), v = featureOn('viz');
    lyricsBtn.setAttribute('aria-pressed', String(l)); lyricsBtn.title = lyricsBtn.ariaLabel = l ? 'Lyrics: on (click to hide)' : 'Lyrics: off (click to show)';
    vizBtn.setAttribute('aria-pressed', String(v)); vizBtn.title = vizBtn.ariaLabel = v ? 'Visualisers: on (click to hide)' : 'Visualisers: off (click to show)';
    eqBtn.setAttribute('aria-pressed', String(eqDrawerOpen()));
    lyricsBtn.classList.toggle('is-off', !l); vizBtn.classList.toggle('is-off', !v);
  };
  on('ui', paintTools);
  onEqDrawer(paintTools);
  paintTools();
  const nowTools = h('div', { class: 'now-tools', role: 'group', 'aria-label': 'Now playing tools' }, lyricsBtn, vizBtn, eqBtn);
  // Always visible whatever the skin does with the navigation links: settings are one click away.
  const gear = iconButton('settings', 'Settings (Ctrl+,)', () => setView('settings'), { cls: 'settings-btn' });
  on('view', (v) => gear.setAttribute('aria-pressed', String(v === 'settings')));
  const full = iconButton('fullscreen', 'Fullscreen', () => (document.fullscreenElement ? document.exitFullscreen() : document.documentElement.requestFullscreen?.().catch(() => {})), { cls: 'full-btn' });
  on('ui', () => skinSel.set(getPrefs().skin));
  on('view', (v) => nav.querySelectorAll('.nav-btn').forEach((b) => b.setAttribute('aria-current', String(b.dataset.view === v))));
  nav.querySelectorAll('.nav-btn').forEach((b) => b.setAttribute('aria-current', String(b.dataset.view === state.view)));
  return h('header', { class: 'chrome' },
    h('div', { class: 'brand' }, h('span', { class: 'brand-mark', 'aria-hidden': 'true' }), h('span', { class: 'brand-name' }, 'Redshift')),
    nav, h('div', { class: 'chrome-tools' }, nowTools, skinSel, scheme, gear, full));
}

/** Background layers that skins show or hide with CSS: ghost type, sun, rings, side labels. */
function buildDecor() {
  const title = h('span', { class: 'd-title' }), artist = h('span', { class: 'd-artist' });
  const side = h('span', { class: 'd-side' }, 'REDSHIFT · LOCAL PLAYER');
  const el = h('div', { class: 'decor', 'aria-hidden': 'true' },
    h('i', { class: 'd-split' }), h('i', { class: 'd-sun' }), h('i', { class: 'd-ring' }), h('i', { class: 'd-ring d-ring2' }), h('i', { class: 'd-record' }),
    h('i', { class: 'd-ticks' }), title, artist, side, h('i', { class: 'd-corner tl' }), h('i', { class: 'd-corner tr' }), h('i', { class: 'd-corner bl' }), h('i', { class: 'd-corner br' }));
  const render = () => {
    const t = state.track;
    title.textContent = t?.title ?? 'Redshift';
    artist.textContent = t?.artist ?? '';
    document.title = t ? `${t.title}${t.artist ? ` · ${t.artist}` : ''}` : 'Redshift';
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
  await loadImportedFonts();
  applyTheme();
  const app = document.getElementById('app');
  app.replaceChildren(h('div', { class: 'bg' }), buildDecor(), buildChrome());
  buildViews(app);
  app.append(h('div', { class: 'grain' }));
  initKeys();
  if (takeMigrated()) setTimeout(() => toast('Redshift now opens in the new Classic layout. Your other skins are still in the skin menu and Settings.'), 800);
  try { await initController(); } catch (e) { console.error(e); toast('Could not reach the audio engine', 'error'); }
  document.documentElement.dataset.ready = 'true';
  addEventListener('pagehide', flushPrefs);
  addEventListener('beforeunload', flushPrefs);
  // Pointer-down on a bare panel area drops focus from buttons so Space always means play/pause.
  window.__redshift = { cfg, state };
}

boot().catch((e) => { console.error(e); document.body.append(h('pre', { class: 'fatal' }, `Redshift could not start.\n${e?.stack ?? e}`)); });
