// Keyboard shortcuts. Every action has a default key combo; the person can remap any of them in
// Settings. Combos are written like "Shift+ArrowRight" (modifiers in the order Ctrl, Alt, Shift).

import { api } from './api.js';
import { act, setView, toggleSettings } from './controller.js';
import { featureOn, get, getPrefs, set, setPref, setSkin } from './prefs.js';
import { toggleAmbient } from './ambient.js';
import { toggleEqDrawer } from './ui/eqdrawer.js';
import { skins } from './skins.js';
import { VIZ } from './viz/index.js';
import { position, state } from './state.js';
import { clamp, toast } from './util.js';

const seekBy = (d) => act(api.seek(clamp(position() + d, 0, Math.max(0, state.queue.current?.duration_secs ?? 1e9))));
const volBy = (d) => act(api.setVolume(clamp(state.player.volume + d, 0, 1)));
const cycle = (list, cur, d) => list[(list.indexOf(cur) + d + list.length) % list.length];

export const ACTIONS = [
  { id: 'playpause', label: 'Play / pause', key: 'Space', run: () => act(state.queue.entries.length ? api.togglePause() : Promise.resolve()) },
  { id: 'next', label: 'Next track', key: 'Shift+ArrowRight', run: () => act(api.next()) },
  { id: 'prev', label: 'Previous track', key: 'Shift+ArrowLeft', run: () => act(api.previous()) },
  { id: 'fwd', label: 'Skip forward 5 s', key: 'ArrowRight', run: () => seekBy(5) },
  { id: 'back', label: 'Skip back 5 s', key: 'ArrowLeft', run: () => seekBy(-5) },
  { id: 'volup', label: 'Volume up', key: 'ArrowUp', run: () => volBy(0.05) },
  { id: 'voldown', label: 'Volume down', key: 'ArrowDown', run: () => volBy(-0.05) },
  { id: 'mute', label: 'Mute', key: 'M', run: () => act(api.setMuted(!state.player.muted)) },
  { id: 'shuffle', label: 'Shuffle', key: 'S', run: () => act(api.setShuffle(!state.queue.shuffle)) },
  { id: 'repeat', label: 'Repeat', key: 'R', run: () => act(api.setRepeat({ off: 'all', all: 'one', one: 'off' }[state.queue.repeat] ?? 'off')) },
  { id: 'now', label: 'Now playing', key: '1', run: () => setView('now') },
  { id: 'library', label: 'Library', key: '2', run: () => setView('library') },
  { id: 'settings', label: 'Settings', key: '3', run: () => toggleSettings() },
  { id: 'search', label: 'Search library', key: 'Ctrl+F', run: () => { setView('library'); setTimeout(() => document.querySelector('.lib-search')?.focus(), 30); } },
  { id: 'skinnext', label: 'Next skin', key: ']', run: () => { const s = cycle(skins.map((x) => x.id), getPrefs().skin, 1); setSkin(s); toast(`Skin: ${skins.find((x) => x.id === s).name}`); } },
  { id: 'skinprev', label: 'Previous skin', key: '[', run: () => { const s = cycle(skins.map((x) => x.id), getPrefs().skin, -1); setSkin(s); toast(`Skin: ${skins.find((x) => x.id === s).name}`); } },
  { id: 'viznext', label: 'Next visualiser', key: 'V', run: () => { const ids = VIZ.map((v) => v.id); const target = get('hero.mode') === 'viz' ? 'hero.viz' : 'viz.main'; const n = cycle(ids, get(target), 1); set(target, n); toast(`Visualiser: ${VIZ.find((v) => v.id === n).name}`); } },
  { id: 'lyrics', label: 'Lyrics on / off', key: 'L', run: () => { setPref('features.lyrics', !featureOn('lyrics')); toast(`Lyrics ${featureOn('lyrics') ? 'on' : 'off'}`); } },
  { id: 'vizonoff', label: 'Visualisers on / off', key: 'Shift+V', run: () => { setPref('features.viz', !featureOn('viz')); toast(`Visualisers ${featureOn('viz') ? 'on' : 'off'}`); } },
  { id: 'queueonoff', label: 'Queue on / off', key: 'Shift+Q', run: () => { setPref('features.queue', !featureOn('queue')); toast(`Queue ${featureOn('queue') ? 'on' : 'off'}`); } },
  { id: 'mini', label: 'Mini player', key: 'Ctrl+M', run: () => api.toggleMiniPlayer() },
  { id: 'eqdrawer', label: 'Equaliser', key: 'E', run: () => toggleEqDrawer() },
  { id: 'scheme', label: 'Light / dark', key: 'D', run: () => { const cur = document.documentElement.dataset.scheme; set('color.scheme', cur === 'dark' ? 'light' : 'dark'); } },
  { id: 'tags', label: 'Edit tags of the current track', key: 'T', run: async () => { if (!state.trackPath) return; const { openTagEditor } = await import('./ui/tageditor.js'); openTagEditor([state.trackPath]); } },
  { id: 'ambient', label: 'Ambient mode', key: 'A', run: () => toggleAmbient() },
  { id: 'fullscreen', label: 'Fullscreen', key: 'F', run: () => { if (document.fullscreenElement) document.exitFullscreen(); else document.documentElement.requestFullscreen?.().catch(() => {}); } },
];

export const comboFor = (id) => getPrefs().keys[id] ?? ACTIONS.find((a) => a.id === id)?.key ?? '';

export function comboFromEvent(e) {
  if (['Control', 'Shift', 'Alt', 'Meta'].includes(e.key)) return null;
  let k = e.code === 'Space' ? 'Space' : e.key.length === 1 ? e.key.toUpperCase() : e.key;
  // Shift is part of the combo for letters, digits and named keys; for symbols like "[" it is
  // already baked into the character, so it is left out.
  const shift = e.shiftKey && !(e.key.length === 1 && !/[A-Za-z0-9]/.test(e.key));
  const mods = [e.ctrlKey || e.metaKey ? 'Ctrl' : '', e.altKey ? 'Alt' : '', shift ? 'Shift' : ''].filter(Boolean);
  return [...mods, k].join('+');
}

export function setCombo(id, combo) {
  const keys = { ...getPrefs().keys };
  if (combo == null) delete keys[id]; else keys[id] = combo;
  setPref('keys', keys);
}

export const prettyCombo = (c) => c.replace('ArrowRight', '→').replace('ArrowLeft', '←').replace('ArrowUp', '↑').replace('ArrowDown', '↓');

let capturing = false;
/** Settings sets this while it is waiting for the person to press a new key. */
export const captureKeys = (on) => { capturing = on; };

export function initKeys() {
  addEventListener('keydown', (e) => {
    if (capturing || e.defaultPrevented) return;
    const t = e.target;
    if (t instanceof HTMLElement && (t.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(t.tagName) && !(t.type === 'range'))) return;
    // Buttons and sliders use Space/Enter/arrows themselves.
    if (t instanceof HTMLElement && (t.closest('[role="slider"]') || t.closest('.segmented')) && /^(Arrow|Home|End|Page)/.test(e.key)) return;
    if (t instanceof HTMLElement && t.tagName === 'BUTTON' && (e.key === ' ' || e.key === 'Enter')) return;
    if (document.querySelector('.dialog-back')) return;
    if ((e.ctrlKey || e.metaKey) && e.key === ',') { e.preventDefault(); toggleSettings(); return; }
    const combo = comboFromEvent(e);
    if (!combo) return;
    const action = ACTIONS.find((a) => comboFor(a.id) === combo);
    if (!action) return;
    e.preventDefault();
    action.run();
  });
}
