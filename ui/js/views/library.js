// Browsing the music: albums, artists, songs, favourites, play history, playlists and folders.

import { api, COVER, coverUrl, pickFile, pickFolder, pickSave } from '../api.js';
import { on, emit } from '../bus.js';
import { act, playPaths, queueAdd, refreshStats, rescanLibrary, setView, shufflePlay, toggleFavourite } from '../controller.js';
import { activeRules, applyFilter, detailLine, formatsIn, playlistsNeeded, RESULT_SORTS, sortResults } from '../filters.js';
import { baseName, crumbs, folderContents, norm, tracksUnder } from '../folders.js';
import { getPrefs, setPref } from '../prefs.js';
import { state } from '../state.js';
import { ago, clear, debounce, fmtTime, h, plural, toast } from '../util.js';
import { button, iconButton, select } from '../ui/controls.js';
import { confirmDialog, promptText } from '../ui/dialog.js';
import { importOnlinePlaylist } from '../ui/playlistimport.js';
import { icon } from '../ui/icons.js';
import { filterPanel } from '../ui/libfilter.js';
import { showMenu } from '../ui/menu.js';
import { trackMenuItems } from '../ui/trackmenu.js';
import { virtualList } from '../ui/vlist.js';

const TABS = [
  ['albums', 'Albums'], ['artists', 'Artists'], ['tracks', 'All songs'], ['favourites', 'Favourites'],
  ['recent-played', 'Recently played'], ['most-played', 'Most played'], ['recent-added', 'Recently added'], ['not-played', 'Not played'],
  ['playlists', 'Playlists'], ['folders', 'Folders'],
];
const SORTS = [['default', 'Artist, album'], ['title', 'Title'], ['artist', 'Artist'], ['album', 'Album'], ['year', 'Year'], ['length', 'Length']];
const LIST_LIMIT = 200;

const cmp = (a, b) => String(a).localeCompare(String(b), undefined, { numeric: true, sensitivity: 'base' });

export function mountLibrary(root) {
  let tab = getPrefs().library.tab ?? 'albums';
  if (!TABS.some(([id]) => id === tab)) tab = 'albums';
  let data = { albums: [], artists: [], tracks: [], byPath: new Map(), playlists: [], folders: [] };
  let detail = null; // { type: 'album' | 'artist' | 'playlist', ... } shown over the tab
  let query = '';
  let folderPath = null; // normalised path of the folder being browsed; null lists the library folders
  let mostMode = 'tracks';
  let renderToken = 0;
  const picked = new Set();   // paths of the selected songs; kept while the page is redrawn, dropped when you go somewhere else
  const plSets = new Map();   // playlist id -> Set of its paths, for the playlist filters

  const filterUI = filterPanel({ onChange: () => { detail = null; updateBadge(); render(); } });
  const filter = filterUI.filter;
  const search = h('input', { class: 'text-input lib-search', type: 'search', placeholder: 'Search your library', 'aria-label': 'Search library', oninput: debounce((e) => { query = filter.text = e.target.value.trim(); detail = null; updateBadge(); render(); }, 160) });
  const badge = h('span', { class: 'lf-badge', hidden: true });
  const filterBtn = h('button', { class: 'btn lib-filter-btn', type: 'button', 'aria-expanded': 'false', 'aria-controls': 'lib-filters', title: 'Filter by any tag, file property, play history or playlist', onclick: () => toggleFilters() }, icon('sliders', 16), 'Filters', badge);
  function updateBadge() {
    const n = activeRules(filter).length;
    badge.hidden = !n; badge.textContent = String(n);
    filterBtn.classList.toggle('active', n > 0);
  }
  function toggleFilters(force) {
    const open = force ?? !filterUI.visible;
    filterUI.show(open);
    filterBtn.setAttribute('aria-expanded', String(open));
    filterBtn.classList.toggle('open', open);
  }
  filterUI.el.id = 'lib-filters';
  const tabs = h('div', { class: 'lib-tabs', role: 'tablist' }, TABS.map(([id, name]) => h('button', { class: 'lib-tab', type: 'button', role: 'tab', dataset: { tab: id }, onclick: () => { tab = id; detail = null; setPref('library.tab', id); render(); } }, name)));
  const scanBox = h('div', { class: 'lib-scan', hidden: true });
  const head = h('header', { class: 'v-head' },
    iconButton('chevronLeft', 'Back to now playing', () => setView('now'), { cls: 'v-back' }),
    h('h1', { class: 'v-title' }, 'Library'),
    h('div', { class: 'lib-tools' }, search, filterBtn, scanBox, iconButton('refresh', 'Rescan folders', () => rescanLibrary(), { cls: 'lib-rescan' })));
  const body = h('div', { class: 'v-body lib-body' });
  root.append(head, tabs, filterUI.el, body);

  async function load() {
    const [albums, artists, tracks, playlists, folders] = await Promise.all([api.albums(), api.artists(), api.tracks(), api.playlists(), api.libraryFolders()]);
    data = { albums, artists, tracks, playlists, folders, byPath: new Map(tracks.map((t) => [t.path, t])) };
    filterUI.setLists({ formats: formatsIn(tracks), playlists });
    render();
  }

  // ---- building blocks ----

  function coverTile(path, cls = 'lib-cover') {
    const img = h('img', { class: 'cover-img', alt: '', loading: 'lazy', draggable: 'false' });
    const el = h('div', { class: cls }, h('div', { class: 'cover-ph' }, icon('note', 32)), img);
    img.onload = () => el.classList.add('has-cover');
    img.src = coverUrl(path, cls === 'detail-cover' ? 480 : COVER.card);
    return el;
  }

  const favBtn = (path) => {
    const b = h('button', { class: `fav-btn${state.favs.has(path) ? ' on' : ''}`, type: 'button', dataset: { path }, title: 'Favourite', 'aria-label': 'Favourite', 'aria-pressed': String(state.favs.has(path)), onclick: (e) => { e.stopPropagation(); toggleFavourite([path]); } }, icon('heart', 15));
    return b;
  };

  /** Plays the row's track: just that track, or the list from there, as the setting says. */
  function playRow(t, i, list) {
    const playable = list.filter((x) => !x.missing);
    if ((getPrefs().library.playMode ?? 'single') === 'list' && playable.length > 1) {
      playPaths(playable.map((x) => x.path), Math.max(0, playable.indexOf(t)));
    } else playPaths([t.path], 0);
  }

  function trackRow(t, i, list, opts = {}) {
    const playing = state.trackPath === t.path;
    const gone = opts.missing?.has(t.path) || t.missing;
    const sel = opts.sel;
    const isPicked = !!sel?.has(t.path);
    const r = h('div', { class: `t-row${playing ? ' current' : ''}${gone ? ' missing' : ''}${opts.extra ? ` has-extra${opts.extra.wide ? ' wide' : ''}` : ''}${sel ? ' sel' : ''}${isPicked ? ' selected' : ''}`, role: 'listitem', tabindex: '-1', 'aria-selected': sel ? String(isPicked) : undefined, title: gone ? 'This file was not found on this PC' : undefined,
      onclick: (e) => { if (sel && !gone && !e.target.closest('button, input, a')) sel.click(i, e); },
      ondblclick: (e) => { if (!gone && !e.target.closest('button, input')) playRow(t, i, list); }, onkeydown: (e) => { if (e.key === 'Enter' && !gone) playRow(t, i, list); } },
      sel && h('span', { class: 't-check' }, h('input', { type: 'checkbox', checked: isPicked, disabled: gone, tabindex: '-1', 'aria-label': `Select ${t.title}`, onclick: (e) => { e.stopPropagation(); sel.toggle(i, e.shiftKey); } })),
      h('span', { class: 't-no' }, h('span', { class: 't-num' }, playing ? icon('play', 12) : String(i + 1)),
        gone ? null : h('button', { class: 'row-play', type: 'button', title: 'Play', 'aria-label': `Play ${t.title}`, onclick: (e) => { e.stopPropagation(); playRow(t, i, list); } }, icon('play', 12))),
      h('span', { class: 't-title' }, t.title), h('span', { class: 't-artist' }, t.artist), h('span', { class: 't-album' }, t.album),
      opts.extra && h('span', { class: 't-extra' }, opts.extra.get(t)),
      favBtn(t.path), h('span', { class: 't-dur' }, fmtTime(t.duration_secs)));
    r.addEventListener('contextmenu', async (ev) => {
      ev.preventDefault();
      // Right-clicking a song that is not selected selects just that one; on a selected song it acts on the whole selection.
      if (sel && !gone && !sel.has(t.path)) sel.only(i);
      const group = sel && !gone && sel.size() > 1 ? sel.paths() : null;
      const extra = [];
      if (!group && !gone && list.length > 1) extra.push({ label: 'Play from here', onClick: () => playPaths(list.filter((x) => !x.missing).map((x) => x.path), Math.max(0, list.filter((x) => !x.missing).indexOf(t))) });
      if (extra.length) extra.push({ sep: true });
      showMenu(await trackMenuItems(group ?? (gone ? [] : [t.path]), [...extra, ...(opts.menuExtra?.(t, i, group ? sel.indexes() : [i]) ?? [])]), ev.clientX, ev.clientY);
    });
    return r;
  }

  /** What to do with the songs picked in a list: the bar above it. */
  async function playlistMenu(paths, anchor) {
    const items = await trackMenuItems(paths);
    const sub = items.find((x) => x.label === 'Add to playlist')?.submenu ?? [];
    const r = anchor.getBoundingClientRect();
    showMenu(sub, r.left, r.bottom + 4);
  }

  function trackTable(list, opts = {}) {
    const selectable = opts.select !== false;
    let anchor = -1, vl = null;
    const have = new Set(list.map((t) => t.path));
    for (const p of [...picked]) if (!have.has(p)) picked.delete(p);
    const pickedList = () => list.filter((t) => picked.has(t.path));
    const bar = h('div', { class: 'sel-bar', hidden: true, role: 'toolbar', 'aria-label': 'Selected songs' });
    const headCheck = h('input', { type: 'checkbox', 'aria-label': 'Select all songs in this list', tabindex: '-1', onclick: () => { if (picked.size === list.filter((t) => !t.missing).length) picked.clear(); else list.forEach((t) => { if (!t.missing) picked.add(t.path); }); changed(); } });

    const paintBar = () => {
      const chosen = pickedList();
      const paths = chosen.map((t) => t.path);
      bar.hidden = !paths.length;
      bar.parentElement?.classList.toggle('picking', paths.length > 0);
      const all = list.filter((t) => !t.missing).length;
      headCheck.checked = paths.length > 0 && paths.length === all;
      headCheck.indeterminate = paths.length > 0 && paths.length < all;
      if (!paths.length) return;
      const allFav = paths.every((p) => state.favs.has(p));
      clear(bar).append(
        h('span', { class: 'sel-count' }, `${plural(paths.length, 'song')} selected · ${fmtTime(sum(chosen))}`),
        button('Play', () => playPaths(paths, 0), { cls: 'primary', iconName: 'play' }),
        button('Shuffle', () => shufflePlay(paths), { iconName: 'shuffle' }),
        button('Add to queue', () => queueAdd(paths), { iconName: 'queue' }),
        button('Add to playlist', (e) => playlistMenu(paths, e.currentTarget), { iconName: 'playlist' }),
        button(allFav ? 'Unfavourite' : 'Favourite', () => toggleFavourite(paths), { iconName: 'heart' }),
        button('More', async (e) => { const r = e.currentTarget.getBoundingClientRect(); showMenu(await trackMenuItems(paths), r.left, r.bottom + 4); }, { iconName: 'more' }),
        h('span', { class: 'sel-spacer' }),
        iconButton('close', 'Clear selection (Esc)', () => { picked.clear(); changed(); }, { size: 16 }));
    };
    let wrap = null;
    // Redrawing the rows drops the keyboard focus; keep it in the table so Ctrl+A and Esc still work.
    const changed = () => {
      const keep = !wrap || document.activeElement === document.body || wrap.contains(document.activeElement);
      vl?.refresh(); paintBar();
      if (keep) wrap?.focus({ preventScroll: true });
    };
    const range = (from, to, add) => { for (let k = Math.min(from, to); k <= Math.max(from, to); k++) if (!list[k].missing) add(list[k].path); };
    const sel = selectable ? {
      has: (p) => picked.has(p),
      size: () => picked.size,
      paths: () => pickedList().map((t) => t.path),
      indexes: () => list.flatMap((t, k) => (picked.has(t.path) ? [k] : [])),
      only(i) { picked.clear(); picked.add(list[i].path); anchor = i; changed(); },
      // A click on the row: this one alone, Ctrl adds or removes it, Shift takes everything from the last one clicked.
      click(i, e) {
        if (e.shiftKey && anchor >= 0) { if (!(e.ctrlKey || e.metaKey)) picked.clear(); range(anchor, i, (p) => picked.add(p)); }
        else if (e.ctrlKey || e.metaKey) { const p = list[i].path; picked.has(p) ? picked.delete(p) : picked.add(p); anchor = i; }
        else { picked.clear(); picked.add(list[i].path); anchor = i; }
        changed();
      },
      // The tick box: adds or removes this one; with Shift it adds the whole stretch.
      toggle(i, shift) {
        const p = list[i].path;
        if (shift && anchor >= 0) range(anchor, i, (q) => picked.add(q));
        else { picked.has(p) ? picked.delete(p) : picked.add(p); anchor = i; }
        changed();
      },
    } : null;

    const o = { ...opts, sel };
    const header = h('div', { class: `t-row t-head${opts.extra ? ` has-extra${opts.extra.wide ? ' wide' : ''}` : ''}${sel ? ' sel' : ''}` }, sel && h('span', { class: 't-check' }, headCheck), h('span', { class: 't-no' }, '#'), h('span', { class: 't-title' }, 'Title'), h('span', { class: 't-artist' }, 'Artist'), h('span', { class: 't-album' }, 'Album'),
      opts.extra && h('span', { class: 't-extra' }, opts.extra.head), h('span', { class: 't-fav' }), h('span', { class: 't-dur' }, 'Time'));
    vl = virtualList({ rowHeight: 40, cls: 't-list', render: (i) => trackRow(list[i], i, list, o) });
    wrap = h('div', { class: 't-table', tabindex: '-1', onkeydown: (e) => {
      if (!sel || e.target.closest('input[type="text"], input[type="search"], textarea')) return;
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'a') { e.preventDefault(); list.forEach((t) => { if (!t.missing) picked.add(t.path); }); changed(); }
      else if (e.key === 'Escape' && picked.size) { e.preventDefault(); e.stopPropagation(); picked.clear(); changed(); }
    } }, bar, header, vl);
    requestAnimationFrame(() => { vl.setCount(list.length); paintBar(); });
    return wrap;
  }

  const sum = (ts) => ts.reduce((s, t) => s + (t.duration_secs ?? 0), 0);

  /** Play, Shuffle and Add to queue for a set of paths. */
  function actionsFor(paths) {
    return h('div', { class: 'detail-actions' },
      button('Play', () => playPaths(paths, 0), { cls: 'primary', iconName: 'play' }),
      button('Shuffle', () => shufflePlay(paths), { iconName: 'shuffle' }),
      button('Add to queue', () => queueAdd(paths), { iconName: 'queue' }));
  }

  /** The small play + shuffle pair shown on hover over covers and rows. `getPaths` is called on click. */
  function quickPlay(getPaths, cls = '') {
    const run = (fn) => async (e) => { e.stopPropagation(); e.preventDefault(); const p = await getPaths(); if (p.length) fn(p); };
    return h('span', { class: `quick-play ${cls}`.trim() },
      iconButton('play', 'Play', run((p) => playPaths(p, 0)), { size: 15, cls: 'qp-btn' }),
      iconButton('shuffle', 'Shuffle play', run((p) => shufflePlay(p)), { size: 15, cls: 'qp-btn' }));
  }

  function emptyNote(text, action) {
    return h('div', { class: 'empty lib-empty' }, icon('record', 36), h('p', null, text), action);
  }

  /** A titled list of tracks with Play / Shuffle at the top: the shape of the smart tabs. */
  function listPage(title, list, opts = {}) {
    const paths = list.filter((t) => !t.missing).map((t) => t.path);
    return h('div', { class: 'detail' },
      h('div', { class: 'list-head' },
        h('div', null, title && h('h2', { class: 'detail-title small' }, title), h('div', { class: 'detail-meta' }, `${plural(list.length, 'track')} · ${fmtTime(sum(list))}${opts.note ? ` · ${opts.note}` : ''}`)),
        h('div', { class: 'list-head-tools' }, opts.tools, paths.length ? actionsFor(paths) : null)),
      trackTable(list, opts));
  }

  const playsOf = (path) => state.stats.get(path)?.plays ?? 0;
  const artistOf = (t) => t.album_artist || t.artist;
  const artistTracks = (name) => data.tracks.filter((t) => artistOf(t) === name);

  // ---- albums and artists ----

  function albumGrid(albums, { badge } = {}) {
    if (!albums.length) return emptyNote('No albums match.');
    const size = getPrefs().library.albumSize ?? 190;
    return h('div', { class: 'album-grid', style: { '--tile': `${size}px` } }, albums.map((a) => {
      const open = () => { detail = { type: 'album', album: a }; render(); };
      const cover = coverTile(a.tracks[0]);
      cover.append(quickPlay(async () => a.tracks, 'on-cover'));
      const card = h('div', { class: 'album-card', role: 'button', tabindex: '0', onclick: open, onkeydown: (e) => { if (e.key === 'Enter') open(); } },
        cover, h('span', { class: 'ac-title' }, a.title), h('span', { class: 'ac-artist' }, a.artist + (a.year ? ` · ${a.year}` : '')), badge && h('span', { class: 'ac-badge' }, badge(a)));
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
          h('div', { class: 'detail-meta' }, [a.year, plural(a.track_count, 'track'), fmtTime(a.duration_secs)].filter(Boolean).join(' · ')), actionsFor(a.tracks))),
      trackTable(tracks));
  }

  function artistRow(name, meta, onOpen) {
    const row = h('div', { class: 'artist-row', role: 'button', tabindex: '0', onclick: onOpen, onkeydown: (e) => { if (e.key === 'Enter') onOpen(); } },
      h('span', { class: 'ar-name' }, name), h('span', { class: 'ar-meta' }, meta), quickPlay(async () => artistTracks(name).map((t) => t.path), 'on-row'));
    row.addEventListener('contextmenu', async (ev) => { ev.preventDefault(); showMenu(await trackMenuItems(artistTracks(name).map((t) => t.path)), ev.clientX, ev.clientY); });
    return row;
  }

  function artistsView() {
    const list = data.artists.filter((a) => !query || a.name.toLowerCase().includes(query.toLowerCase()));
    if (!list.length) return emptyNote('No artists match.');
    return h('div', { class: 'artist-list' }, list.map((a) => artistRow(a.name, `${plural(a.album_count, 'album')}, ${plural(a.track_count, 'track')}`, () => { detail = { type: 'artist', artist: a }; render(); })));
  }

  function artistDetail(artist) {
    const albums = data.albums.filter((a) => a.artist === artist.name);
    const paths = artistTracks(artist.name).map((t) => t.path);
    return h('div', { class: 'detail' }, button('All artists', () => { detail = null; render(); }, { iconName: 'chevronLeft', cls: 'crumb' }),
      h('div', { class: 'detail-text' }, h('h2', { class: 'detail-title' }, artist.name), h('div', { class: 'detail-meta' }, `${plural(artist.album_count, 'album')} · ${plural(artist.track_count, 'track')}`), paths.length ? actionsFor(paths) : null),
      albumGrid(albums));
  }

  // ---- smart lists ----

  function allSongsView() {
    const key = getPrefs().library.sort ?? 'default';
    const by = {
      title: (a, b) => cmp(a.title, b.title), artist: (a, b) => cmp(artistOf(a), artistOf(b)) || cmp(a.album, b.album) || (a.track_no ?? 0) - (b.track_no ?? 0),
      album: (a, b) => cmp(a.album, b.album) || (a.track_no ?? 0) - (b.track_no ?? 0), year: (a, b) => (b.year ?? 0) - (a.year ?? 0) || cmp(a.album, b.album), length: (a, b) => b.duration_secs - a.duration_secs,
    }[key];
    const list = by ? [...data.tracks].sort(by) : data.tracks;
    const sorter = select({ label: 'Sort songs', value: key, options: SORTS.map(([value, label]) => ({ value, label })), onChange: (v) => { setPref('library.sort', v); render(); } });
    return listPage(null, list, { tools: sorter });
  }

  function favouritesView() {
    const list = state.favOrder.map((p) => data.byPath.get(p)).filter(Boolean);
    if (!list.length) return emptyNote('No favourites yet. Click the heart next to any track, or on the Now Playing screen, to add it here.');
    return listPage(null, list);
  }

  function recentPlayedView() {
    const rows = [...state.stats.values()].filter((s) => s.last_played > 0).sort((a, b) => b.last_played - a.last_played);
    const list = rows.map((s) => data.byPath.get(s.path)).filter(Boolean).slice(0, LIST_LIMIT);
    if (!list.length) return emptyNote('Nothing played yet. Tracks you listen to show up here, newest first.');
    return listPage(null, list, { extra: { head: 'Played', get: (t) => ago(state.stats.get(t.path)?.last_played) } });
  }

  function recentAddedView() {
    const list = [...data.tracks].filter((t) => t.added > 0).sort((a, b) => b.added - a.added || cmp(a.title, b.title)).slice(0, LIST_LIMIT);
    if (!list.length) return emptyNote('Nothing here yet.');
    return listPage(null, list, { extra: { head: 'Added', get: (t) => ago(t.added) } });
  }

  function notPlayedView() {
    const list = data.tracks.filter((t) => playsOf(t.path) === 0);
    if (!list.length) return emptyNote('You have played every track at least once.');
    return listPage(null, list);
  }

  function mostPlayedView() {
    const seg = h('div', { class: 'segmented', role: 'tablist', 'aria-label': 'Most played' }, [['tracks', 'Tracks'], ['albums', 'Albums'], ['artists', 'Artists']].map(([id, name]) =>
      h('button', { class: 'seg-btn', type: 'button', role: 'tab', 'aria-selected': String(mostMode === id), onclick: () => { mostMode = id; render(); } }, name)));
    let content;
    if (mostMode === 'tracks') {
      const list = [...data.tracks].filter((t) => playsOf(t.path) > 0).sort((a, b) => playsOf(b.path) - playsOf(a.path) || cmp(a.title, b.title)).slice(0, LIST_LIMIT);
      content = list.length ? listPage(null, list, { extra: { head: 'Plays', get: (t) => String(playsOf(t.path)) } }) : null;
    } else if (mostMode === 'albums') {
      const total = (a) => a.tracks.reduce((s, p) => s + playsOf(p), 0);
      const list = data.albums.filter((a) => total(a) > 0).sort((a, b) => total(b) - total(a) || cmp(a.title, b.title)).slice(0, 60);
      content = list.length ? albumGrid(list, { badge: (a) => plural(total(a), 'play') }) : null;
    } else {
      const totals = new Map();
      for (const t of data.tracks) { const n = playsOf(t.path); if (n) totals.set(artistOf(t), (totals.get(artistOf(t)) ?? 0) + n); }
      const list = [...totals].sort((a, b) => b[1] - a[1] || cmp(a[0], b[0])).slice(0, 60);
      content = list.length ? h('div', { class: 'artist-list' }, list.map(([name, n]) => {
        const a = data.artists.find((x) => x.name === name);
        return artistRow(name, `${plural(n, 'play')}${a ? ` · ${plural(a.track_count, 'track')}` : ''}`, () => { if (a) { detail = { type: 'artist', artist: a }; render(); } });
      })) : null;
    }
    return h('div', { class: 'detail' }, seg, content ?? emptyNote('Nothing has been played yet. Counts build up as you listen: a play counts after half a track or 30 seconds.'));
  }

  // ---- playlists ----

  function playlistsView() {
    const left = h('div', { class: 'pl-list' },
      h('div', { class: 'pl-actions' },
        button('New playlist', async () => { const n = await promptText('New playlist', { ok: 'Create', placeholder: 'Playlist name' }); if (n) { const id = await act(api.createPlaylist(n)); await reloadPlaylists(); if (id) detail = { type: 'playlist', id }; render(); } }, { iconName: 'plus' }),
        button('Import M3U', importM3u),
        button('Import online', importOnline)),
      data.playlists.length ? data.playlists.map((p) => h('button', { class: `pl-item${detail?.id === p.id ? ' current' : ''}`, type: 'button', onclick: () => { detail = { type: 'playlist', id: p.id }; render(); } }, h('span', { class: 'pl-name' }, p.name), h('span', { class: 'pl-count' }, String(p.track_count))))
        : h('p', { class: 'hint' }, 'No playlists yet.'));
    const right = h('div', { class: 'pl-detail' });
    if (detail?.type === 'playlist') playlistDetail(right, detail.id); else right.append(emptyNote('Choose a playlist, or make a new one.'));
    return h('div', { class: 'pl-layout' }, left, right);
  }

  async function importM3u() {
    const f = await pickFile([{ name: 'Playlists', extensions: ['m3u', 'm3u8'] }]).catch(() => null);
    if (!f) return;
    const r = await act(api.importM3u(f));
    if (!r) return;
    const parts = [`${r.matched + r.relinked} of ${plural(r.total, 'track')} found`];
    if (r.relinked) parts.push(`${r.relinked} matched to your library by name`);
    if (r.missing_count) parts.push(`${r.missing_count} not found on this PC`);
    toast(`Imported "${r.name}": ${parts.join(', ')}`);
    await reloadPlaylists();
    detail = { type: 'playlist', id: r.id };
    render();
  }

  async function importOnline() {
    const id = await importOnlinePlaylist();
    if (id == null) return;
    await reloadPlaylists();
    detail = { type: 'playlist', id };
    render();
  }

  async function reloadPlaylists() { data.playlists = await api.playlists(); emit('playlists'); }

  async function playlistDetail(host, id) {
    const token = renderToken;
    const p = await api.playlist(id);
    if (token !== renderToken) return;
    if (!p) { host.append(emptyNote('That playlist no longer exists.')); return; }
    const exists = p.tracks.length ? await act(api.pathsExist(p.tracks)) ?? [] : [];
    if (token !== renderToken) return;
    const gone = new Set(p.tracks.filter((_, i) => exists[i] === false));
    const tracks = p.tracks.map((path) => data.byPath.get(path) ?? { path, title: baseName(path), artist: '', album: '', duration_secs: 0, missing: gone.has(path) });
    tracks.forEach((t) => { if (gone.has(t.path)) t.missing = true; });
    const playable = tracks.filter((t) => !t.missing).map((t) => t.path);
    const reorder = async (from, to) => { await act(api.moveInPlaylist(id, from, to)); detail = { type: 'playlist', id }; render(); };
    const relink = async () => {
      const r = await act(api.relinkPlaylist(id));
      if (!r) return;
      toast(r.relinked ? `Matched ${plural(r.relinked, 'track')} to your library${r.still_missing ? `, ${r.still_missing} still missing` : ''}` : `No more matches found. Add the folder that holds these files in the Folders tab, then try again.`);
      render();
    };
    clear(host).append(
      h('div', { class: 'detail-head' }, h('div', { class: 'detail-text' }, h('h2', { class: 'detail-title' }, p.name),
        h('div', { class: 'detail-meta' }, `${plural(tracks.length, 'track')} · ${fmtTime(sum(tracks))}${gone.size ? ` · ${gone.size} not found` : ''}`),
        h('div', { class: 'detail-actions' },
          playable.length ? [button('Play', () => act(api.playPlaylist(id, 0)), { cls: 'primary', iconName: 'play' }), button('Shuffle', () => shufflePlay(playable), { iconName: 'shuffle' }), button('Add to queue', () => queueAdd(playable), { iconName: 'queue' })] : null,
          gone.size ? button('Relink missing', relink, { iconName: 'refresh' }) : null,
          button('Rename', async () => { const n = await promptText('Rename playlist', { value: p.name }); if (n) { await act(api.renamePlaylist(id, n)); await reloadPlaylists(); render(); } }),
          button('Export M3U', async () => { const d = await pickSave(`${p.name}.m3u8`, [{ name: 'Playlist', extensions: ['m3u8'] }]).catch(() => null); if (d) { await act(api.exportM3u(id, d)); toast('Playlist exported'); } }),
          button('Delete', async () => { if (await confirmDialog('Delete playlist?', `"${p.name}" will be removed. Your music files are not touched.`)) { await act(api.deletePlaylist(id)); detail = null; await reloadPlaylists(); render(); } }, { iconName: 'trash' })))),
      gone.size ? h('p', { class: 'hint warn' }, `${plural(gone.size, 'track')} in this playlist ${gone.size === 1 ? 'points' : 'point'} to files that aren't on this PC (the playlist was probably made elsewhere). They're skipped when playing. If your music is in a folder that isn't in the library yet, add it under Folders and press Relink missing.`) : '',
      tracks.length ? trackTable(tracks, {
        missing: gone,
        menuExtra: (t, i, idxs = [i]) => [
          { label: 'Remove from playlist', onClick: async () => { await act(api.removeFromPlaylist(id, idxs)); detail = { type: 'playlist', id }; reloadPlaylists().then(render); } },
          ...(idxs.length === 1 && i > 0 ? [{ label: 'Move up', onClick: () => reorder(i, i - 1) }] : []),
          ...(idxs.length === 1 && i < tracks.length - 1 ? [{ label: 'Move down', onClick: () => reorder(i, i + 1) }] : []), { sep: true }],
      }) : emptyNote('This playlist is empty. Right-click tracks anywhere and choose Add to playlist.'));
  }

  // ---- folders ----

  function foldersView() {
    const addFolder = async () => {
      const f = await pickFolder().catch(() => null);
      if (!f) return;
      if (await act(api.addFolder(f)) !== undefined) { data.folders = await api.libraryFolders(); render(); rescanLibrary(); }
    };
    const manage = h('div', { class: 'detail-actions' }, button('Add folder', addFolder, { iconName: 'plus', cls: 'primary' }), button('Rescan now', () => rescanLibrary(), { iconName: 'refresh' }));
    const root = folderPath && data.folders.map(norm).find((r) => folderPath === r || folderPath.startsWith(`${r}/`));
    if (!folderPath || !root) {
      return h('div', { class: 'folders' },
        h('p', { class: 'hint' }, 'Redshift reads tags and cover art from these folders and everything inside them. Your files are never changed. Open a folder to browse and play what is inside.'),
        data.folders.map((f) => {
          const n = tracksUnder(data.tracks, f).length;
          const row = h('div', { class: 'folder-row', role: 'button', tabindex: '0', onclick: () => { folderPath = norm(f); render(); }, onkeydown: (e) => { if (e.key === 'Enter') { folderPath = norm(f); render(); } } },
            icon('folder', 18), h('span', { class: 'folder-path' }, f), h('span', { class: 'folder-count' }, plural(n, 'track')), quickPlay(async () => tracksUnder(data.tracks, f).map((t) => t.path), 'on-row'),
            iconButton('trash', `Remove ${f}`, async (e) => { e.stopPropagation(); if (await act(api.removeFolder(f)) !== undefined) { data.folders = await api.libraryFolders(); await load(); } }, { size: 16 }));
          return row;
        }), manage);
    }
    const { dirs, files } = folderContents(data.tracks, folderPath);
    const all = tracksUnder(data.tracks, folderPath);
    const trail = crumbs(root, folderPath);
    return h('div', { class: 'detail' },
      h('div', { class: 'crumbs' }, button('Folders', () => { folderPath = null; render(); }, { iconName: 'chevronLeft', cls: 'crumb' }),
        trail.map((c, i) => [h('span', { class: 'crumb-sep' }, '/'), i === trail.length - 1 ? h('span', { class: 'crumb-here' }, c.name) : h('button', { class: 'crumb-link', type: 'button', onclick: () => { folderPath = c.path; render(); } }, c.name)])),
      h('div', { class: 'list-head' }, h('div', null, h('h2', { class: 'detail-title small' }, trail[trail.length - 1].name), h('div', { class: 'detail-meta' }, `${plural(all.length, 'track')} · ${fmtTime(sum(all))}${dirs.length ? ` · ${plural(dirs.length, 'folder')}` : ''}`)),
        all.length ? actionsFor(all.map((t) => t.path)) : null),
      dirs.length ? h('div', { class: 'sub-folders' }, dirs.map((d) => {
        const row = h('div', { class: 'folder-row', role: 'button', tabindex: '0', onclick: () => { folderPath = d.path; render(); }, onkeydown: (e) => { if (e.key === 'Enter') { folderPath = d.path; render(); } } },
          icon('folder', 18), h('span', { class: 'folder-path' }, d.name), h('span', { class: 'folder-count' }, plural(d.tracks.length, 'track')), quickPlay(async () => tracksUnder(data.tracks, d.path).map((t) => t.path), 'on-row'));
        row.addEventListener('contextmenu', async (ev) => { ev.preventDefault(); showMenu(await trackMenuItems(tracksUnder(data.tracks, d.path).map((t) => t.path)), ev.clientX, ev.clientY); });
        return row;
      })) : null,
      files.length ? trackTable(files) : (dirs.length ? null : emptyNote('No tracks in this folder.')));
  }

  // ---- search results ----

  /** What the filters look at besides the tags: favourites, play history and the songs of the playlists. */
  async function filterContext() {
    const ids = playlistsNeeded(filter);
    const want = ids.has('*') ? data.playlists.map((p) => p.id) : [...ids];
    await Promise.all(want.filter((id) => !plSets.has(id)).map(async (id) => { const p = await api.playlist(id).catch(() => null); plSets.set(id, new Set(p?.tracks ?? [])); }));
    return { favs: state.favs, stats: state.stats, playlists: plSets, now: Date.now() / 1000 };
  }

  async function saveResultsAsPlaylist(paths) {
    const name = await promptText('Save these songs as a playlist', { ok: 'Create', placeholder: 'Playlist name', value: query || '' });
    if (!name) return;
    await act(api.createPlaylist(name, paths));
    await reloadPlaylists();
    toast(`Created "${name}" with ${plural(paths.length, 'song')}`);
  }

  function resultsPage(list) {
    const paths = list.map((t) => t.path);
    const n = activeRules(filter).length;
    const what = [query && `\u201c${query}\u201d`, n && plural(n, 'filter')].filter(Boolean).join(' + ');
    const sorter = select({ label: 'Sort the results', value: getPrefs().library.resultSort ?? 'library', options: RESULT_SORTS.map(([value, label]) => ({ value, label })), onChange: (v) => { setPref('library.resultSort', v); render(); } });
    const clearAll = () => { filter.text = ''; query = ''; search.value = ''; filterUI.clear(); updateBadge(); };
    return h('div', { class: 'detail' },
      h('div', { class: 'list-head' },
        h('div', null, h('h2', { class: 'detail-title small' }, 'Search results'),
          h('div', { class: 'detail-meta' }, `${plural(list.length, 'song')} · ${fmtTime(sum(list))}${what ? ` · ${what}` : ''}${n > 1 ? ` · ${filter.match === 'any' ? 'any rule' : 'every rule'}` : ''}`)),
        h('div', { class: 'list-head-tools' }, list.length ? sorter : null, paths.length ? [actionsFor(paths), button('Save as playlist', () => saveResultsAsPlaylist(paths), { iconName: 'playlist' })] : null, button('Clear search', clearAll, { iconName: 'close' }))),
      list.length ? trackTable(list, { extra: { head: 'Format', get: detailLine, wide: true } })
        : emptyNote(`Nothing matches${what ? ` ${what}` : ''}.`, h('div', { class: 'detail-actions' }, button('Change the filters', () => toggleFilters(true), { iconName: 'sliders' }), button('Clear search', clearAll))));
  }

  // ---- the page ----

  // Where you were in the list. Rebuilding the same page (a track started, a favourite changed, a scan
  // finished) keeps it; going to another tab, album, search or sort starts at the top.
  let lastKey = '';
  const viewKey = () => JSON.stringify([tab, query, activeRules(filter), filter.match, getPrefs().library.resultSort, folderPath, mostMode, getPrefs().library.sort, detail?.type, detail?.id, detail?.album?.name, detail?.album?.artist, detail?.artist?.name]);
  function restoreScroll(top, listTop) {
    body.scrollTop = top;
    const list = body.querySelector('.vlist');
    if (!list || !listTop) return;
    // The list fills in on the next frame; put it back once it has its rows.
    requestAnimationFrame(() => requestAnimationFrame(() => { list.scrollTop = listTop; }));
  }

  async function render() {
    const token = ++renderToken;
    tabs.querySelectorAll('.lib-tab').forEach((b) => b.setAttribute('aria-selected', String(b.dataset.tab === tab)));
    const key = viewKey(), keep = key === lastKey;
    lastKey = key;
    if (!keep) picked.clear();
    const scroll = keep ? body.scrollTop : 0, listScroll = keep ? (body.querySelector('.vlist')?.scrollTop ?? 0) : 0;
    clear(body);
    if (!data.tracks.length && !state.scan?.running) {
      if (tab === 'folders') { body.append(foldersView()); return; }
      body.append(emptyNote('Your library is empty.', h('div', { class: 'detail-actions' }, button('Add a music folder', async () => { const f = await pickFolder().catch(() => null); if (f && (await act(api.addFolder(f))) !== undefined) { rescanLibrary(); } }, { cls: 'primary', iconName: 'folder' }))));
      return;
    }
    const searchable = ['albums', 'tracks', 'favourites', 'recent-played', 'most-played', 'recent-added', 'not-played'].includes(tab);
    // Filters look at the whole library whichever tab you are on; a typed search only does on the song tabs
    // (on Artists it narrows the list of artists).
    if (activeRules(filter).length || (query && searchable)) {
      const ctx = await filterContext();
      if (token !== renderToken) return;
      const results = sortResults(applyFilter(data.tracks, filter, ctx), getPrefs().library.resultSort ?? 'library', ctx);
      body.append(resultsPage(results));
      return;
    }
    if (detail?.type === 'album') body.append(albumDetail(detail.album));
    else if (detail?.type === 'artist') body.append(artistDetail(detail.artist));
    else if (tab === 'albums') body.append(albumGrid(data.albums));
    else if (tab === 'artists') body.append(artistsView());
    else if (tab === 'tracks') body.append(allSongsView());
    else if (tab === 'favourites') body.append(favouritesView());
    else if (tab === 'recent-played') body.append(recentPlayedView());
    else if (tab === 'most-played') body.append(mostPlayedView());
    else if (tab === 'recent-added') body.append(recentAddedView());
    else if (tab === 'not-played') body.append(notPlayedView());
    else if (tab === 'playlists') body.append(playlistsView());
    else if (tab === 'folders') body.append(foldersView());
    if (scroll || listScroll) restoreScroll(scroll, listScroll);
  }

  const renderScan = () => {
    const s = state.scan;
    scanBox.hidden = !s?.running;
    if (!s?.running) return;
    const pct = s.total ? Math.round((s.done / s.total) * 100) : 0;
    scanBox.replaceChildren(h('span', { class: 'scan-text' }, s.phase === 'reading' ? `Reading tags ${s.done} / ${s.total}` : 'Looking for music…'), h('span', { class: 'scan-bar' }, h('i', { style: { width: `${pct}%` } })));
  };
  const visible = () => state.view === 'library';
  const usesField = (id) => activeRules(filter).some((r) => r.field === id);
  on('scan', renderScan);
  on('library', () => load().catch(console.error));
  on('playlists', () => api.playlists().then((p) => { data.playlists = p; plSets.clear(); filterUI.setLists({ playlists: p }); if (visible() && (tab === 'playlists' || usesField('playlist'))) render(); }));
  // A new song only changes which row is highlighted: redraw the rows on screen, never the page.
  on('track', () => body.querySelectorAll('.vlist').forEach((l) => l.refresh?.()));
  on('favs', () => {
    body.querySelectorAll('.fav-btn').forEach((b) => { const on = state.favs.has(b.dataset.path); b.classList.toggle('on', on); b.setAttribute('aria-pressed', String(on)); });
    if (visible() && (tab === 'favourites' || usesField('favourite'))) render();
  });
  on('stats', () => { if (visible() && (['recent-played', 'most-played', 'not-played'].includes(tab) || usesField('plays') || usesField('lastPlayed'))) render(); });
  on('view', (v) => { if (v === 'library') refreshStats().catch(() => {}); });
  load().catch((e) => { console.error(e); body.append(emptyNote('The library could not be loaded.')); });
  return { reload: load };
}
