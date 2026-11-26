// Your own Navidrome / Subsonic server, front-end side: whether the Server screen may be used, how a
// song on the server is addressed, and how its details are shown. The browsing, login and playing live
// in the backend (see api.js).

import { api } from './api.js';
import { emit } from './bus.js';
import { online } from './online.js';

/** A song on the server rides through the queue, playlists and history as this path. */
export const serverSongPath = (id) => `subsonic://${id}`;
/** An album's cover is asked for like any cover, by a path of this kind. */
export const serverCoverPath = (coverId) => `subsonic-cover://${coverId}`;

/** What Settings holds about the server (never the password). */
export const server = { settings: null, loaded: false };

export const serverOn = () => { const o = online(); return !!(o.enabled && o.server !== false); };

export async function refreshServer() {
  try { server.settings = await api.serverSettings(); server.loaded = true; } catch (e) { console.error(e); }
  emit('server');
}

const KHZ = (hz) => (hz ? `${+(hz / 1000).toFixed(1)} kHz` : '');

/** "FLAC 16-bit / 44.1 kHz" or "MP3 320 kbps": what the file is, in a line. */
export function qualityLine(s) {
  const fmt = (s.suffix || '').toUpperCase();
  if (s.lossless) {
    const bits = s.bit_depth ? `${s.bit_depth}-bit` : '';
    return [fmt, [bits, KHZ(s.sample_rate)].filter(Boolean).join(' / ')].filter(Boolean).join(' ');
  }
  return [fmt, s.bitrate_kbps ? `${s.bitrate_kbps} kbps` : ''].filter(Boolean).join(' ');
}

/** 'lossless' when every song is, 'lossy' when none is, else 'mixed' (or '' with no songs). */
export function qualityOf(songs) {
  if (!songs.length) return '';
  const n = songs.filter((s) => s.lossless).length;
  return n === songs.length ? 'lossless' : n === 0 ? 'lossy' : 'mixed';
}
