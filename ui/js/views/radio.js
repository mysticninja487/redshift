// Radio: find internet radio stations in the free radio-browser.info directory, keep favourites and
// play a station from its web address. Lossless (FLAC) stations are marked, because this player
// puts lossless first; everything else is labelled lossy.

import { api } from '../api.js';
import { on } from '../bus.js';
import { askToGoOnline, online } from '../online.js';
import { isFavStation, playStation, radio, radioOn, refreshStations, setStationFavourite } from '../radio.js';
import { setView } from '../controller.js';
import { isLive, state } from '../state.js';
import { debounce, h, toast } from '../util.js';
import { button } from '../ui/controls.js';
import { icon } from '../ui/icons.js';
import { toggle } from '../ui/controls.js';

const GENRES = ['jazz', 'classical', 'ambient', 'electronic', 'rock', 'pop', 'folk', 'hip hop', 'news'];
const LIMIT = 80;

export function mountRadio(root) {
  let query = '', tag = '', lossless = true;
  let results = [], status = 'idle'; // idle | loading | done | error
  let error = '';
  let token = 0;

  const search = h('input', { class: 'text-input rd-search', type: 'search', placeholder: 'Search stations by name', 'aria-label': 'Search stations',
    oninput: debounce((e) => { query = e.target.value.trim(); run(); }, 350) });
  const losslessToggle = toggle({ label: 'Lossless stations only', value: lossless, onChange: (v) => { lossless = v; run(); } });
  const genres = h('div', { class: 'rd-genres', role: 'group', 'aria-label': 'Genre' });
  const addr = h('input', { class: 'text-input rd-addr', type: 'url', placeholder: 'https://example.com/stream', 'aria-label': 'Station web address',
    onkeydown: (e) => { if (e.key === 'Enter') playAddress(); } });
  const body = h('div', { class: 'rd-body' });
  root.append(
    h('div', { class: 'v-head' }, h('h2', { class: 'v-title' }, 'Radio'),
      h('p', { class: 'rd-lede' }, 'Live stations from the free radio-browser.info directory. Lossless ones are marked, and everything else is labelled lossy.')),
    h('div', { class: 'rd-tools' },
      search,
      h('label', { class: 'rd-lossless' }, losslessToggle, h('span', null, 'Lossless only')),
      h('div', { class: 'rd-play-addr' }, addr, button('Play address', () => playAddress(), { iconName: 'play' }))),
    genres,
    h('div', { class: 'v-body' }, body));

  function paintGenres() {
    genres.replaceChildren(...GENRES.map((g) => h('button', { class: 'rd-genre', type: 'button', 'aria-pressed': String(tag === g), onclick: () => { tag = tag === g ? '' : g; paintGenres(); run(); } }, g)));
  }

  async function playAddress() {
    const url = addr.value.trim();
    if (!url) { addr.focus(); return; }
    if (!radioOn() && !await askToGoOnline()) return;
    try {
      const st = await api.radioStationFromUrl(url);
      await playStation(st);
    } catch (e) { toast(String(e?.message ?? e), 'error'); }
  }

  async function run() {
    if (!radioOn()) { render(); return; }
    const mine = ++token;
    status = 'loading'; error = ''; render();
    try {
      const r = await api.radioSearch({ name: query, tag, country: '', lossless_only: lossless, limit: LIMIT });
      if (mine !== token) return;
      results = r; status = 'done';
    } catch (e) {
      if (mine !== token) return;
      results = []; status = 'error'; error = String(e?.message ?? e);
    }
    render();
  }

  const nowUrl = () => (isLive() ? state.player.live?.url ?? state.queue.current?.path : null);

  function chips(s) {
    const out = [];
    if (s.codec) out.push(h('span', { class: 'rd-chip' }, s.bitrate && !s.lossless ? `${s.codec} ${s.bitrate} kbps` : s.codec));
    out.push(s.lossless ? h('span', { class: 'rd-chip is-lossless' }, 'Lossless') : h('span', { class: 'rd-chip is-lossy' }, 'Lossy'));
    if (!s.secure) out.push(h('span', { class: 'rd-chip is-warn', title: 'This address is plain http, so the stream and its address are not encrypted.' }, 'Not encrypted'));
    if (!s.supported) out.push(h('span', { class: 'rd-chip is-warn', title: 'Redshift cannot play HLS or HE-AAC streams yet.' }, 'Not supported'));
    return out;
  }

  function row(s) {
    const playing = nowUrl() === s.url;
    const fav = isFavStation(s.url);
    const heart = h('button', { class: `fav-btn rd-heart${fav ? ' on' : ''}`, type: 'button', 'aria-pressed': String(fav), title: fav ? 'Remove from favourites' : 'Add to favourites', 'aria-label': fav ? 'Remove from favourites' : 'Add to favourites',
      onclick: (e) => { e.stopPropagation(); setStationFavourite(s, !fav); } }, icon('heart', 18));
    const sub = [s.country, (s.tags || '').split(',').map((t) => t.trim()).filter(Boolean).slice(0, 3).join(', ')].filter(Boolean).join(' · ');
    const play = h('button', { class: 'rd-play', type: 'button', disabled: !s.supported, 'aria-label': `Play ${s.name}`, onclick: () => playStation(s) }, icon(playing ? 'record' : 'play', 18));
    return h('li', { class: `rd-row${playing ? ' is-playing' : ''}${s.supported ? '' : ' is-off'}` },
      play,
      h('div', { class: 'rd-text', onclick: () => s.supported && playStation(s) },
        h('div', { class: 'rd-name' }, s.name, playing ? h('span', { class: 'rd-live' }, 'Playing') : null),
        h('div', { class: 'rd-sub' }, sub)),
      h('div', { class: 'rd-chips' }, ...chips(s)), heart);
  }

  const section = (title, list, note) => (list.length ? h('section', { class: 'rd-sec' }, h('h3', { class: 'st-h3' }, title), note ? h('p', { class: 'hint' }, note) : null, h('ul', { class: 'rd-list' }, ...list.map(row))) : null);

  function gate() {
    const o = online();
    if (!o.enabled) return h('div', { class: 'rd-gate' }, icon('globe', 28),
      h('h3', null, 'Radio needs the internet'),
      h('p', null, 'Redshift stays offline unless you allow it. Searching sends your search words to radio-browser.info, and playing a station connects straight to that station.'),
      button('Go online', async () => { if (await askToGoOnline()) { render(); run(); } }, { cls: 'primary' }));
    return h('div', { class: 'rd-gate' }, icon('globe', 28),
      h('h3', null, 'Internet radio is switched off'),
      h('p', null, 'Turn it on in Settings > Online to search for stations and play them.'),
      button('Open settings', () => { setView('settings'); }, { cls: 'primary' }));
  }

  function render() {
    if (!radioOn()) { body.replaceChildren(gate()); return; }
    const favs = radio.favs, recent = radio.recent.filter((s) => !isFavStation(s.url)).slice(0, 8);
    const heading = query || tag ? 'Results' : lossless ? 'Lossless stations' : 'Popular stations';
    const kids = [];
    if (!query && !tag) {
      const f = section('Favourites', favs); if (f) kids.push(f);
      const r = section('Recently played', recent); if (r) kids.push(r);
    }
    if (status === 'loading') kids.push(h('p', { class: 'rd-note' }, 'Searching…'));
    else if (status === 'error') kids.push(h('div', { class: 'rd-note is-error' }, h('p', null, `Could not search the directory: ${error}`), button('Try again', run)));
    else if (status === 'done') {
      kids.push(results.length ? section(heading, results) : h('p', { class: 'rd-note' }, lossless ? 'No lossless stations matched. Turn off Lossless only to see every station.' : 'No stations matched.'));
    }
    body.replaceChildren(...kids);
  }

  paintGenres();
  render();
  const start = async () => { await refreshStations(); if (radioOn() && status === 'idle') run(); else render(); };
  on('radio', render);
  on('settings', () => { render(); if (radioOn() && status === 'idle') run(); });
  on('player', debounce(render, 400));
  start();
  return { reload: () => { render(); if (radioOn() && status !== 'loading' && !results.length) run(); } };
}
