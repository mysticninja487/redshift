// "Import online playlist…": paste a public Deezer, Spotify or YouTube playlist link (or a list of
// songs), see which of the songs are in the library, and make a playlist of the ones that are.

import { api } from '../api.js';
import { askToGoOnline, online } from '../online.js';
import { h, toast } from '../util.js';
import { button } from './controls.js';
import { modal } from './dialog.js';

const OFFLINE = /turned off/i;

/** Resolves to the new playlist's id, or null if nothing was made. */
export function importOnlinePlaylist() {
  return modal((box, done) => {
    box.classList.add('pi-dialog');
    const input = h('textarea', {
      class: 'text-input pi-input', rows: 4, spellcheck: false, 'aria-label': 'Playlist link or list of songs',
      placeholder: 'Playlist link, or Artist - Title on each line',
    });
    const note = h('p', { class: 'hint' }, 'Works with public Deezer, Spotify and YouTube playlists, and with pasted lists, including Spotify CSV exports. A link sends only that address to the service, and only when you press Find songs. Songs are matched to files in your library; nothing is downloaded.');
    const status = h('p', { class: 'pi-status', role: 'status', hidden: true });
    const result = h('div', { class: 'pi-result', hidden: true });
    const find = button('Find songs', () => run(), { cls: 'primary' });
    const cancel = button('Cancel', () => done(null));
    const actions = h('div', { class: 'dialog-actions' }, cancel, find);
    box.append(h('h3', { class: 'dialog-title' }, 'Import a playlist'), input, note, status, result, actions);

    const say = (text, kind = '') => { status.hidden = !text; status.textContent = text; status.dataset.kind = kind; };

    async function run() {
      const text = input.value.trim();
      if (!text) { say('Paste a playlist link or a list of songs first.', 'error'); return; }
      const isLink = !text.includes('\n') && /^https?:\/\//i.test(text);
      if (isLink && !online().enabled && !await askToGoOnline()) return;
      find.disabled = true; result.hidden = true;
      say(isLink ? 'Reading the playlist…' : 'Matching songs…');
      try {
        show(await api.playlistImportPreview(text));
      } catch (e) {
        const msg = String(e?.message ?? e);
        say(OFFLINE.test(msg) ? 'Online features are off. Turn them on in Settings > Online, or paste the list as text.' : msg, 'error');
      }
      find.disabled = false;
    }

    function show(p) {
      say('');
      const found = p.rows.filter((r) => r.path);
      const missing = p.rows.filter((r) => !r.path);
      const name = h('input', { class: 'text-input', type: 'text', value: p.name || 'Imported playlist', 'aria-label': 'Playlist name', translate: 'no' });
      const sum = h('p', { class: 'pi-sum' }, `Songs found in your library: ${found.length} of ${p.total}`, ...(p.source ? [h('span', { class: 'pi-src' }, ' · '), h('span', { class: 'pi-src' }, p.source)] : []));
      const trunc = p.truncated ? h('p', { class: 'hint' }, 'The service only lists the first part of a long playlist, so some songs may be missing here. A pasted list or CSV has no such limit.') : null;
      const list = h('div', { class: 'pi-list', role: 'list' }, p.rows.map((r) => h('div', { class: 'pi-row', role: 'listitem', 'data-found': String(!!r.path) },
        h('span', { class: 'pi-mark', title: r.path ? 'Found in your library' : 'Not in your library' }, r.path ? '✓' : '–'),
        h('span', { class: 'pi-name', translate: 'no' }, r.artist ? `${r.title} · ${r.artist}` : r.title))));
      const copyMissing = missing.length ? button('Copy missing songs', async () => {
        try { await navigator.clipboard.writeText(missing.map((r) => (r.artist ? `${r.artist} - ${r.title}` : r.title)).join('\n')); toast('Copied the songs that were not found'); }
        catch { toast('Could not copy', 'error'); }
      }) : null;
      const make = button('Create playlist', async () => {
        if (!found.length) return;
        make.disabled = true;
        try {
          const id = await api.createPlaylist(name.value.trim() || 'Imported playlist', found.map((r) => r.path));
          toast(`Created "${name.value.trim() || 'Imported playlist'}" (${found.length} of ${p.total} songs found)`);
          done(id);
        } catch (e) { toast(String(e?.message ?? e), 'error'); make.disabled = false; }
      }, { cls: 'primary' });
      make.disabled = !found.length;
      result.replaceChildren(sum, ...(trunc ? [trunc] : []), h('label', { class: 'pi-name-label' }, h('span', {}, 'Playlist name'), name), list,
        ...(found.length ? [] : [h('p', { class: 'hint' }, 'None of these songs are in your library. Add the folders that hold them in the Folders tab, then try again.')]));
      result.hidden = false;
      actions.replaceChildren(...(copyMissing ? [copyMissing] : []), cancel, make);
      box.querySelector('.pi-list')?.scrollTo?.(0, 0);
    }
    setTimeout(() => input.focus());
  });
}
