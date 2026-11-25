// Internet radio, front-end side: the stations kept as favourites, and what the Radio screen and the
// heart on Now Playing both need. The directory search and playback live in the backend (see api.js).

import { api } from './api.js';
import { emit } from './bus.js';
import { act, setView } from './controller.js';
import { askToGoOnline, online } from './online.js';
import { state } from './state.js';
import { toast } from './util.js';

/** Favourite stations, newest first. */
export const radio = { favs: [], recent: [], loaded: false };

export const radioOn = () => { const o = online(); return !!(o.enabled && o.radio !== false); };
export const isFavStation = (url) => radio.favs.some((s) => s.url === url);

export async function refreshStations() {
  try {
    [radio.favs, radio.recent] = await Promise.all([api.radioFavourites(), api.radioRecent()]);
    radio.loaded = true;
  } catch (e) { console.error(e); }
  emit('radio');
}

/** Adds or removes a station from the favourites. */
export async function setStationFavourite(station, on) {
  await act(api.radioSetFavourite(station, on));
  await refreshStations();
}

/** Plays a station, asking first to go online if that is still off. */
export async function playStation(station) {
  if (!station.supported) { toast('Redshift cannot play this kind of stream yet', 'error'); return false; }
  if (!radioOn()) {
    if (!await askToGoOnline()) return false;
    if (!radioOn()) { toast('Internet radio is switched off in Settings > Online', 'error'); return false; }
  }
  try {
    await api.radioPlay(station);
  } catch (e) { toast(String(e?.message ?? e), 'error'); return false; }
  await refreshStations();
  return true;
}

/** The station playing now, as the favourites or the recent list know it. */
export const stationPlaying = () => {
  const url = state.player.live?.url ?? (state.queue.current?.path ?? '');
  return [...radio.recent, ...radio.favs].find((s) => s.url === url) ?? null;
};

export const openRadio = () => setView('radio');
