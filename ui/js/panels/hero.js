// The centrepiece. Modes: turntable (a record that spins, with a tonearm that tracks the song),
// art (the cover, with optional sun / halftone / glitch treatments), viz (any visualiser), none.

import { cfg, featureOn, get, getPrefs } from '../prefs.js';
import { getSkin } from '../skins.js';
import { duration, isPlaying, position, state } from '../state.js';
import { clear, clamp, h, svg } from '../util.js';
import { mountViz, watchLive } from '../viz/engine.js';
import { cover } from './info.js';
import { lifecycle } from './common.js';

// Tonearm geometry in a 100-unit square: pivot top-right, platter centred at 50,50.
const PIVOT = [88, 12], CENTRE = [50, 50], ARM_LEN = 60, R_OUT = 43, R_IN = 21, R_REST = 58;
const D = Math.hypot(CENTRE[0] - PIVOT[0], CENTRE[1] - PIVOT[1]);
const ALPHA = (Math.atan2(CENTRE[1] - PIVOT[1], CENTRE[0] - PIVOT[0]) * 180) / Math.PI;
const phi = (r) => (Math.acos(clamp((D * D + ARM_LEN * ARM_LEN - r * r) / (2 * D * ARM_LEN), -1, 1)) * 180) / Math.PI;
/** Arm angle (degrees) that puts the stylus at radius `r` from the platter centre. */
const armAngle = (r, side = 1) => ALPHA + side * phi(r);

const ARM_SVG = `
<svg class="tt-arm" viewBox="0 0 100 100" aria-hidden="true">
  <g class="arm" transform="translate(88 12) rotate(81)">
    <rect x="-17" y="-3.2" width="12" height="6.4" rx="1.4" class="arm-weight"/>
    <rect x="-6" y="-1.15" width="${ARM_LEN - 4}" height="2.3" rx="1.1" class="arm-tube"/>
    <g transform="translate(${ARM_LEN - 6} 0) rotate(-14)"><rect x="0" y="-3" width="9" height="6" rx="1.2" class="arm-head"/><rect x="7.5" y="-1" width="3" height="2" class="arm-tip"/></g>
    <circle r="6.4" class="arm-base"/><circle r="2.4" class="arm-pin"/>
  </g>
</svg>`;

function turntable(host, c) {
  const L = lifecycle();
  const label = cover('tt-label');
  const spin = h('div', { class: 'tt-spin' }, h('div', { class: 'tt-scratch' }), label.el, h('div', { class: 'tt-hole' }));
  const disc = h('div', { class: 'tt-disc' }, spin, h('div', { class: 'tt-sheen' }));
  const arm = c.hero.tonearm ? svg(ARM_SVG) : null;
  const tt = h('div', { class: `tt${c.hero.plinth ? ' with-plinth' : ''}` }, h('div', { class: 'tt-plinth' }), h('div', { class: 'tt-platter' }, disc), arm);
  host.append(h('div', { class: 'tt-frame' }, tt));
  const armEl = arm?.querySelector('.arm');
  let angle = 0, omega = 0, theta = armAngle(R_REST, -1), first = true;

  L.frame((dt) => {
    const playing = isPlaying(), calm = document.documentElement.dataset.motion === 'reduced' || !get('motion.spin');
    omega += ((playing && !calm ? 200 : 0) - omega) * (1 - Math.exp(-(playing ? 1.4 : 0.9) * dt)); // 33 1/3 rpm
    angle = (angle + omega * dt) % 360;
    spin.style.transform = `rotate(${angle}deg)`;
    if (armEl) {
      const dur = duration(), p = dur > 0 ? clamp(position() / dur, 0, 1) : 0;
      const on = state.player.status !== 'stopped' && !!state.track;
      const target = on ? armAngle(R_OUT + (R_IN - R_OUT) * p) : armAngle(R_REST, -1);
      if (first) { theta = target; first = false; }
      theta += (target - theta) * (1 - Math.exp(-3.2 * dt));
      armEl.setAttribute('transform', `translate(${PIVOT[0]} ${PIVOT[1]}) rotate(${theta.toFixed(2)})`);
    }
  });
  const setCover = () => label.set(state.coverUrl);
  L.on('track', setCover); setCover();
  return { destroy: () => L.destroy() };
}

function art(host, c) {
  const L = lifecycle();
  const main = cover('art-cover');
  const g1 = h('img', { class: 'art-g g1', alt: '', draggable: 'false' }), g2 = h('img', { class: 'art-g g2', alt: '', draggable: 'false' });
  const frame = h('div', { class: 'art-frame' }, main.el, c.hero.halftone ? h('div', { class: 'art-halftone' }) : null, c.hero.glitch ? [g1, g2] : null);
  const root = h('div', { class: 'art' }, c.hero.sun ? h('div', { class: 'art-sun' }) : null, frame);
  host.append(root);
  const setCover = () => { main.set(state.coverUrl); if (state.coverUrl) { g1.src = state.coverUrl; g2.src = state.coverUrl; } };
  L.on('track', setCover); setCover();
  let glitchUntil = 0;
  if (c.hero.glitch || c.hero.sun) {
    L.add(watchLive((live) => {
      root.style.setProperty('--beat', live.beat.toFixed(3));
      root.style.setProperty('--bass', live.bass.toFixed(3));
      if (c.hero.glitch && live.beat > 0.9 && performance.now() > glitchUntil) {
        glitchUntil = performance.now() + 140;
        root.style.setProperty('--gx', `${((Math.random() - 0.5) * 16).toFixed(1)}px`);
        root.style.setProperty('--gy', `${Math.round(Math.random() * 70)}%`);
        root.classList.add('glitching');
        setTimeout(() => root.classList.remove('glitching'), 130);
      }
    }));
  }
  return { destroy: () => L.destroy() };
}

function vizHero(host) {
  const cv = h('canvas', { class: 'viz-canvas hero-canvas' });
  host.append(cv);
  const handle = mountViz(cv, { id: () => get('hero.viz') });
  return { destroy: () => handle.destroy() };
}

/**
 * A quiet centrepiece for skins whose hero is a visualiser: the album set large in the skin's own
 * display type. No picture, so nothing sits in the skin's design that the skin didn't draw.
 */
function quiet(host) {
  const L = lifecycle();
  const album = h('div', { class: 'quiet-album' }), meta = h('div', { class: 'quiet-meta' });
  host.append(h('div', { class: 'quiet' }, album, meta));
  const render = () => {
    const t = state.track;
    album.textContent = t ? t.album || t.title || '' : '';
    meta.textContent = t ? [t.artist, t.year].filter(Boolean).join('  ·  ') : '';
  };
  L.on('track', render); render();
  return { destroy: () => L.destroy() };
}

/** The centrepiece mode. With visualisers off, a visualiser hero becomes what the skin says (skins.js). */
const heroMode = (c) => (c.hero.mode === 'viz' && !featureOn('viz') ? (getSkin(getPrefs().skin).vizOff === 'quiet' ? 'quiet' : 'none') : c.hero.mode);

const MODES = { turntable, art, viz: vizHero, quiet, none: () => null };

export function mount(el) {
  const L = lifecycle();
  const host = h('div', { class: 'hero' });
  el.append(host);
  let sig = '', inner = null;
  function build() {
    const c = cfg();
    const s = [heroMode(c), c.hero.viz, c.hero.plinth, c.hero.sun, c.hero.halftone, c.hero.glitch, c.hero.tonearm].join('|');
    if (s === sig) return;
    sig = s;
    inner?.destroy(); clear(host);
    host.dataset.mode = heroMode(c);
    inner = (MODES[heroMode(c)] ?? MODES.art)(host, c) ?? null;
  }
  L.on('ui', build);
  build();
  return { destroy: () => { inner?.destroy(); L.destroy(); } };
}
