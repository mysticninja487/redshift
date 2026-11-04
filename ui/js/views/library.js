// Browsing the music: albums, artists, all tracks, playlists and the folders being watched.

import { api, coverUrl, pickFile, pickFolder, pickSave } from '../api.js';
import { on, emit } from '../bus.js';
import { act, playPaths, queueAdd, rescanLibrary, setView } from '../controller.js';
import { getPrefs, setPref } from '../prefs.js';
import { state } from '../state.js';
import { clear, debounce, fmtTime, h, toast } from '../util.js';
import { button, iconButton } from '../ui/controls.js';
import { confirmDialog, promptText } from '../ui/dialog.js';
import { icon } from '../ui/icons.js';
import { showMenu } from '../ui/menu.js';
import { trackMenuItems } from '../ui/trackmenu.js';
import { virtualList } from '../ui/vlist.js';

const TABS = [['albums', 'Albums'], ['artists', 'Artists'], ['tracks', 'Tracks'], ['playlists', 'Playlists'], ['folders', 'Folders']];

export function mountLibrary(root) {
  let tab = getPrefs().library.tab ?? 'albums';
  let data = { albums: [], artists: [], tracks: [], byPath: new Map(), playlists: [], folders: [] };
  let detail = null; // { type: 'album' | 'artist' | 'playlist', ... } shown over the tab
  let query = '';

  const search = h('input', { class: 'text-input lib-search', type: 'search', placeholder: 'Search title, artist or album', 'aria-label': 'Search library', oninput: debounce((e) => { query = e.target.value.trim(); detail = null; render(); }, 160) });
  const tabs = h('div', { class: 'lib-tabs', role: 'tablist' }, TABS.map(([id, name]) => h('button', { class: 'lib-tab', type: 'button', role: 'tab', dataset: { tab: id }, onclick: () => { tab = id; detail = null; setPref('library.tab', id); render(); } }, name)));
  const scanBox = h('div', { class: 'lib-scan', hidden: true });
  const head = h('header', { class: 'v-head' },
    iconButton('chevronLeft', 'Back to now playing', () => setView('now'), { cls: 'v-back' }),
    h('h1', { class: 'v-title' }, 'Library'), tabs,
    h('div', { class: 'lib-tools' }, search, scanBox,
      iconButton('refresh', 'Rescan folders', () => rescanLibrary(), { cls: 'lib-rescan' })));
  const body = h('div', { class: 'v-body lib-body' });
  root.append(head, body);

  async function load() {
    const [albums, artists, tracks, playlists, folders] = await Promise.all([api.albums(), api.artists(), api.tracks(), api.playlists(), api.libraryFolders()]);
    data = { albums, artists, tracks, playlists, folders, byPath: new Map(tracks.map((t) => [t.path, t])) };
    render();
  }

  function coverTile(path, cls = 'lib-cover') {
    const img = h('img', { class: 'cover-img', alt: '', loading: 'lazy', draggable: 'false' });
    const el = h('div', { class: cls }, h('div', { class: 'cover-ph' }, icon('note', 32)), img);
    img.onload = () => el.classList.add('has-cover');
    img.src = coverUrl(path);
    return el;
  }

  function trackRow(t, i, list, opts = {}) {
    const playing = state.trackPath === t.path;
    const r = h('div', { class: `t-row${playing ? ' current' : ''}`, role: 'listitem', ondblclick: () => playPaths(list.map((x) => x.path), i) },
      h('span', { class: 't-no' }, playing ? icon('play', 12) : String(opts.number ?? i + 1)),
      h('span', { class: 't-title' }, t.title), h('span', { class: 't-artist' }, t.artist), h('span', { class: 't-album' }, t.album), h('span', { class: 't-dur' }, fmtTime(t.duration_secs)));
    r.addEventListener('contextmenu', async (ev) => {
      ev.preventDefault();
      showMenu(await trackMenuItems([t.path], opts.menuExtra?.(t, i) ?? []), ev.clientX, ev.clientY);
    });
    return r;
  }

  function trackTable(list, opts = {}) {
    const header = h('div', { class: 't-row t-head' }, h('span', { class: 't-no' }, '#'), h('span', { class: 't-title' }, 'Title'), h('span', { class: 't-artist' }, 'Artist'), h('span', { class: 't-album' }, 'Album'), h('span', { class: 't-dur' }, 'Time'));
    const vl = virtualList({ rowHeight: 40, cls: 't-list', render: (i) => trackRow(list[i], i, list, opts) });
    const wrap = h('div', { class: 't-table' }, header, vl);
    requestAnimationFrame(() => vl.setCount(list.length));
    return wrap;
  }

  const sum = (ts) => ts.reduce((s, t) => s + (t.duration_secs ?? 0), 0);

  function actionsFor(paths) {
    return h('div', { class: 'detail-actions' },
      button('Play', () => playPaths(paths, 0), { cls: 'primary', iconName: 'play' }),
      button('Shuffle', async () => { await act(api.setShuffle(true)); playPaths(paths, Math.floor(Math.random() * paths.length)); }, { iconName: 'shuffle' }),
      button('Add to queue', () => queueAdd(paths), { iconName: 'queue' }));
  }

  function albumGrid(albums) {
    if (!albums.length) return emptyNote('No albums match.');
    const size = getPrefs().library.albumSize ?? 190;
    return h('div', { class: 'album-grid', style: { '--tile': `${size}px` } }, albums.map((a) => {
      const card = h('button', { class: 'album-card', type: 'button', onclick: () => { detail = { type: 'album', album: a }; render(); } },
        coverTile(a.tracks[0]), h('span', { class: 'ac-title' }, a.title), h('span', { class: 'ac-artist' }, a.artist + (a.year ? ` · ${a.year}` : '')));
      card.addEventListener('contextmenu', async (ev) => { ev.preventDefault(); showMenu(await trackMenuItems(a.tracks), ev.clientX, ev.clientY); });
      return card;
    }));
  }

  function albumDetail(a) {
    const tracks = a.tracks.map((p) => data.byPath.get(p)).filter(Boolean);
    return h('div', { class: 'detail' },
      button('All albums', () => { detail = null; render(); }, { iconName: 'chevronLeft', cls: 'crumb' }),
      h('div', { class: 'detail-head' }, coverTile(a.tracks[0], 'detail-cover'),
        h('div', { class: 'detail-text' }, h('h2', { class: 'detail-title' }, a.title), h('div', { class: 'detail-sub' }, a.artist),
          h('div', { class: 'detail-meta' }, [a.year, `${a.track_count} ${a.track_count === 1 ? 'track' : 'tracks'}`, fmtTime(a.duration_secs)].filter(Boolean).join(' · ')), actionsFor(a.tracks))),
      trackTable(tracks, { number: undefined }));
  }

  function artistsView() {
    const list = data.artists.filter((a) => !query || a.name.toLowerCase().includes(query.toLowerCase()));
    if (!list.length) return emptyNote('No artists match.');
    return h('div', { class: 'artist-list' }, list.map((a) => h('button', { class: 'artist-row', type: 'button', onclick: () => { detail = { type: 'artist', artist: a }; render(); } },
      h('span', { class: 'ar-name' }, a.name), h('span', { class: 'ar-meta' }, `${a.album_count} ${a.album_count === 1 ? 'album' : 'albums'}, ${a.track_count} ${a.track_count === 1 ? 'track' : 'tracks'}`))));
  }

  function emptyNote(text, action) {
    return h('div', { class: 'empty lib-empty' }, icon('record', 36), h('p', null, text), action);
  }

  function playlistsView() {
    const left = h('div', { class: 'pl-list' },
      h('div', { class: 'pl-actions' },
        button('New playlist', async () => { const n = await promptText('New playlist', { ok: 'Create', placeholder: 'Playlist name' }); if (n) { const id = await act(api.createPlaylist(n)); await reloadPlaylists(); if (id) detail = { type: 'playlist', id }; render(); } }, { iconName: 'plus' }),
        button('Import M3U', async () => { const f = await pickFile([{ name: 'Playlists', extensions: ['m3u', 'm3u8'] }]).catch(() => null); if (f) { const r = await act(api.importM3u(f)); if (r) { toast(`Imported ${r[1]} tracks`); await reloadPlaylists(); detail = { type: 'playlist', id: r[0] }; render(); } } })),
      data.playlists.length ? data.playlists.map((p) => h('button', { class: `pl-item${detail?.id === p.id ? ' current' : ''}`, type: 'button', onclick: () => { detail = { type: 'playlist', id: p.id }; render(); } }, h('span', { class: 'pl-name' }, p.name), h('span', { class: 'pl-count' }, String(p.track_count))))
        : h('p', { class: 'hint' }, 'No playlists yet.'));
    const right = h('div', { class: 'pl-detail' });
    if (detail?.type === 'playlist') playlistDetail(right, detail.id); else right.append(emptyNote('Choose a playlist, or make a new one.'));
    return h('div', { class: 'pl-layout' }, left, right);
  }

  async function reloadPlaylists() { data.playlists = await api.playlists(); emit('playlists'); }

  async function playlistDetail(host, id) {
    const p = await api.playlist(id);
    if (!p) { host.append(emptyNote('That playlist no longer exists.')); return; }
    const tracks = p.tracks.map((path) => data.byPath.get(path) ?? { path, title: path.split(/[\\/]/).pop(), artist: '', album: '', duration_secs: 0 });
    const reorder = async (from, to) => { await act(api.moveInPlaylist(id, from, to)); detail = { type: 'playlist', id }; render(); };
    clear(host).append(
      h('div', { class: 'detail-head' }, h('div', { class: 'detail-text' }, h('h2', { class: 'detail-title' }, p.name), h('div', { class: 'detail-meta' }, `${tracks.length} tracks · ${fmtTime(sum(tracks))}`),
        h('div', { class: 'detail-actions' }, tracks.length ? [button('Play', () => act(api.playPlaylist(id, 0)), { cls: 'primary', iconName: 'play' }), button('Add to queue', () => queueAdd(p.tracks), { iconName: 'queue' })] : null,
          button('Rename', async () => { const n = await promptText('Rename playlist', { value: p.name }); if (n) { await act(api.renamePlaylist(id, n)); await reloadPlaylists(); render(); } }),
          button('Export M3U', async () => { const d = await pickSave(`${p.name}.m3u8`, [{ name: 'Playlist', extensions: ['m3u8'] }]).catch(() => null); if (d) { await act(api.exportM3u(id, d)); toast('Playlist exported'); } }),
          button('Delete', async () => { if (await confirmDialog('Delete playlist?', `"${p.name}" will be removed. Your music files are not touched.`)) { await act(api.deletePlaylist(id)); detail = null; await reloadPlaylists(); render(); } }, { iconName: 'trash' })))),
      tracks.length ? trackTable(tracks, {
        menuExtra: (t, i) => [
          { label: 'Remove from playlist', onClick: async () => { await act(api.removeFromPlaylist(id, [i])); detail = { type: 'playlist', id }; reloadPlaylists().then(render); } },
          ...(i > 0 ? [{ label: 'Move up', onClick: () => reorder(i, i - 1) }] : []),
          ...(i < tracks.length - 1 ? [{ label: 'Move down', onClick: () => reorder(i, i + 1) }] : []), { sep: true }],
      }) : emptyNote('This playlist is empty. Right-click tracks anywhere and choose Add to playlist.'));
  }

  function foldersView() {
    const add = async () => {
      const f = await pickFolder().catch(() => null);
      if (!f) return;
      if (await act(api.addFolder(f)) !== undefined) { data.folders = await api.libraryFolders(); render(); rescanLibrary(); }
    };
    return h('div', { class: 'folders' },
      h('p', { class: 'hint' }, 'Redshift reads tags and cover art from these folders and everything inside them. Your files are never changed.'),
      data.folders.map((f) => h('div', { class: 'folder-row' }, icon('folder', 18), h('span', { class: 'folder-path' }, f),
        iconButton('trash', `Remove ${f}`, async () => { if (await act(api.removeFolder(f)) !== undefined) { data.folders = await api.libraryFolders(); await load(); } }, { size: 16 }))),
      h('div', { class: 'detail-actions' }, button('Add folder', add, { iconName: 'plus', cls: 'primary' }), button('Rescan now', () => rescanLibrary(), { iconName: 'refresh' })));
  }

  async function render() {
    tabs.querySelectorAll('.lib-tab').forEach((b) => b.setAttribute('aria-selected', String(b.dataset.tab === tab)));
    clear(body);
    if (!data.tracks.length && !state.scan?.running) {
      body.append(emptyNote('Your library is empty.', h('div', { class: 'detail-actions' }, button('Add a music folder', async () => { const f = await pickFolder().catch(() => null); if (f && (await act(api.addFolder(f))) !== undefined) { rescanLibrary(); } }, { cls: 'primary', iconName: 'folder' }))));
      if (tab === 'folders') body.replaceChildren(foldersView());
      return;
    }
    if (query && tab !== 'artists' && tab !== 'folders' && tab !== 'playlists') {
      const results = await api.search(query);
      body.append(results.length ? h('div', { class: 'detail' }, h('div', { class: 'detail-meta' }, `${results.length} results for "${query}"`), trackTable(results)) : emptyNote(`Nothing matches "${query}".`));
      return;
    }
    if (detail?.type === 'album') body.append(albumDetail(detail.album));
    else if (detail?.type === 'artist') {
      const albums = data.albums.filter((a) => a.artist === detail.artist.name);
      body.append(h('div', { class: 'detail' }, button('All artists', () => { detail = null; render(); }, { iconName: 'chevronLeft', cls: 'crumb' }), h('h2', { class: 'detail-title' }, detail.artist.name), albumGrid(albums)));
    } else if (tab === 'albums') body.append(albumGrid(data.albums));
    else if (tab === 'artists') body.append(artistsView());
    else if (tab === 'tracks') body.append(trackTable(data.tracks));
    else if (tab === 'playlists') body.append(playlistsView());
    else if (tab === 'folders') body.append(foldersView());
  }

  const renderScan = () => {
    const s = state.scan;
    scanBox.hidden = !s?.running;
    if (!s?.running) return;
    const pct = s.total ? Math.round((s.done / s.total) * 100) : 0;
    scanBox.replaceChildren(h('span', { class: 'scan-text' }, s.phase === 'reading' ? `Reading tags ${s.done} / ${s.total}` : 'Looking for music…'), h('span', { class: 'scan-bar' }, h('i', { style: { width: `${pct}%` } })));
  };
  on('scan', renderScan);
  on('library', () => load().catch(console.error));
  on('playlists', () => api.playlists().then((p) => { data.playlists = p; if (state.view === 'library' && tab === 'playlists') render(); }));
  on('track', () => { if (state.view === 'library' && (tab === 'tracks' || detail)) render(); });
  load().catch((e) => { console.error(e); body.append(emptyNote('The library could not be loaded.')); });
  return { reload: load };
}
