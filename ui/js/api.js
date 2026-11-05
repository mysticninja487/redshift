// The only file that talks to the Tauri backend. Everything else calls these functions, which
// keeps the rest of the UI testable in a plain browser against dev/mock.js.

const T = () => window.__TAURI__;

export const invoke = (cmd, args) => T().core.invoke(cmd, args);
export const listen = (event, cb) => T().event.listen(event, (e) => cb(e.payload));

const isWindows = /Windows/i.test(navigator.userAgent);

/** URL of a track's cover art (served by the app; 404 if the track has none). */
export function coverUrl(path) {
  if (!path) return null;
  if (window.__REDSHIFT_COVER__) return window.__REDSHIFT_COVER__(path);
  const enc = encodeURIComponent(path);
  return isWindows ? `http://cover.localhost/${enc}` : `cover://localhost/${enc}`;
}

/** URL of an imported font file (served by the app). */
export function fontUrl(file) {
  if (window.__REDSHIFT_FONT__) return window.__REDSHIFT_FONT__(file);
  const enc = encodeURIComponent(file);
  return isWindows ? `http://font.localhost/${enc}` : `font://localhost/${enc}`;
}

export async function pickFolder() {
  const r = await T().dialog.open({ directory: true, multiple: false });
  return typeof r === 'string' ? r : null;
}
export async function pickFile(filters) {
  const r = await T().dialog.open({ multiple: false, filters });
  return typeof r === 'string' ? r : null;
}
export async function pickSave(defaultPath, filters) {
  const r = await T().dialog.save({ defaultPath, filters });
  return typeof r === 'string' ? r : null;
}

export const api = {
  // transport and queue
  setQueue: (paths, start = 0, autoplay = true) => invoke('set_queue', { paths, start, autoplay }),
  playEntry: (id) => invoke('play_entry', { id }),
  pause: () => invoke('pause'), resume: () => invoke('resume'), togglePause: () => invoke('toggle_pause'), stop: () => invoke('stop'),
  next: () => invoke('next'), previous: () => invoke('previous'), seek: (secs) => invoke('seek', { secs }),
  queueAppend: (paths) => invoke('queue_append', { paths }), queuePlayNext: (paths) => invoke('queue_play_next', { paths }),
  queueRemove: (id) => invoke('queue_remove', { id }), queueMove: (from, to) => invoke('queue_move', { from, to }),
  queueClear: () => invoke('queue_clear'),
  playerState: () => invoke('player_state'), queueView: () => invoke('queue_view'),
  // sound
  getSettings: () => invoke('get_settings'),
  setVolume: (volume) => invoke('set_volume', { volume }), setMuted: (muted) => invoke('set_muted', { muted }),
  setShuffle: (on) => invoke('set_shuffle', { on }), setRepeat: (mode) => invoke('set_repeat', { mode }),
  setCrossfade: (secs) => invoke('set_crossfade_secs', { secs }),
  setReplayGain: (mode, preampDb) => invoke('set_replaygain', { mode, preampDb }),
  outputDevices: () => invoke('output_devices'), setOutputDevice: (id) => invoke('set_output_device', { id }),
  // eq
  eqInfo: () => invoke('eq_info'), eqEnabled: (on) => invoke('set_eq_enabled', { on }), eqPreamp: (db) => invoke('set_eq_preamp_db', { db }),
  eqBand: (band, db) => invoke('set_eq_band_db', { band, db }), eqGains: (gains) => invoke('set_eq_gains_db', { gains }),
  eqPreset: (name) => invoke('apply_eq_preset', { name }), saveEqPreset: (name) => invoke('save_eq_preset', { name }),
  deleteEqPreset: (name) => invoke('delete_eq_preset', { name }), eqCurve: (points) => invoke('eq_curve', { points }),
  // visualisation
  vizFrame: () => invoke('viz_frame'), configureViz: (config) => invoke('configure_viz', { config }),
  // ui state
  uiState: () => invoke('ui_state'), setUiState: (value) => invoke('set_ui_state', { value }),
  // library
  libraryFolders: () => invoke('library_folders'), addFolder: (folder) => invoke('add_library_folder', { folder }),
  removeFolder: (folder) => invoke('remove_library_folder', { folder }), scanLibrary: () => invoke('scan_library'),
  isScanning: () => invoke('is_scanning'), tracks: () => invoke('library_tracks'), search: (query) => invoke('search_library', { query }),
  albums: () => invoke('library_albums'), artists: () => invoke('library_artists'), recent: (limit = 30) => invoke('recently_added', { limit }),
  trackMeta: (path) => invoke('track_meta', { path }), lyrics: (path) => invoke('lyrics', { path }),
  waveform: (path, buckets) => invoke('waveform', { path, buckets }),
  // playlists
  playlists: () => invoke('playlists'), playlist: (id) => invoke('playlist', { id }),
  createPlaylist: (name, tracks = []) => invoke('create_playlist', { name, tracks }),
  renamePlaylist: (id, name) => invoke('rename_playlist', { id, name }), deletePlaylist: (id) => invoke('delete_playlist', { id }),
  movePlaylist: (id, to) => invoke('move_playlist', { id, to }),
  addToPlaylist: (id, tracks, position = null) => invoke('add_to_playlist', { id, tracks, position }),
  removeFromPlaylist: (id, positions) => invoke('remove_from_playlist', { id, positions }),
  moveInPlaylist: (id, from, to) => invoke('move_in_playlist', { id, from, to }),
  importM3u: (file) => invoke('import_m3u', { file }), exportM3u: (id, dest) => invoke('export_m3u', { id, dest }),
  playPlaylist: (id, start = 0) => invoke('play_playlist', { id, start }),
  relinkPlaylist: (id) => invoke('relink_playlist', { id }), pathsExist: (paths) => invoke('paths_exist', { paths }),
  // favourites and play history
  favouritePaths: () => invoke('favourite_paths'), setFavourite: (paths, on) => invoke('set_favourite', { paths, on }),
  playStats: () => invoke('play_stats'), resetPlayStats: (path = null) => invoke('reset_play_stats', { path }),
  // fonts and tray
  fonts: () => invoke('fonts'), importFont: (path) => invoke('import_font', { path }), removeFont: (file) => invoke('remove_font', { file }),
  setTray: (minimizeToTray, closeToTray) => invoke('set_tray', { minimizeToTray, closeToTray }),
};
