// The position bar: five looks (line, waveform, ruler, dots), scrubbing, and elapsed/total time.

import { api } from '../api.js';
import { act } from '../controller.js';
import { get } from '../prefs.js';
import { duration, position, state } from '../state.js';
import { tokens } from '../theme.js';
import { clamp, fmtTime, h } from '../util.js';
import { rgba } from '../viz/util.js';
import { lifecycle } from './common.js';

export function mount(el) {
  const L = lifecycle();
  const cur = h('span', { class: 's-cur' }, '0:00');
  const total = h('span', { class: 's-dur' }, '0:00');
  const cv = h('canvas', { class: 's-canvas' });
  const tip = h('div', { class: 's-tip', hidden: true });
  const wrap = h('div', { class: 's-track', role: 'slider', tabindex: '0', 'aria-label': 'Position', 'aria-valuemin': '0' }, cv, tip);
  el.append(h('div', { class: 'seek' }, cur, wrap, total));
  const ctx = cv.getContext('2d');
  let scrub = null, hover = null, W = 0, H = 0;

  const resize = () => {
    const r = wrap.getBoundingClientRect(), dpr = Math.min(devicePixelRatio || 1, 2);
    W = Math.max(1, Math.round(r.width)); H = Math.max(1, Math.round(r.height));
    cv.width = Math.round(W * dpr); cv.height = Math.round(H * dpr);
    cv.style.width = `${W}px`; cv.style.height = `${H}px`;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    draw();
  };
  new ResizeObserver(resize).observe(wrap);

  const progress = () => (scrub ?? (duration() > 0 ? clamp(position() / duration(), 0, 1) : 0));
  const peakAt = (f) => {
    const p = state.waveform?.peaks;
    if (!p?.length) return null;
    const i = f * (p.length - 1), k = Math.floor(i);
    return p[k] * (1 - (i - k)) + p[Math.min(p.length - 1, k + 1)] * (i - k);
  };

  // Skip frames where nothing visible changed: the bar moves a fraction of a pixel per frame.
  let lastKey = '';
  const tick = () => {
    const style = get('seek.style'), idle = !state.waveform && style === 'wave' ? Math.floor(performance.now() / 120) : 0;
    const key = `${style}|${W}|${H}|${Math.round(progress() * W * 2)}|${hover}|${scrub}|${idle}|${state.waveform ? 1 : 0}`;
    if (key === lastKey) { setText(); return; }
    draw();
  };
  const setText = () => {
    const secs = scrub != null ? scrub * duration() : position();
    cur.textContent = fmtTime(secs);
  };

  function draw() {
    const style = get('seek.style'), p = progress(), t = tokens;
    lastKey = `${style}|${W}|${H}|${Math.round(p * W * 2)}|${hover}|${scrub}|${!state.waveform && style === 'wave' ? Math.floor(performance.now() / 120) : 0}|${state.waveform ? 1 : 0}`;
    ctx.clearRect(0, 0, W, H);
    const played = t.accent, rest = rgba(t.fg, 0.28);
    const x = p * W, cy = H / 2;
    if (style === 'wave') {
      const bw = 3, gap = 2, n = Math.max(8, Math.floor(W / (bw + gap)));
      const has = !!state.waveform;
      for (let i = 0; i < n; i++) {
        const f = (i + 0.5) / n, v = has ? Math.sqrt(peakAt(f) ?? 0) : 0.14 + 0.08 * Math.sin(i * 0.6 + performance.now() / 500);
        const bh = Math.max(2, v * H * 0.92), bx = i * (bw + gap);
        ctx.fillStyle = bx + bw / 2 <= x ? played : rest;
        ctx.beginPath(); ctx.roundRect(bx, cy - bh / 2, bw, bh, bw / 2); ctx.fill();
      }
      ctx.fillStyle = t.fg; ctx.fillRect(Math.min(W - 2, Math.max(0, x - 1)), 0, 2, H);
    } else if (style === 'ticks') {
      const step = 6, n = Math.floor(W / step), dur = duration();
      const majorEvery = 5, labelEvery = Math.max(1, Math.ceil(80 / (step * majorEvery)));
      ctx.font = '10px "JetBrains Mono", monospace'; ctx.textBaseline = 'top';
      for (let i = 0; i <= n; i++) {
        const tx = i * step + 0.5, major = i % majorEvery === 0, isPlayed = tx <= x;
        ctx.strokeStyle = isPlayed ? played : rest; ctx.lineWidth = 1;
        ctx.beginPath(); ctx.moveTo(tx, H * 0.5); ctx.lineTo(tx, H * (major ? 0.9 : 0.72)); ctx.stroke();
        if (major && (i / majorEvery) % labelEvery === 0 && dur > 0) { ctx.fillStyle = rgba(t.fg, 0.55); ctx.fillText(fmtTime((tx / W) * dur), tx + 3, 1); }
      }
      ctx.strokeStyle = played; ctx.lineWidth = 1.5; ctx.beginPath(); ctx.moveTo(x, 0); ctx.lineTo(x, H); ctx.stroke();
      ctx.fillStyle = played; ctx.beginPath(); ctx.moveTo(x - 4, 0); ctx.lineTo(x + 4, 0); ctx.lineTo(x, 6); ctx.fill();
    } else if (style === 'dots') {
      const gap = 10, n = Math.max(6, Math.floor(W / gap));
      for (let i = 0; i < n; i++) {
        const dx = (i + 0.5) * (W / n), on = dx <= x;
        ctx.fillStyle = on ? played : rest; ctx.beginPath(); ctx.arc(dx, cy, on ? 2.6 : 2, 0, Math.PI * 2); ctx.fill();
      }
      ctx.fillStyle = played; ctx.beginPath(); ctx.arc(x, cy, 5, 0, Math.PI * 2); ctx.fill();
    } else {
      const th = hover != null || scrub != null ? 4 : 2.5;
      ctx.fillStyle = rest; ctx.beginPath(); ctx.roundRect(0, cy - th / 2, W, th, th / 2); ctx.fill();
      ctx.fillStyle = played; ctx.beginPath(); ctx.roundRect(0, cy - th / 2, Math.max(th, x), th, th / 2); ctx.fill();
      ctx.fillStyle = played; ctx.beginPath(); ctx.arc(clamp(x, 5, W - 5), cy, hover != null || scrub != null ? 6.5 : 5, 0, Math.PI * 2); ctx.fill();
    }
    if (hover != null && scrub == null && style !== 'ticks') { ctx.strokeStyle = rgba(t.fg, 0.4); ctx.lineWidth = 1; ctx.beginPath(); ctx.moveTo(hover * W, 0); ctx.lineTo(hover * W, H); ctx.stroke(); }
    const secs = scrub != null ? scrub * duration() : position();
    cur.textContent = fmtTime(secs);
    total.textContent = fmtTime(duration());
    wrap.setAttribute('aria-valuenow', String(Math.round(secs))); wrap.setAttribute('aria-valuemax', String(Math.round(duration())));
    wrap.setAttribute('aria-valuetext', `${fmtTime(secs)} of ${fmtTime(duration())}`);
  }

  const fracOf = (ev) => { const r = wrap.getBoundingClientRect(); return clamp((ev.clientX - r.left) / r.width, 0, 1); };
  wrap.addEventListener('pointerdown', (ev) => {
    if (ev.button !== 0 || !duration()) return;
    wrap.setPointerCapture(ev.pointerId); scrub = fracOf(ev); draw();
  });
  wrap.addEventListener('pointermove', (ev) => {
    hover = fracOf(ev);
    if (scrub != null) scrub = hover;
    tip.hidden = false; tip.textContent = fmtTime(hover * duration());
    tip.style.left = `${clamp(hover * W, 20, W - 20)}px`;
    draw();
  });
  const finish = (ev) => {
    if (scrub == null) return;
    const target = scrub * duration(); scrub = null;
    state.player.position_secs = target; state.stamp = performance.now(); // show it at once
    act(api.seek(target)); draw();
  };
  wrap.addEventListener('pointerup', finish);
  wrap.addEventListener('pointercancel', () => { scrub = null; draw(); });
  wrap.addEventListener('pointerleave', () => { hover = null; tip.hidden = true; draw(); });
  wrap.addEventListener('keydown', (ev) => {
    const d = { ArrowRight: 5, ArrowLeft: -5, PageUp: 30, PageDown: -30 }[ev.key];
    if (d !== undefined) { ev.preventDefault(); act(api.seek(clamp(position() + d, 0, duration()))); }
  });

  L.frame(tick, { whenPlaying: true });
  L.on(['status', 'track', 'waveform', 'ui', 'theme', 'player'], () => { if (state.player.status !== 'playing') draw(); });
  L.on('ui', () => { el.dataset.style = get('seek.style'); wrap.parentElement.classList.toggle('no-times', !get('seek.times')); });
  el.dataset.style = get('seek.style');
  wrap.parentElement.classList.toggle('no-times', !get('seek.times'));
  return { destroy: () => L.destroy() };
}
