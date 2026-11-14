// The only file that talks to the Tauri backend. Everything else calls these functions, which
// keeps the rest of the UI testable in a plain browser against dev/mock.js.

const T = () => window.__TAURI__;

export const invoke = (cmd, args) => T().core.invoke(cmd, args);
export const listen = (event, cb) => T().event.listen(event, (e) => cb(e.payload));

const isWindows = /Windows/i.test(navigator.userAgent);

// Covers are cached by address, so a track whose artwork was just edited gets a new one.
const coverEdits = new Map();
export function bumpCover(path) { coverEdits.set(path, (coverEdits.get(path) ?? 0) + 1); }

/** URL of a track's cover art (served by the app; 404 if the track has none). Pass `size` (pixels) for a smaller copy. */
export function coverUrl(path, size) {
  if (!path) return null;
  if (window.__REDSHIFT_COVER__) return window.__REDSHIFT_COVER__(path, coverEdits.get(path));
  const enc = encodeURIComponent(path), v = coverEdits.get(path);
  const q = size || v ? `?${[size && `s=${size}`, v && `v=${v}`].filter(Boolean).join('&')}` : '';
  return `${isWindows ? `http://cover.localhost/${enc}` : `cover://localhost/${enc}`}${q}`;
}
/** Sizes for covers shown small: a row or tile, a grid card, and the big now-playing picture. */
export const COVER = { row: 96, card: 320, big: 1400 };

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
  // tag editor: the form's values, saving (only what changed), a .lrc file, and a picture file shrunk for preview
  tagForm: (path) => invoke('tag_form', { path }), writeTags: (path, edit) => invoke('write_tags', { path, edit }),
  writeLookFile: (path, text) => invoke('write_look_file', { path, text }), readLookFile: (path) => invoke('read_look_file', { path }),
  saveLyricsFile: (path, text, synced) => invoke('save_lyrics_file', { path, text, synced }),
  imagePreview: async (path) => new Blob([await invoke('image_preview', { path })]),
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
  listeningTotals: (ranges) => invoke('listening_totals', { ranges }),
  listeningReport: (from, to, tzOffsetMin) => invoke('listening_report', { from, to, tzOffsetMin }),
  appInfo: () => invoke('app_info'),
  openDataFolder: () => invoke('open_data_folder'),
  songInfo: (path) => invoke('song_info', { path }),
  listeningStreak: (tzOffsetMin) => invoke('listening_streak', { tzOffsetMin }),
  mixes: () => invoke('mixes'),
  clearHistory: () => invoke('clear_history'),
  setRecordHistory: (on) => invoke('set_record_history', { on }),
  favouritePaths: () => invoke('favourite_paths'), setFavourite: (paths, on) => invoke('set_favourite', { paths, on }),
  playStats: () => invoke('play_stats'), resetPlayStats: (path = null) => invoke('reset_play_stats', { path }),
  // fonts and tray
  fonts: () => invoke('fonts'), importFont: (path) => invoke('import_font', { path }), removeFont: (file) => invoke('remove_font', { file }),
  toggleMiniPlayer: () => invoke('toggle_mini_player'), miniSize: (expanded, size) => invoke('mini_size', { expanded, width: size?.[0] ?? null, height: size?.[1] ?? null }),
  miniOnTop: (on) => invoke('mini_on_top', { on }),
  setSleepTimer: (minutes) => invoke('set_sleep_timer', { minutes }), cancelSleepTimer: () => invoke('cancel_sleep_timer'), sleepStatus: () => invoke('sleep_status'),
  setCountPercent: (percent) => invoke('set_count_percent', { percent: Math.round(percent) }),
  setWatchFolders: (on) => invoke('set_watch_folders', { on }),
  setQueueOptions: (noDuplicates, keepShuffle, shuffleWholeQueue) => invoke('set_queue_options', { noDuplicates, keepShuffle, shuffleWholeQueue }),
  setNotifyOnTrack: (on) => invoke('set_notify_on_track', { on }),
  launchOnStartup: () => invoke('launch_on_startup'),
  setLaunchOnStartup: (on) => invoke('set_launch_on_startup', { on }),
  setRestoreSession: (on) => invoke('set_restore_session', { on }),
  setTray: (minimizeToTray, closeToTray) => invoke('set_tray', { minimizeToTray, closeToTray }),
};
