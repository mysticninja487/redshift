// Server: browse and play the music on your own Navidrome / Subsonic server. Songs play as the original
// files, so a lossless library stays lossless; each one is marked Lossless or Lossy.

import { api, coverUrl, COVER } from '../api.js';
import { on } from '../bus.js';
import { act, playPaths, queueAdd, setView, shufflePlay, toggleFavourite } from '../controller.js';
import { getPrefs } from '../prefs.js';
import { askToGoOnline, online } from '../online.js';
import { isServerPath, state } from '../state.js';
import { qualityLine, qualityOf, refreshServer, server, serverCoverPath, serverOn, serverSongPath } from '../server.js';
import { debounce, fmtTime, h, plural, toast } from '../util.js';
import { button, iconButton, select } from '../ui/controls.js';
import { icon } from '../ui/icons.js';
import { showMenu } from '../ui/menu.js';
import { trackMenuItems } from '../ui/trackmenu.js';
import { virtualList } from '../ui/vlist.js';

const TABS = [['albums', 'Albums'], ['artists', 'Artists'], ['playlists', 'Playlists']];
const LISTS = [['newest', 'Recently added'], ['recent', 'Recently played'], ['frequent', 'Most played'], ['random', 'Random'], ['alphabeticalByName', 'A to Z'], ['starred', 'Favourites on the server']];
const PAGE = 60;

export function mountServer(root) {
  let tab = 'albums', kind = 'newest';
  let query = '';
  let detail = null; // { type: 'album' | 'artist' | 'playlist', id, data }
  let albums = [], more = false;
  let artists = null, playlists = null, found = null;
  let status = 'idle', error = ''; // idle | loading | done | error
  let token = 0;

  const search = h('input', { class: 'text-input lib-search', type: 'search', placeholder: 'Search your server', 'aria-label': 'Search your server',
    oninput: debounce((e) => { query = e.target.value.trim(); detail = null; run(); }, 350) });
  const tabs = h('div', { class: 'lib-tabs', role: 'tablist' }, TABS.map(([id, name]) => h('button', { class: 'lib-tab', type: 'button', role: 'tab', dataset: { tab: id },
    onclick: () => { tab = id; detail = null; run(); } }, name)));
  const body = h('div', { class: 'v-body lib-body sv-body' });
  const chip = h('span', { class: 'sv-chip', hidden: true });
  root.append(
    h('header', { class: 'v-head' }, h('h1', { class: 'v-title' }, 'Server'), chip, h('div', { class: 'lib-tools' }, search)),
    tabs, body);

  const ready = () => serverOn() && server.settings?.configured;
  const msg = (e) => String(e?.message ?? e);

  // ---- loading ----

  async function run() {
    paintTabs();
    if (!ready()) { render(); return; }
    const mine = ++token;
    status = 'loading'; error = ''; render();
    try {
      if (query) found = await api.serverSearch(query);
      else if (detail) { /* the detail was loaded when it was opened */ }
      else if (tab === 'albums') { albums = await api.serverAlbums(kind, 0, PAGE); more = albums.length === PAGE && kind !== 'random'; }
      else if (tab === 'artists') artists = await api.serverArtists();
      else playlists = await api.serverPlaylists();
    } catch (e) {
      if (mine !== token) return;
      status = 'error'; error = msg(e); render(); return;
    }
    if (mine !== token) return;
    status = 'done'; render();
  }

  async function loadMore() {
    try {
      const next = await api.serverAlbums(kind, albums.length, PAGE);
      albums = [...albums, ...next]; more = next.length === PAGE;
    } catch (e) { toast(msg(e), 'error'); }
    render();
  }

  async function open(type, id, loader) {
    const mine = ++token;
    status = 'loading'; error = ''; detail = { type, id, data: null }; render();
    try {
      const data = await loader(id);
      if (mine !== token) return;
      detail = { type, id, data }; status = 'done';
    } catch (e) {
      if (mine !== token) return;
      status = 'error'; error = msg(e);
    }
    render();
  }
  const openAlbum = (id) => open('album', id, api.serverAlbum);
  const openArtist = (id) => open('artist', id, api.serverArtist);
  const openPlaylist = (id) => open('playlist', id, api.serverPlaylist);

  // ---- pieces ----

  const pathsOf = (songs) => songs.map((s) => serverSongPath(s.id));

  function cover(coverId, cls = 'lib-cover', px = COVER.card) {
    const img = h('img', { class: 'cover-img', alt: '', loading: 'lazy', draggable: 'false' });
    const el = h('div', { class: cls }, h('div', { class: 'cover-ph' }, icon('note', 32)), img);
    if (coverId) { img.onload = () => el.classList.add('has-cover'); img.src = coverUrl(serverCoverPath(coverId), px); }
    return el;
  }

  const qualityChip = (s) => h('span', { class: `rd-chip ${s.lossless ? 'is-lossless' : 'is-lossy'}` }, s.lossless ? 'Lossless' : 'Lossy');

  function actions(songs) {
    const paths = pathsOf(songs);
    return h('div', { class: 'detail-actions' },
      button('Play', () => playPaths(paths, 0), { cls: 'primary', iconName: 'play' }),
      button('Shuffle', () => shufflePlay(paths), { iconName: 'shuffle' }),
      button('Add to queue', () => queueAdd(paths), { iconName: 'queue' }));
  }

  function playRow(songs, i) {
    if ((getPrefs().library.playMode ?? 'single') === 'list' && songs.length > 1) playPaths(pathsOf(songs), i);
    else playPaths([serverSongPath(songs[i].id)], 0);
  }

  function songRow(songs, i) {
    const s = songs[i], path = serverSongPath(s.id), playing = state.trackPath === path;
    const fav = state.favs.has(path);
    const r = h('div', { class: `t-row sv-row${playing ? ' current' : ''}`, role: 'listitem', tabindex: '-1', ondblclick: () => playRow(songs, i), onkeydown: (e) => { if (e.key === 'Enter') playRow(songs, i); } },
      h('span', { class: 't-no' }, h('span', { class: 't-num' }, playing ? icon('play', 12) : String(s.track ?? i + 1)),
        h('button', { class: 'row-play', type: 'button', title: 'Play', 'aria-label': `Play ${s.title}`, onclick: (e) => { e.stopPropagation(); playRow(songs, i); } }, icon('play', 12))),
      h('span', { class: 't-title', translate: 'no' }, s.title), h('span', { class: 't-artist', translate: 'no' }, s.artist), h('span', { class: 't-album', translate: 'no' }, s.album),
      h('span', { class: 'sv-quality', title: qualityLine(s) }, qualityChip(s), h('span', { class: 'sv-fmt', translate: 'no' }, qualityLine(s))),
      h('button', { class: `fav-btn${fav ? ' on' : ''}`, type: 'button', title: 'Favourite', 'aria-label': 'Favourite', 'aria-pressed': String(fav), onclick: (e) => { e.stopPropagation(); toggleFavourite([path]); } }, icon('heart', 15)),
      h('span', { class: 't-dur' }, fmtTime(s.duration_secs)));
    r.addEventListener('contextmenu', async (ev) => {
      ev.preventDefault();
      const extra = songs.length > 1 ? [{ label: 'Play from here', onClick: () => playPaths(pathsOf(songs), i) }, { sep: true }] : [];
      showMenu(await trackMenuItems([path], extra), ev.clientX, ev.clientY);
    });
    return r;
  }

  function songTable(songs) {
    const head = h('div', { class: 't-row sv-row t-head' }, h('span', { class: 't-no' }, '#'), h('span', { class: 't-title' }, 'Title'), h('span', { class: 't-artist' }, 'Artist'), h('span', { class: 't-album' }, 'Album'),
      h('span', { class: 'sv-quality' }, 'Quality'), h('span', { class: 't-fav' }), h('span', { class: 't-dur' }, 'Time'));
    const vl = virtualList({ rowHeight: 40, cls: 't-list', render: (i) => songRow(songs, i) });
    requestAnimationFrame(() => vl.setCount(songs.length));
    return h('div', { class: 't-table' }, head, vl);
  }

  function albumCard(a) {
    const c = cover(a.cover_art);
    const card = h('div', { class: 'album-card', role: 'button', tabindex: '0', onclick: () => openAlbum(a.id), onkeydown: (e) => { if (e.key === 'Enter') openAlbum(a.id); } },
      c, h('span', { class: 'ac-title', translate: 'no' }, a.name), h('span', { class: 'ac-artist', translate: 'no' }, a.artist + (a.year ? ` · ${a.year}` : '')));
    card.addEventListener('contextmenu', async (ev) => {
      ev.preventDefault();
      try { const v = await api.serverAlbum(a.id); showMenu(await trackMenuItems(pathsOf(v.songs)), ev.clientX, ev.clientY); } catch (e) { toast(msg(e), 'error'); }
    });
    return card;
  }

  const albumGrid = (list) => h('div', { class: 'album-grid', style: { '--tile': `${getPrefs().library.albumSize ?? 190}px` } }, list.map(albumCard));
  const note = (text, cls = '') => h('p', { class: `rd-note ${cls}`.trim() }, text);

  function artistRow(a) {
    return h('div', { class: 'artist-row', role: 'button', tabindex: '0', onclick: () => openArtist(a.id), onkeydown: (e) => { if (e.key === 'Enter') openArtist(a.id); } },
      h('span', { class: 'ar-name', translate: 'no' }, a.name), h('span', { class: 'ar-meta' }, plural(a.album_count, 'album')));
  }

  // ---- screens ----

  const back = (label, fn) => button(label, fn, { iconName: 'chevronLeft', cls: 'crumb' });

  function albumDetail(v) {
    const { album, songs } = v, q = qualityOf(songs);
    return h('div', { class: 'detail' },
      back('Back', () => { detail = null; render(); }),
      h('div', { class: 'detail-head' }, cover(album.cover_art, 'detail-cover', 480),
        h('div', { class: 'detail-text' }, h('h2', { class: 'detail-title', translate: 'no' }, album.name), h('div', { class: 'detail-sub', translate: 'no' }, album.artist),
          h('div', { class: 'detail-meta' }, [album.year, plural(songs.length, 'song'), fmtTime(songs.reduce((t, s) => t + s.duration_secs, 0))].filter(Boolean).join(' · ')),
          q ? h('div', { class: 'sv-q-line' }, h('span', { class: `rd-chip ${q === 'lossless' ? 'is-lossless' : 'is-lossy'}` }, q === 'lossless' ? 'Lossless' : q === 'lossy' ? 'Lossy' : 'Mixed quality')) : null,
          actions(songs))),
      songTable(songs));
  }

  function artistDetail(v) {
    return h('div', { class: 'detail' }, back('Back', () => { detail = null; render(); }),
      h('div', { class: 'detail-text' }, h('h2', { class: 'detail-title', translate: 'no' }, v.artist.name), h('div', { class: 'detail-meta' }, plural(v.albums.length, 'album'))),
      albumGrid(v.albums));
  }

  function playlistDetail(v) {
    const { playlist, songs } = v;
    return h('div', { class: 'detail' }, back('Back', () => { detail = null; render(); }),
      h('div', { class: 'list-head' },
        h('div', null, h('h2', { class: 'detail-title small', translate: 'no' }, playlist.name), h('div', { class: 'detail-meta' }, `${plural(songs.length, 'song')} · ${fmtTime(songs.reduce((t, s) => t + s.duration_secs, 0))}`)),
        h('div', { class: 'list-head-tools' }, songs.length ? actions(songs) : null)),
      songs.length ? songTable(songs) : note('This playlist is empty.'));
  }

  function searchView() {
    const f = found ?? { artists: [], albums: [], songs: [] };
    if (!f.artists.length && !f.albums.length && !f.songs.length) return note('Nothing on your server matches that.');
    return h('div', { class: 'detail' },
      f.artists.length ? h('section', { class: 'rd-sec' }, h('h3', { class: 'st-h3' }, 'Artists'), h('div', { class: 'artist-list' }, f.artists.map(artistRow))) : null,
      f.albums.length ? h('section', { class: 'rd-sec' }, h('h3', { class: 'st-h3' }, 'Albums'), albumGrid(f.albums)) : null,
      f.songs.length ? h('section', { class: 'rd-sec' }, h('h3', { class: 'st-h3' }, 'Songs'), songTable(f.songs)) : null);
  }

  function albumsView() {
    const picker = select({ label: 'Show albums', value: kind, options: LISTS.map(([value, label]) => ({ value, label })), onChange: (v) => { kind = v; run(); } });
    return h('div', { class: 'detail' },
      h('div', { class: 'list-head' }, picker, kind === 'random' ? button('Shuffle the list', () => run(), { iconName: 'refresh' }) : null),
      albums.length ? albumGrid(albums) : note(kind === 'starred' ? 'No albums are starred on your server.' : kind === 'recent' || kind === 'frequent' ? 'Nothing has been played from this server yet.' : 'No albums found.'),
      more ? button('Show more', loadMore, { cls: 'sv-more' }) : null);
  }

  function gate() {
    const o = online();
    if (!o.enabled) return h('div', { class: 'rd-gate' }, icon('globe', 28), h('h3', null, 'Your server needs the network'),
      h('p', null, 'Redshift stays offline unless you allow it. Browsing and playing your server sends requests to the address you set up, and nowhere else.'),
      button('Go online', async () => { if (await askToGoOnline()) { await refreshServer(); run(); } }, { cls: 'primary' }));
    if (o.server === false) return h('div', { class: 'rd-gate' }, icon('server', 28), h('h3', null, 'Your server is switched off'),
      h('p', null, 'Turn on Play from my server in Settings > Online to use it.'), button('Open settings', () => setView('settings'), { cls: 'primary' }));
    return h('div', { class: 'rd-gate' }, icon('server', 28), h('h3', null, 'Connect your music server'),
      h('p', null, 'Play the music on your own Navidrome or any Subsonic-compatible server, as the original files, so lossless stays lossless. Add its address and login in Settings > Online.'),
      button('Open settings', () => setView('settings'), { cls: 'primary' }));
  }

  function render() {
    paintTabs();
    chip.hidden = !ready();
    if (!chip.hidden) { const enc = server.settings.encrypted; chip.textContent = enc ? 'Encrypted' : 'Not encrypted'; chip.className = `sv-chip ${enc ? 'is-ok' : 'is-warn'}`; chip.title = enc ? 'This server’s address is https.' : 'This address is plain http, so your login token and your music cross the network unencrypted.'; }
    if (!ready()) { body.replaceChildren(gate()); return; }
    if (status === 'loading') { body.replaceChildren(note('Loading…')); return; }
    if (status === 'error') { body.replaceChildren(h('div', { class: 'rd-note is-error' }, h('p', null, `Could not reach your server: ${error}`), button('Try again', () => { if (detail && !detail.data) { const d = detail; detail = null; ({ album: openAlbum, artist: openArtist, playlist: openPlaylist })[d.type](d.id); } else run(); }))); return; }
    if (detail?.data) { body.replaceChildren(detail.type === 'album' ? albumDetail(detail.data) : detail.type === 'artist' ? artistDetail(detail.data) : playlistDetail(detail.data)); return; }
    if (query) { body.replaceChildren(searchView()); return; }
    if (tab === 'albums') body.replaceChildren(albumsView());
    else if (tab === 'artists') body.replaceChildren(artists?.length ? h('div', { class: 'artist-list' }, artists.map(artistRow)) : note('No artists found.'));
    else body.replaceChildren(playlists?.length ? h('div', { class: 'artist-list' }, playlists.map((p) => h('div', { class: 'artist-row', role: 'button', tabindex: '0', onclick: () => openPlaylist(p.id), onkeydown: (e) => { if (e.key === 'Enter') openPlaylist(p.id); } },
      h('span', { class: 'ar-name', translate: 'no' }, p.name), h('span', { class: 'ar-meta' }, plural(p.song_count, 'song'))))) : note('No playlists on your server.'));
  }

  function paintTabs() {
    tabs.hidden = !!query;
    for (const b of tabs.children) b.setAttribute('aria-selected', String(b.dataset.tab === tab && !detail));
  }

  on('server', () => { render(); if (ready() && status === 'idle') run(); });
  on('settings', () => { render(); if (ready() && status === 'idle') run(); });
  on('player', debounce(() => { if (detail?.data || found) render(); }, 400));
  on('favs', () => { if (detail?.data || found) render(); });
  const start = async () => { await refreshServer(); if (ready()) run(); else render(); };
  render();
  start();
  return { reload: () => { render(); if (ready() && status === 'idle') run(); } };
}
