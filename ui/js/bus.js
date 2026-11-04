// A tiny publish/subscribe hub. Topics are plain strings: 'player', 'queue', 'track', 'lyrics',
// 'waveform', 'art', 'ui', 'view', 'library', 'playlists'.

const subs = new Map();

export function on(topic, fn) {
  if (!subs.has(topic)) subs.set(topic, new Set());
  subs.get(topic).add(fn);
  return () => subs.get(topic)?.delete(fn);
}
export function emit(topic, payload) {
  for (const fn of subs.get(topic) ?? []) {
    try { fn(payload); } catch (e) { console.error(`handler for "${topic}" failed`, e); }
  }
}
