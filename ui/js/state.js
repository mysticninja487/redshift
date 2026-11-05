// Everything the UI knows about playback right now, in one object. Components read it and
// subscribe to the bus topics that say when parts of it change.

export const state = {
  player: { status: 'stopped', position_secs: 0, volume: 1, muted: false, crossfade_secs: 0, replaygain: 'off', replaygain_preamp_db: 0, device_name: '', sample_rate: 48000 },
  queue: { entries: [], current_entry: null, current: null, shuffle: false, repeat: 'off' },
  settings: null,          // saved core settings (volume, eq, ...)
  track: null,             // tags of the playing track
  trackPath: null,
  coverUrl: null,          // image URL of the playing track's cover, or null
  art: null,               // colours pulled from that cover (see theme.js)
  lyrics: null,
  waveform: null,          // { peaks, rms, duration_secs } for the playing track
  favs: new Set(),         // favourite track paths
  favOrder: [],            // the same, most recently added first
  stats: new Map(),        // path -> { plays, last_played } for tracks that have been played
  view: 'now',             // 'now' | 'library' | 'settings'
  scan: null,              // library scan progress while one runs
  stamp: 0,                // performance.now() when player.position_secs was read
};

/** Playback position in seconds, moving smoothly between the backend's updates. */
export function position() {
  const p = state.player;
  if (p.status !== 'playing') return p.position_secs;
  return p.position_secs + (performance.now() - state.stamp) / 1000;
}

/** Length of the playing track, from the player or the tags. */
export function duration() {
  return state.queue.current?.duration_secs ?? state.track?.duration_secs ?? state.waveform?.duration_secs ?? 0;
}
export const isPlaying = () => state.player.status === 'playing';
