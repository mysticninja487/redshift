// Play, skip, shuffle, repeat and volume.

import { api } from '../api.js';
import { emit } from '../bus.js';
import { act } from '../controller.js';
import { get } from '../prefs.js';
import { state } from '../state.js';
import { h } from '../util.js';
import { iconButton, knob, slider } from '../ui/controls.js';
import { icon } from '../ui/icons.js';
import { lifecycle } from './common.js';

const REPEAT_NEXT = { off: 'all', all: 'one', one: 'off' };
const REPEAT_LABEL = { off: 'Repeat off', all: 'Repeat all', one: 'Repeat one' };

export function mount(el) {
  const L = lifecycle();
  const shuffle = iconButton('shuffle', 'Shuffle', () => act(setShuffle(!state.queue.shuffle)), { cls: 't-shuffle' });
  const prev = iconButton('prev', 'Previous', () => act(api.previous()), { size: 22, cls: 't-prev' });
  const play = h('button', { class: 'btn t-play', type: 'button', 'aria-label': 'Play' }, h('span', { class: 't-play-icon' }));
  const next = iconButton('next', 'Next', () => act(api.next()), { size: 22, cls: 't-next' });
  const repeat = iconButton('repeat', 'Repeat', () => act(setRepeat(REPEAT_NEXT[state.queue.repeat] ?? 'off')), { cls: 't-repeat' });
  play.addEventListener('click', () => act(state.queue.entries.length ? api.togglePause() : Promise.resolve()));

  const mute = iconButton('volume', 'Mute', () => act(setMuted(!state.player.muted)), { cls: 't-mute' });
  let vol;
  const makeVolume = () => {
    const kind = get('transport.volume');
    const common = { min: 0, max: 1, step: 0.01, value: state.player.volume, label: 'Volume', reset: 1, format: (v) => `${Math.round(v * 100)}%`, onInput: (v) => act(setVolume(v)) };
    return kind === 'knob' ? knob({ ...common, size: 52 }) : slider(common);
  };
  vol = makeVolume();
  const volWrap = h('div', { class: 't-volume' }, get('transport.mute') ? mute : null, vol);
  const main = h('div', { class: 't-main' }, shuffle, prev, play, next, repeat);
  el.append(h('div', { class: 'transport' }, main, volWrap));

  let volKind = get('transport.volume');
  function render() {
    const playing = state.player.status === 'playing';
    play.replaceChildren(icon(playing ? 'pause' : 'play', 26));
    play.setAttribute('aria-label', playing ? 'Pause' : 'Play');
    play.classList.toggle('is-playing', playing);
    shuffle.setAttribute('aria-pressed', String(state.queue.shuffle));
    const r = state.queue.repeat;
    repeat.replaceChildren(icon(r === 'one' ? 'repeatOne' : 'repeat', 20));
    repeat.setAttribute('aria-pressed', String(r !== 'off'));
    repeat.title = repeat.ariaLabel = REPEAT_LABEL[r] ?? 'Repeat';
    const m = state.player.muted;
    mute.replaceChildren(icon(m || state.player.volume === 0 ? 'mute' : state.player.volume < 0.4 ? 'volumeLow' : 'volume', 20));
    mute.setAttribute('aria-pressed', String(m));
    const k = get('transport.volume');
    if (k !== volKind) { volKind = k; const nv = makeVolume(); vol.replaceWith(nv); vol = nv; }
    vol.set(state.player.volume);
    vol.classList.toggle('muted', m);
  }
  L.on(['player', 'queue', 'ui', 'status'], render);
  render();
  return { destroy: () => L.destroy() };
}

// The player is the source of truth, so these also update `state` right away to keep the UI snappy.
async function setVolume(v) { state.player.volume = v; emit('player'); if (state.player.muted && v > 0) await setMuted(false); return api.setVolume(v); }
async function setMuted(m) { state.player.muted = m; emit('player'); return api.setMuted(m); }
async function setShuffle(on) { state.queue.shuffle = on; emit('queue'); return api.setShuffle(on); }
async function setRepeat(mode) { state.queue.repeat = mode; emit('queue'); return api.setRepeat(mode); }
