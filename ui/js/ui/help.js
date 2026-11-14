// Help & about: a short guide, the shortcut list (with whatever you've changed them to), and the
// version and data folder. Opens with F1 or from Settings > Help & about.

import { api } from '../api.js';
import { ACTIONS, comboFor, prettyCombo } from '../keys.js';
import { h, toast } from '../util.js';
import { button } from './controls.js';
import { modal } from './dialog.js';

const ISSUES = 'https://github.com/mysticninja487/redshift/issues';

const GUIDE = [
  ['Getting started', [
    ['Add your music', 'Open Library > Folders and choose Add folder. Redshift reads your files and never changes them. New songs show up by themselves within about a minute.'],
    ['Play something', 'Double-click a song to play it, or press play on an album, artist, folder or playlist. Right-click anything for Play next, Add to queue, Add to playlist and Edit tags.'],
    ['Open files from Windows', 'Right-click a song, folder or playlist and choose Open with > Redshift, or drag it onto the window.'],
  ]],
  ['Listening', [
    ['Queue', 'The queue is on the right of Now Playing. Drag songs to reorder them, and right-click one to remove it.'],
    ['Favourites and playlists', 'Click the heart on any song, or right-click > Add to playlist. M3U and M3U8 playlists can be imported and exported.'],
    ['Lyrics', 'Synced lyrics come from a .lrc file next to the song or from its tags. In the tag editor you can add lyrics and time them line by line.'],
    ['Sleep timer', 'The clock in the top bar stops playback after a set time or at the end of the song, fading the volume out over the last ten seconds.'],
    ['Song info', 'Right-click a song > Song info, press I, or use the info button beside the title on Now Playing.'],
  ]],
  ['Your listening', [
    ['Stats', 'Press 4. See how long you listened today, this week, this month and this year, your top song, artist and album, your taste, a listening streak and mixes made from your history.'],
    ['What counts as a play', 'A song counts once you have heard a set share of it (Settings > Playback, 50% by default). Songs you only flick past still show up in recently played, but not in most played, taste or mixes.'],
  ]],
  ['Making it yours', [
    ['Skins', 'Press ] and [ to switch skins. Change one until it looks right, then Settings > Appearance > Save this look as a new skin.'],
    ['Mini player', 'Ctrl+M opens a small always-on-top player with the cover, controls and a heart. Its corner arrow expands it to show the queue or the lyrics.'],
    ['Ambient mode', 'Press A for the whole window as a visualiser or lyrics screen.'],
    ['Settings', 'A dot beside a setting means it differs from the default. Hold to compare shows the player with everything at its default.'],
  ]],
];

function guide() {
  return h('div', { class: 'hp-guide' }, GUIDE.map(([title, items]) => h('section', { class: 'hp-sec' },
    h('h4', { class: 'si-h' }, title),
    h('dl', { class: 'hp-list' }, items.map(([k, v]) => [h('dt', null, k), h('dd', null, v)])))));
}

function shortcuts() {
  return h('div', { class: 'hp-keys' },
    h('p', { class: 'hint' }, 'Change any of these in Settings > Shortcuts.'),
    h('dl', { class: 'hp-keylist' }, ACTIONS.map((a) => [h('dt', null, a.label), h('dd', null, h('kbd', null, prettyCombo(comboFor(a.id)) || '—'))])));
}

/** Version, data folder and where to report a problem. Used by the dialog and the Settings page. */
export function aboutBlock() {
  const box = h('div', { class: 'hp-about' });
  const copy = async (text, what) => { try { await navigator.clipboard.writeText(text); toast(`${what} copied`); } catch { toast('Couldn’t copy that'); } };
  api.appInfo().then((i) => {
    box.replaceChildren(
      h('p', { class: 'hp-name' }, h('b', null, 'Redshift'), h('span', null, `version ${i.version}`)),
      h('p', { class: 'hint' }, 'A local music player. No account, no streaming, no tracking: it plays the files you point it at.'),
      h('dl', { class: 'hp-list' },
        h('dt', null, 'Your data'), h('dd', null, h('span', { class: 'si-path' }, i.data_dir), h('span', { class: 'hp-hint' }, 'Library, playlists, play counts, history and settings. Nothing in your music folders is ever changed.')),
        h('dt', null, 'Problems'), h('dd', null, h('span', { class: 'si-path' }, ISSUES))),
      h('div', { class: 'dialog-actions hp-actions' },
        button('Open data folder', () => api.openDataFolder().catch((e) => toast(`Couldn’t open it: ${e}`))),
        button('Copy data path', () => copy(i.data_dir, 'Path')),
        button('Copy issues link', () => copy(ISSUES, 'Link'))));
  }).catch(() => box.replaceChildren(h('p', { class: 'hint' }, 'Version information isn’t available here.')));
  return box;
}

/** Opens the help dialog on the given tab ('guide', 'keys' or 'about'). */
export function openHelp(start = 'guide') {
  return modal((box, done) => {
    box.classList.add('si-dialog', 'hp-dialog');
    const panes = { guide: guide, keys: shortcuts, about: aboutBlock };
    const names = { guide: 'Guide', keys: 'Shortcuts', about: 'About' };
    const body = h('div', { class: 'si-body', role: 'tabpanel' });
    const tabs = h('div', { class: 'hp-tabs', role: 'tablist', 'aria-label': 'Help sections' });
    const show = (id) => {
      for (const b of tabs.children) { const on = b.dataset.id === id; b.setAttribute('aria-selected', String(on)); b.tabIndex = on ? 0 : -1; }
      body.replaceChildren(panes[id]());
      body.scrollTop = 0;
    };
    for (const id of Object.keys(panes)) tabs.append(h('button', { class: 'hp-tab', type: 'button', role: 'tab', 'data-id': id, onclick: () => show(id) }, names[id]));
    tabs.addEventListener('keydown', (e) => {
      const ids = Object.keys(panes), i = ids.indexOf(document.activeElement?.dataset?.id);
      if (i < 0 || !['ArrowLeft', 'ArrowRight'].includes(e.key)) return;
      e.preventDefault();
      const next = ids[(i + (e.key === 'ArrowRight' ? 1 : ids.length - 1)) % ids.length];
      show(next); tabs.querySelector(`[data-id="${next}"]`).focus();
    });
    box.append(h('h3', { class: 'dialog-title' }, 'Help & about'), tabs, body, h('div', { class: 'dialog-actions' }, button('Close', () => done(), { cls: 'primary' })));
    show(start);
  });
}
