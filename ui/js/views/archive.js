// Internet Archive: search the Archive's free audio and play it as the original files, so a FLAC stays
// FLAC. Each song is the best file the item has for it; "Lossless only" hides items and songs that
// only come as lossy files.

import { api, coverUrl, COVER } from '../api.js';
import { on } from '../bus.js';
import { act, playPaths, queueAdd, setView, shufflePlay, toggleFavourite } from '../controller.js';
import { getPrefs } from '../prefs.js';
import { askToGoOnline, online } from '../online.js';
import { state } from '../state.js';
import { qualityLine, qualityOf } from '../server.js';
import { debounce, fmtTime, h, plural, toast } from '../util.js';
import { button, select, toggle } from '../ui/controls.js';
import { icon } from '../ui/icons.js';
import { showMenu } from '../ui/menu.js';
import { trackMenuItems } from '../ui/trackmenu.js';
import { virtualList } from '../ui/vlist.js';

const COLLECTIONS = [['', 'All audio'], ['etree', 'Live concerts'], ['netlabels', 'Netlabels'], ['opensource_audio', 'Community audio']];
const SORTS = [['popular', 'Most downloaded'], ['newest', 'Newest'], ['relevance', 'Best match']];
const NO_COVER = '';

const archiveCover = (identifier) => `archive-cover://${identifier}`;
const compact = (n) => (n >= 1e6 ? `${+(n / 1e6).toFixed(1)}M` : n >= 1e3 ? `${+(n / 1e3).toFixed(1)}k` : String(n));
/** "Flac" → "FLAC", "VBR MP3" stays, "24bit Flac" → "24-bit FLAC". */
const formatName = (f) => f.replace(/^(\d+)bit/i, '$1-bit').replace(/flac/i, 'FLAC').replace(/wave/i, 'WAV');

export function mountArchive(root) {
  const q = { text: '', lossless_only: true, collection: '', sort: 'popular', page: 1 };
  let hits = [], total = 0, more = false;
  let detail = null; // { identifier, data }
  let status = 'idle', error = ''; // idle | loading | done | error
  let searched = false, token = 0;

  const search = h('input', { class: 'text-input lib-search', type: 'search', placeholder: 'Search the Internet Archive', 'aria-label': 'Search the Internet Archive',
    oninput: debounce((e) => { q.text = e.target.value.trim(); detail = null; run(); }, 400) });
  const head = h('header', { class: 'v-head' }, h('h1', { class: 'v-title' }, 'Internet Archive'), h('div', { class: 'lib-tools' }, search));
  const body = h('div', { class: 'v-body lib-body sv-body' });
  root.append(head, body);

  const ready = () => { const o = online(); return !!(o.enabled && o.archive !== false); };
  const msg = (e) => String(e?.message ?? e);

  // ---- loading ----

  async function run() {
    if (!ready()) { render(); return; }
    const mine = ++token;
    q.page = 1; status = 'loading'; error = ''; searched = true; render();
    try {
      const r = await api.archiveSearch({ ...q });
      if (mine !== token) return;
      hits = r.hits; total = r.total; more = r.hits.length >= r.page_size && r.page * r.page_size < r.total;
    } catch (e) {
      if (mine !== token) return;
      status = 'error'; error = msg(e); render(); return;
    }
    status = 'done'; render();
  }

  async function loadMore() {
    try {
      const r = await api.archiveSearch({ ...q, page: q.page + 1 });
      q.page = r.page; const seen = new Set(hits.map((x) => x.identifier));
      hits = [...hits, ...r.hits.filter((x) => !seen.has(x.identifier))];
      more = r.hits.length >= r.page_size && r.page * r.page_size < r.total;
    } catch (e) { toast(msg(e), 'error'); }
    render();
  }

  async function open(identifier) {
    const mine = ++token;
    status = 'loading'; error = ''; detail = { identifier, data: null }; render();
    try {
      const data = await api.archiveItem(identifier, q.lossless_only);
      if (mine !== token) return;
      detail = { identifier, data }; status = 'done';
    } catch (e) {
      if (mine !== token) return;
      status = 'error'; error = msg(e);
    }
    render();
  }

  // ---- pieces ----

  function cover(identifier, cls = 'lib-cover', px = COVER.card) {
    const img = h('img', { class: 'cover-img', alt: '', loading: 'lazy', draggable: 'false' });
    const el = h('div', { class: cls }, h('div', { class: 'cover-ph' }, icon('note', 32)), img);
    if (identifier) { img.onload = () => el.classList.add('has-cover'); img.src = coverUrl(archiveCover(identifier), px) ?? NO_COVER; }
    return el;
  }

  const qualityChip = (s) => h('span', { class: `rd-chip ${s.lossless ? 'is-lossless' : 'is-lossy'}` }, s.lossless ? 'Lossless' : 'Lossy');
  const pathsOf = (songs) => songs.map((s) => s.path);

  function actions(songs) {
    const paths = pathsOf(songs);
    return h('div', { class: 'detail-actions' },
      button('Play', () => act(api.archivePlay(paths, 0)), { cls: 'primary', iconName: 'play' }),
      button('Shuffle', () => shufflePlay(paths), { iconName: 'shuffle' }),
      button('Add to queue', () => queueAdd(paths), { iconName: 'queue' }));
  }

  function playRow(songs, i) {
    if ((getPrefs().library.playMode ?? 'single') === 'list' && songs.length > 1) act(api.archivePlay(pathsOf(songs), i));
    else act(api.archivePlay([songs[i].path], 0));
  }

  function songRow(songs, i) {
    const s = songs[i], playing = state.trackPath === s.path, fav = state.favs.has(s.path);
    const r = h('div', { class: `t-row sv-row${playing ? ' current' : ''}`, role: 'listitem', tabindex: '-1', ondblclick: () => playRow(songs, i), onkeydown: (e) => { if (e.key === 'Enter') playRow(songs, i); } },
      h('span', { class: 't-no' }, h('span', { class: 't-num' }, playing ? icon('play', 12) : String(s.track ?? i + 1)),
        h('button', { class: 'row-play', type: 'button', title: 'Play', 'aria-label': `Play ${s.title}`, onclick: (e) => { e.stopPropagation(); playRow(songs, i); } }, icon('play', 12))),
      h('span', { class: 't-title', translate: 'no' }, s.title), h('span', { class: 't-artist', translate: 'no' }, s.artist), h('span', { class: 't-album', translate: 'no' }, s.album),
      h('span', { class: 'sv-quality', title: qualityLine(s) }, qualityChip(s), h('span', { class: 'sv-fmt', translate: 'no' }, qualityLine(s) || formatName(s.format))),
      h('button', { class: `fav-btn${fav ? ' on' : ''}`, type: 'button', title: 'Favourite', 'aria-label': 'Favourite', 'aria-pressed': String(fav), onclick: (e) => { e.stopPropagation(); toggleFavourite([s.path]); } }, icon('heart', 15)),
      h('span', { class: 't-dur' }, fmtTime(s.duration_secs)));
    r.addEventListener('contextmenu', async (ev) => {
      ev.preventDefault();
      const extra = songs.length > 1 ? [{ label: 'Play from here', onClick: () => act(api.archivePlay(pathsOf(songs), i)) }, { sep: true }] : [];
      showMenu(await trackMenuItems([s.path], extra), ev.clientX, ev.clientY);
    });
    return r;
  }

  function songTable(songs) {
    const headRow = h('div', { class: 't-row sv-row t-head' }, h('span', { class: 't-no' }, '#'), h('span', { class: 't-title' }, 'Title'), h('span', { class: 't-artist' }, 'Artist'), h('span', { class: 't-album' }, 'Item'),
      h('span', { class: 'sv-quality' }, 'Quality'), h('span', { class: 't-fav' }), h('span', { class: 't-dur' }, 'Time'));
    const vl = virtualList({ rowHeight: 40, cls: 't-list', render: (i) => songRow(songs, i) });
    requestAnimationFrame(() => vl.setCount(songs.length));
    return h('div', { class: 't-table' }, headRow, vl);
  }

  const note = (text, cls = '') => h('p', { class: `rd-note ${cls}`.trim() }, text);
  const back = (label, fn) => button(label, fn, { iconName: 'chevronLeft', cls: 'crumb' });

  function hitCard(a) {
    const sub = [a.creator, a.year].filter(Boolean).join(' · ');
    const card = h('div', { class: 'album-card ar-hit', role: 'button', tabindex: '0', onclick: () => open(a.identifier), onkeydown: (e) => { if (e.key === 'Enter') open(a.identifier); } },
      cover(a.identifier), h('span', { class: 'ac-title', translate: 'no' }, a.title), h('span', { class: 'ac-artist', translate: 'no' }, sub || ' '),
      h('span', { class: 'ar-formats' }, h('span', { class: `rd-chip ${a.lossless ? 'is-lossless' : 'is-lossy'}` }, a.lossless ? 'Lossless' : 'Lossy'),
        h('span', { class: 'sv-fmt', translate: 'no', title: a.formats.map(formatName).join(', ') }, a.formats.slice(0, 2).map(formatName).join(', ')),
        a.downloads ? h('span', { class: 'ar-dl', title: `${a.downloads.toLocaleString()} downloads` }, `${compact(a.downloads)} plays`) : null));
    card.addEventListener('contextmenu', async (ev) => {
      ev.preventDefault();
      showMenu([{ label: 'Open', onClick: () => open(a.identifier) }, { label: 'Play all', onClick: async () => { try { const v = await api.archiveItem(a.identifier, q.lossless_only); if (v.tracks.length) await act(api.archivePlay(pathsOf(v.tracks), 0)); else toast('Nothing to play in that item', 'error'); } catch (e) { toast(msg(e), 'error'); } } },
        { label: 'Add all to queue', onClick: async () => { try { const v = await api.archiveItem(a.identifier, q.lossless_only); queueAdd(pathsOf(v.tracks)); } catch (e) { toast(msg(e), 'error'); } } }], ev.clientX, ev.clientY);
    });
    return card;
  }

  // ---- screens ----

  function itemDetail(v) {
    const { tracks } = v, qq = qualityOf(tracks);
    const total = tracks.reduce((t, s) => t + s.duration_secs, 0);
    const lines = [v.date, plural(tracks.length, 'song'), total ? fmtTime(total) : ''].filter(Boolean).join(' · ');
    return h('div', { class: 'detail' },
      back('Back', () => { detail = null; render(); }),
      h('div', { class: 'detail-head' }, cover(v.identifier, 'detail-cover', 480),
        h('div', { class: 'detail-text' }, h('h2', { class: 'detail-title', translate: 'no' }, v.title), h('div', { class: 'detail-sub', translate: 'no' }, v.creator),
          h('div', { class: 'detail-meta' }, lines),
          qq ? h('div', { class: 'sv-q-line' }, h('span', { class: `rd-chip ${qq === 'lossless' ? 'is-lossless' : 'is-lossy'}` }, qq === 'lossless' ? 'Lossless' : qq === 'lossy' ? 'Lossy' : 'Mixed quality')) : null,
          tracks.length ? actions(tracks) : null)),
      v.hidden_lossy ? note(`${plural(v.hidden_lossy, 'song')} hidden because ${v.hidden_lossy === 1 ? 'it only exists' : 'they only exist'} as lossy files.`, 'ar-hidden') : null,
      tracks.length ? songTable(tracks) : note(q.lossless_only ? 'This item has no lossless songs. Turn off Lossless only to see its lossy files.' : 'This item has no songs Redshift can play.'),
      v.description ? h('p', { class: 'ar-desc', translate: 'no' }, v.description) : null,
      h('div', { class: 'ar-credit' },
        h('span', null, 'From the Internet Archive.'),
        button('View on archive.org', () => act(api.openProjectPage(v.details_url)), { cls: 'crumb' }),
        v.license_url ? button('Licence', () => act(api.openProjectPage(v.license_url)), { cls: 'crumb' }) : null));
  }

  function tools() {
    return h('div', { class: 'ar-tools' },
      h('label', { class: 'ar-lossless' }, toggle({ label: 'Lossless only', value: q.lossless_only, onChange: (v) => { q.lossless_only = v; if (detail?.data) open(detail.identifier); else if (searched) run(); } }), h('span', null, 'Lossless only')),
      select({ label: 'Collection', value: q.collection, options: COLLECTIONS.map(([value, label]) => ({ value, label })), onChange: (v) => { q.collection = v; detail = null; run(); } }),
      select({ label: 'Sort by', value: q.sort, options: SORTS.map(([value, label]) => ({ value, label })), onChange: (v) => { q.sort = v; detail = null; run(); } }));
  }

  function results() {
    const first = !searched || (!q.text && status === 'idle');
    if (first) return h('div', { class: 'rd-gate ar-intro' }, icon('archive', 28), h('h3', null, 'Free music from the Internet Archive'),
      h('p', null, 'Live concert recordings, netlabel releases and public-domain audio, mostly as FLAC, so they play as the original files. Search above, or browse the most downloaded.'),
      button('Browse popular', () => run(), { cls: 'primary' }));
    return h('div', { class: 'detail' },
      h('p', { class: 'hint ar-count' }, total ? `${total.toLocaleString()} ${total === 1 ? 'item' : 'items'}${q.lossless_only ? ' with lossless files' : ''}` : ''),
      hits.length ? h('div', { class: 'album-grid', style: { '--tile': `${getPrefs().library.albumSize ?? 190}px` } }, hits.map(hitCard)) : note(q.lossless_only ? 'Nothing with lossless files matches that. Turn off Lossless only to include lossy items.' : 'Nothing matches that.'),
      more ? button('Show more', loadMore, { cls: 'sv-more' }) : null);
  }

  function gate() {
    const o = online();
    if (!o.enabled) return h('div', { class: 'rd-gate' }, icon('globe', 28), h('h3', null, 'The Internet Archive needs the network'),
      h('p', null, 'Redshift stays offline unless you allow it. Searching and playing here sends requests to archive.org, and nowhere else. Your search words are sent to it.'),
      button('Go online', async () => { if (await askToGoOnline()) { run(); } }, { cls: 'primary' }));
    return h('div', { class: 'rd-gate' }, icon('archive', 28), h('h3', null, 'The Internet Archive is switched off'),
      h('p', null, 'Turn on Internet Archive in Settings > Online to use it.'), button('Open settings', () => setView('settings'), { cls: 'primary' }));
  }

  function render() {
    const keep = body.scrollTop;
    if (!ready()) { body.replaceChildren(gate()); return; }
    const bar = detail ? null : tools();
    const view = status === 'loading' ? note('Loading…')
      : status === 'error' ? h('div', { class: 'rd-note is-error' }, h('p', null, `Could not reach the Internet Archive: ${error}`), button('Try again', () => { if (detail && !detail.data) { const id = detail.identifier; detail = null; open(id); } else run(); }))
      : detail?.data ? itemDetail(detail.data) : results();
    body.replaceChildren(...[bar, view].filter(Boolean));
    if (detail?.data == null) body.scrollTop = keep;
  }

  on('settings', () => { render(); });
  on('player', debounce(() => { if (detail?.data) render(); }, 400));
  on('favs', () => { if (detail?.data) render(); });
  render();
  return { reload: () => render() };
}
