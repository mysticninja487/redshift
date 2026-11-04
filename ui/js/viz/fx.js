// Atmospheric visualisers: the glowing orb on its orbit, and the particle burst.

import { alpha, lighten, mixHex } from '../color.js';
import { cfg } from '../prefs.js';
import { ease, keep, rgba } from './util.js';

export const orb = {
  id: 'orb', name: 'Orb', blurb: 'A glowing sphere that swells with the bass, with a ring orbiting it.',
  options: [
    { key: 'size', label: 'Size', type: 'range', min: 0.15, max: 0.42, step: 0.01, def: 0.27 },
    { key: 'orbit', label: 'Orbit rings', type: 'range', min: 0, max: 3, step: 1, def: 2 },
    { key: 'speed', label: 'Orbit speed', type: 'range', min: 0, max: 2, step: 0.05, def: 0.6 },
    { key: 'grain', label: 'Halftone shading', type: 'toggle', def: true },
    { key: 'scale', label: 'Progress ticks', type: 'toggle', def: true },
  ],
  draw(ctx, e, o) {
    const { w, h } = e, cx = w / 2, cy = h / 2 - (o.scale ? h * 0.02 : 0), m = Math.min(w, h);
    const st = keep(e.store, 'orb', () => ({ s: 0, hi: 0 }));
    st.s = ease(st.s, e.bass, e.dt, 25, 7);
    st.hi = ease(st.hi, (e.frame.bars.slice(-24).reduce((a, b) => a + b, 0) / 24) || 0, e.dt, 20, 6);
    const R = m * o.size * (1 + st.s * 0.1 + e.beat * 0.035), a = e.tokens.accent;
    const glow = cfg().color.glow;

    // Halo
    const halo = ctx.createRadialGradient(cx, cy, R * 0.6, cx, cy, R * (1.9 + st.s * 0.6));
    halo.addColorStop(0, alpha(a, 0.5 * glow + 0.08)); halo.addColorStop(1, alpha(a, 0));
    ctx.fillStyle = halo; ctx.beginPath(); ctx.arc(cx, cy, R * 2.4, 0, Math.PI * 2); ctx.fill();

    // Rings behind the sphere, then the sphere, then rings in front.
    const rings = [];
    for (let k = 0; k < o.orbit; k++) rings.push({ rx: R * (1.55 + k * 0.38), ry: R * (0.5 + k * 0.14), rot: -0.34 + k * 0.5, k });
    const drawRing = (r, front) => {
      ctx.save(); ctx.translate(cx, cy); ctx.rotate(r.rot);
      ctx.strokeStyle = alpha(a, r.k ? 0.35 : 0.8); ctx.lineWidth = r.k ? 1 : 1.4;
      ctx.beginPath();
      ctx.ellipse(0, 0, r.rx, r.ry, 0, front ? 0 : Math.PI, front ? Math.PI : Math.PI * 2); ctx.stroke();
      if (front && r.k === 0) {
        // Chevrons riding the orbit
        ctx.fillStyle = alpha(a, 0.9);
        for (let i = 0; i < 3; i++) {
          const th = e.t * o.speed + i * 0.5 + (r.k * 1.3), x = Math.cos(th) * r.rx, y = Math.sin(th) * r.ry;
          const dx = -Math.sin(th) * r.rx, dy = Math.cos(th) * r.ry, ang = Math.atan2(dy, dx);
          ctx.save(); ctx.translate(x, y); ctx.rotate(ang); ctx.beginPath(); ctx.moveTo(-5, -4); ctx.lineTo(3, 0); ctx.lineTo(-5, 4); ctx.lineWidth = 1.3; ctx.strokeStyle = a; ctx.stroke(); ctx.restore();
        }
      }
      ctx.restore();
    };
    rings.forEach((r) => drawRing(r, false));

    const g = ctx.createRadialGradient(cx - R * 0.35, cy - R * 0.4, R * 0.1, cx, cy, R);
    g.addColorStop(0, lighten(a, 0.1)); g.addColorStop(0.55, a); g.addColorStop(1, mixHex(a, '#000000', 0.62));
    ctx.save();
    ctx.shadowColor = alpha(a, 0.55 * glow + 0.1); ctx.shadowBlur = R * 0.5;
    ctx.fillStyle = g; ctx.beginPath(); ctx.arc(cx, cy, R, 0, Math.PI * 2); ctx.fill();
    ctx.restore();
    if (o.grain) {
      ctx.save(); ctx.beginPath(); ctx.arc(cx, cy, R, 0, Math.PI * 2); ctx.clip();
      ctx.fillStyle = rgba('#000000', 0.28);
      const step = Math.max(5, R / 16);
      for (let y = -R; y < R; y += step) for (let x = -R; x < R; x += step) {
        const d = (x * 0.8 + y * 0.6) / R; // darker towards the lower right
        if (d > 0.1) { ctx.beginPath(); ctx.arc(cx + x + ((Math.round(y / step) % 2) * step) / 2, cy + y, Math.max(0.4, (d - 0.1) * step * 0.5), 0, Math.PI * 2); ctx.fill(); }
      }
      ctx.restore();
    }
    rings.forEach((r) => drawRing(r, true));

    if (o.scale) {
      const n = 41, tw = Math.min(w * 0.5, m * 0.9), y = cy + R * 2.2 + 8, p = e.duration > 0 ? e.position / e.duration : 0;
      ctx.lineWidth = 1;
      for (let i = 0; i < n; i++) {
        const x = cx - tw / 2 + (i / (n - 1)) * tw, big = i % 5 === 0;
        ctx.strokeStyle = i / (n - 1) <= p ? a : rgba(e.tokens.fg, 0.3);
        ctx.beginPath(); ctx.moveTo(x, y); ctx.lineTo(x, y + (big ? 9 : 5)); ctx.stroke();
      }
    }
  },
};

export const particles = {
  id: 'particles', name: 'Particles', blurb: 'Sparks thrown off the centre on every beat.',
  options: [
    { key: 'count', label: 'Amount', type: 'range', min: 60, max: 600, step: 10, def: 260 },
    { key: 'speed', label: 'Speed', type: 'range', min: 0.3, max: 2.5, step: 0.05, def: 1 },
    { key: 'size', label: 'Size', type: 'range', min: 0.5, max: 3, step: 0.1, def: 1.4 },
  ],
  draw(ctx, e, o) {
    const { w, h } = e, cx = w / 2, cy = h / 2, m = Math.min(w, h);
    const st = keep(e.store, 'pt', () => ({ ps: [] }));
    const spawn = (n) => {
      for (let i = 0; i < n && st.ps.length < o.count; i++) {
        const a = Math.random() * Math.PI * 2, v = (0.12 + Math.random() * 0.4) * m * o.speed;
        st.ps.push({ x: cx + Math.cos(a) * m * 0.06, y: cy + Math.sin(a) * m * 0.06, vx: Math.cos(a) * v, vy: Math.sin(a) * v, life: 1, decay: 0.25 + Math.random() * 0.5, r: (0.6 + Math.random()) * o.size });
      }
    };
    if (e.beat > 0.9) spawn(14 + Math.round(e.bass * 40));
    spawn(Math.round(e.energy * 90 * e.dt * 60 / 60));
    ctx.fillStyle = e.paint(0, 0, w, h);
    const dark = e.scheme === 'dark';
    if (dark) ctx.globalCompositeOperation = 'lighter';
    ctx.shadowColor = e.tokens.accent; ctx.shadowBlur = 8;
    st.ps = st.ps.filter((p) => {
      p.life -= p.decay * e.dt;
      if (p.life <= 0) return false;
      const sw = 0.3 * Math.sin(e.t * 1.6 + p.x * 0.01);
      p.vx = p.vx * 0.985 - p.vy * sw * e.dt; p.vy = p.vy * 0.985 + p.vx * sw * e.dt;
      p.x += p.vx * e.dt; p.y += p.vy * e.dt;
      ctx.globalAlpha = Math.min(1, p.life * 1.4);
      ctx.beginPath(); ctx.arc(p.x, p.y, p.r * (0.5 + p.life), 0, Math.PI * 2); ctx.fill();
      return true;
    });
    ctx.globalAlpha = 1; ctx.shadowBlur = 0; ctx.globalCompositeOperation = 'source-over';
    // A soft core so it never looks empty when the music is quiet.
    const g = ctx.createRadialGradient(cx, cy, 0, cx, cy, m * (0.08 + e.bass * 0.05));
    g.addColorStop(0, rgba(e.tokens.accent, 0.5)); g.addColorStop(1, rgba(e.tokens.accent, 0));
    ctx.fillStyle = g; ctx.beginPath(); ctx.arc(cx, cy, m * 0.2, 0, Math.PI * 2); ctx.fill();
  },
};
