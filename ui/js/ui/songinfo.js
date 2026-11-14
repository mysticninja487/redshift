// "Song info": everything the player knows about one song, in one dialog. Opened from the song
// right-click menu, the info button beside the title on Now Playing, or the I key.

import { api, COVER, coverUrl } from '../api.js';
import { state } from '../state.js';
import { fmtKhz, fmtTime, h, plural, toast } from '../util.js';
import { icon } from './icons.js';
import { button } from './controls.js';
import { modal } from './dialog.js';

const CODEC = { flac: 'FLAC', opus: 'Opus', mp3: 'MP3', ogg: 'Ogg Vorbis', oga: 'Ogg Vorbis', m4a: 'AAC / ALAC (M4A)', aac: 'AAC', wav: 'WAV', wv: 'WavPack', ape: 'Monkey’s Audio', aiff: 'AIFF', aif: 'AIFF' };

const bytes = (n) => {
  if (!n) return '';
  const u = ['B', 'KB', 'MB', 'GB'];
  let i = 0, v = n;
  while (v >= 1024 && i < u.length - 1) { v /= 1024; i++; }
  return `${i === 0 ? v : v.toFixed(v >= 100 ? 0 : 1)} ${u[i]}`;
};
const day = (secs) => new Date(secs * 1000).toLocaleDateString(undefined, { day: 'numeric', month: 'long', year: 'numeric' });
const when = (secs) => new Date(secs * 1000).toLocaleString(undefined, { day: 'numeric', month: 'short', year: 'numeric', hour: 'numeric', minute: '2-digit' });
const listened = (secs) => (secs < 60 ? `${Math.round(secs)} s` : secs < 3600 ? `${Math.round(secs / 60)} min` : `${(secs / 3600).toFixed(1)} h`);
const gain = (db) => (db == null ? '' : `${db > 0 ? '+' : ''}${db.toFixed(2)} dB`);

/** The rows of the dialog, as [section title, [[label, value], …]] with empty values left out. */
export function infoSections(info) {
  const m = info.meta;
  const known = (v, bad = []) => (v && !bad.includes(v) ? String(v) : '');
  const channels = m.channels === 1 ? 'Mono' : m.channels === 2 ? 'Stereo' : m.channels ? `${m.channels} channels` : '';
  const lyrics = { file: 'In a .lrc / .txt file next to the song', embedded: 'Saved in the song’s tags', none: 'None found' }[info.lyrics];
  const sections = [
    ['Tags', [
      ['Title', m.title],
      ['Artist', known(m.artist, ['Unknown Artist'])],
      ['Album', known(m.album, ['Unknown Album'])],
      ['Album artist', m.album_artist !== m.artist ? m.album_artist : ''],
      ['Genre', m.genre],
      ['Year', m.year],
      ['Track', m.track_no ? (m.disc_no && m.disc_no > 1 ? `${m.track_no} (disc ${m.disc_no})` : m.track_no) : ''],
      ['Lyrics', lyrics],
    ]],
    ['Audio', [
      ['Length', fmtTime(m.duration_secs)],
      ['Format', CODEC[m.format] ?? m.format.toUpperCase()],
      ['Bitrate', m.bitrate_kbps ? `${m.bitrate_kbps} kbps` : ''],
      ['Sample rate', fmtKhz(m.sample_rate)],
      ['Bit depth', m.bit_depth ? `${m.bit_depth}-bit` : ''],
      ['Channels', channels],
      ['ReplayGain', [m.replaygain_track_db != null ? `track ${gain(m.replaygain_track_db)}` : '', m.replaygain_album_db != null ? `album ${gain(m.replaygain_album_db)}` : ''].filter(Boolean).join(' · ')],
    ]],
    ['Listening', info.in_library || info.plays || info.listened_secs ? [
      ['Plays', plural(info.plays, 'play')],
      ['Time listened', info.listened_secs ? listened(info.listened_secs) : ''],
      ['Last played', info.last_played ? when(info.last_played) : 'Never'],
      ['First heard', info.first_heard ? day(info.first_heard) : ''],
      ['Favourite', info.favourite ? 'Yes' : 'No'],
    ] : []],
    ['File', [
      ['Location', m.path],
      ['Size', bytes(m.size)],
      ['Modified', m.modified ? day(m.modified) : ''],
      ['Added to library', info.in_library && m.added ? day(m.added) : info.in_library ? '' : 'Not in your library'],
    ]],
  ];
  return sections.map(([t, rows]) => [t, rows.filter(([, v]) => v !== '' && v != null)]).filter(([, rows]) => rows.length);
}

/** Opens the dialog for one song. */
export async function openSongInfo(path) {
  if (!path) return;
  let info;
  try { info = await api.songInfo(path); } catch (e) { toast(`Couldn’t read that song: ${e?.message ?? e}`); return; }
  const m = info.meta;
  await modal((box, done) => {
    box.classList.add('si-dialog');
    const img = h('img', { class: 'si-cover', alt: '', draggable: 'false' });
    const frame = h('div', { class: 'si-art' }, icon('note', 34), img);
    img.onload = () => frame.classList.add('has-cover');
    img.src = coverUrl(path, COVER.big);
    const copy = async (text, what) => { try { await navigator.clipboard.writeText(text); toast(`${what} copied`); } catch { toast('Couldn’t copy that'); } };
    const sections = infoSections(info).map(([title, rows]) => h('section', { class: 'si-sec' },
      h('h4', { class: 'si-h' }, title),
      h('dl', { class: 'si-list' }, rows.map(([k, v]) => [h('dt', null, k), h('dd', { class: k === 'Location' ? 'si-path' : '' }, String(v))]))));
    box.append(
      h('div', { class: 'si-head' }, frame,
        h('div', { class: 'si-who' }, h('h3', { class: 'dialog-title', id: 'si-title' }, m.title), h('p', { class: 'si-sub' }, [m.artist, m.album].filter((x) => x && !x.startsWith('Unknown ')).join(' · ')))),
      h('div', { class: 'si-body' }, sections),
      h('div', { class: 'dialog-actions' },
        button('Copy path', () => copy(m.path, 'Path'), { iconName: 'copy' }),
        button('Edit tags…', async () => { done(); const { openTagEditor } = await import('./tageditor.js'); openTagEditor([path]); }, { iconName: 'edit' }),
        button('Close', () => done(), { cls: 'primary' })));
    box.setAttribute('aria-labelledby', 'si-title');
  });
}

/** The song that's playing, if any. */
export function openCurrentSongInfo() {
  if (state.trackPath) openSongInfo(state.trackPath);
}
