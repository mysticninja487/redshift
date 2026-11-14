// Title, artist, album and the file's technical details.

import { setView, toggleFavourite } from '../controller.js';
import { get } from '../prefs.js';
import { state } from '../state.js';
import { fmtKhz, h } from '../util.js';
import { icon } from '../ui/icons.js';
import { lifecycle } from './common.js';

export function cover(cls = 'i-cover') {
  const img = h('img', { class: 'cover-img', alt: '', draggable: 'false' });
  const ph = h('div', { class: 'cover-ph' }, icon('note', 40));
  const el = h('div', { class: cls }, ph, img);
  const set = (url) => {
    if (!url) { img.removeAttribute('src'); el.classList.remove('has-cover'); return; }
    img.onload = () => el.classList.add('has-cover');
    img.onerror = () => el.classList.remove('has-cover');
    if (img.getAttribute('src') !== url) { el.classList.remove('has-cover'); img.src = url; }
  };
  return { el, set };
}

const CODEC = { flac: 'FLAC', opus: 'Opus', mp3: 'MP3', vorbis: 'Vorbis', wav: 'WAV', pcm: 'WAV', aac: 'AAC', alac: 'ALAC' };

export function mount(el) {
  const L = lifecycle();
  const art = cover();
  const title = h('h2', { class: 'i-title' });
  const heart = h('button', { class: 'fav-btn i-fav', type: 'button', title: 'Favourite', 'aria-label': 'Favourite', onclick: () => state.trackPath && toggleFavourite([state.trackPath]) }, icon('heart', 20));
  const infoBtn = h('button', { class: 'i-infobtn', type: 'button', title: 'Song info (I)', 'aria-label': 'Song info', onclick: () => state.trackPath && import('../ui/songinfo.js').then((m) => m.openSongInfo(state.trackPath)) }, icon('info', 20));
  const titleRow = h('div', { class: 'i-title-row' }, title, infoBtn, heart);
  // Right-clicking the title (or anything in the title row) opens the same menu as a song in the library.
  titleRow.addEventListener('contextmenu', async (ev) => {
    if (!state.trackPath) return;
    ev.preventDefault();
    const [{ showMenu }, { trackMenuItems }] = await Promise.all([import('../ui/menu.js'), import('../ui/trackmenu.js')]);
    showMenu(await trackMenuItems([state.trackPath]), ev.clientX, ev.clientY);
  });
  const artist = h('div', { class: 'i-artist' });
  const album = h('div', { class: 'i-album' });
  const pos = h('div', { class: 'i-pos' });
  const tech = h('ul', { class: 'i-tech' });
  const empty = h('div', { class: 'i-empty' },
    h('p', null, 'Add a music folder to start listening.'),
    h('button', { class: 'btn primary', type: 'button', onclick: () => setView('library') }, icon('folder', 16), 'Open library'));
  const text = h('div', { class: 'i-text' }, pos, titleRow, artist, album, tech, empty);
  el.append(h('div', { class: 'info' }, art.el, text));

  function render() {
    const t = state.track, idle = !t;
    el.classList.toggle('is-idle', idle);
    art.el.hidden = !get('info.cover');
    art.set(state.coverUrl);
    title.textContent = idle ? 'Nothing playing' : t.title || 'Untitled';
    title.title = title.textContent;
    heart.hidden = idle;
    infoBtn.hidden = idle;
    const fav = !!state.trackPath && state.favs.has(state.trackPath);
    heart.classList.toggle('on', fav);
    heart.setAttribute('aria-pressed', String(fav));
    heart.title = heart.ariaLabel = fav ? 'Remove from favourites' : 'Add to favourites';
    artist.textContent = idle ? '' : t.artist || '';
    album.replaceChildren(...(idle ? [] : [h('span', { class: 'i-album-name' }, t.album || ''), t.year ? h('span', { class: 'i-year' }, String(t.year)) : null].filter(Boolean)));
    const idx = state.queue.entries.findIndex((e) => e.id === state.queue.current_entry);
    pos.textContent = idle || idx < 0 ? '' : `${String(idx + 1).padStart(2, '0')} / ${String(state.queue.entries.length).padStart(2, '0')}`;
    empty.hidden = !(idle && state.queue.entries.length === 0);
    tech.replaceChildren();
    if (idle || !get('info.format')) return;
    const c = state.queue.current;
    const bits = [];
    if (c?.codec) bits.push(CODEC[c.codec] ?? c.codec.toUpperCase());
    if (c?.source_rate) bits.push(fmtKhz(c.source_rate));
    if (c?.bits_per_sample) bits.push(`${c.bits_per_sample}-bit`);
    else if (t.bitrate_kbps) bits.push(`${t.bitrate_kbps} kbps`);
    if (c?.source_channels) bits.push(c.source_channels === 1 ? 'Mono' : c.source_channels === 2 ? 'Stereo' : `${c.source_channels} ch`);
    const rg = state.player.replaygain !== 'off' && get('info.replaygain');
    const db = state.player.replaygain === 'album' ? t.replaygain_album_db ?? t.replaygain_track_db : t.replaygain_track_db ?? t.replaygain_album_db;
    if (rg && db != null) bits.push(`${db > 0 ? '+' : ''}${db.toFixed(1)} dB`);
    tech.replaceChildren(...bits.map((b) => h('li', null, b)));
  }
  L.on(['track', 'queue', 'ui', 'player-event', 'favs'], render);
  L.on('status', render);
  render();
  return { destroy: () => L.destroy() };
}
