// Visualisers drawn from the waveform: the whole-track hairline view, oscilloscope, vectorscope
// and the blurred "wavelength" pods.

import { getPrefs } from '../prefs.js';
import { rgba, ease, keep, resample, smoothstep } from './util.js';

export const lines = {
  id: 'lines', name: 'Hairlines', blurb: 'The whole song as fine vertical lines scrolling past a playhead, so you see what is coming.',
  options: [
    { key: 'gap', label: 'Line spacing', type: 'range', min: 3, max: 14, step: 1, def: 6 },
    { key: 'window', label: 'Seconds in view', type: 'range', min: 6, max: 90, step: 1, def: 28 },
    { key: 'head', label: 'Playhead position', type: 'range', min: 0.1, max: 0.9, step: 0.01, def: 0.3 },
    { key: 'height', label: 'Height', type: 'range', min: 0.2, max: 0.5, step: 0.01, def: 0.42 },
    { key: 'playhead', label: 'Show playhead', type: 'toggle', def: true },
  ],
  draw(ctx, e, o) {
    const { w, h } = e, cy = h / 2, headX = w * o.head, pps = w / o.window;
    const wf = e.waveform?.peaks, dur = e.duration;
    const st = keep(e.store, 'lines', () => ({ pulse: 0 }));
    st.pulse = ease(st.pulse, e.bass, e.dt, 30, 10);

    const base = Math.floor((e.position * pps) / o.gap) * o.gap / pps, shift = (e.position - base) * pps;
    const half = h * o.height;
    const fg = e.tokens.fg, gkey = `${Math.round(cy - half)}|${Math.round(half)}|${fg}`;
    if (st.gkey !== gkey) { // the fade is the same every frame, so build it once
      const g = ctx.createLinearGradient(0, cy - half, 0, cy + half);
      g.addColorStop(0, rgba(fg, 0)); g.addColorStop(0.5, rgba(fg, 1)); g.addColorStop(1, rgba(fg, 0));
      st.grad = g; st.gkey = gkey;
    }
    ctx.lineCap = 'round';
    ctx.lineWidth = Math.max(1, o.gap > 8 ? 1.6 : 1.2);
    const from = Math.floor(-headX / o.gap) - 1, to = Math.ceil((w - headX) / o.gap) + 1;
    // Past lines are bright, lines still to come are dimmer: one pass builds both paths.
    const pastPath = new Path2D(), nextPath = new Path2D();
    for (let i = from; i <= to; i++) {
      const x = headX + i * o.gap - shift;
      if (x < -2 || x > w + 2) continue;
      const t = base + (i * o.gap) / pps;
      let a;
      if (wf && dur > 0) {
        const idx = (t / dur) * (wf.length - 1);
        if (idx < 0 || idx > wf.length - 1) a = 0.02;
        else { const k = Math.floor(idx), f = idx - k; a = wf[k] * (1 - f) + (wf[Math.min(wf.length - 1, k + 1)] ?? 0) * f; }
      } else {
        // No overview yet: draw the recent loudness history instead.
        const back = Math.round(((headX - x) / pps) * 60);
        a = back >= 0 ? (e.levels[e.levels.length - 1 - back] ?? 0) * 2.2 : 0.02;
      }
      a = Math.min(1, Math.sqrt(a)) * (0.06 + 0.94 * smoothstep(0, 0.08, x / w) * smoothstep(0, 0.08, 1 - x / w));
      const near = Math.exp(-(((x - headX) / (o.gap * 5)) ** 2));
      const hh = Math.max(1.5, a * half * (1 + near * st.pulse * 0.45));
      const path = i * o.gap - shift <= 0 ? pastPath : nextPath;
      path.moveTo(x, cy - hh); path.lineTo(x, cy + hh);
    }
    ctx.strokeStyle = st.grad;
    ctx.globalAlpha = 0.95; ctx.stroke(pastPath);
    ctx.globalAlpha = 0.5; ctx.stroke(nextPath);
    ctx.globalAlpha = 1;
    if (o.playhead) {
      ctx.strokeStyle = e.tokens.accent; ctx.lineWidth = 1.5;
      ctx.beginPath(); ctx.moveTo(headX, cy - half * 1.08); ctx.lineTo(headX, cy + half * 1.08); ctx.stroke();
    }
  },
};

export const wave = {
  id: 'wave', name: 'Oscilloscope', blurb: 'The live waveform, with an optional glow.',
  options: [
    { key: 'thickness', label: 'Line weight', type: 'range', min: 1, max: 6, step: 0.5, def: 2 },
    { key: 'glow', label: 'Glow', type: 'range', min: 0, max: 1, step: 0.05, def: 0.5 },
    { key: 'split', label: 'Stack left and right', type: 'toggle', def: false },
    { key: 'gain', label: 'Gain', type: 'range', min: 0.5, max: 4, step: 0.1, def: 1.6 },
  ],
  draw(ctx, e, o) {
    const { w, h } = e;
    const lanes = o.split ? [[e.frame.scope_left, h * 0.25], [e.frame.scope_right, h * 0.75]] : [[e.frame.scope_left, h / 2], [e.frame.scope_right, h / 2]];
    ctx.lineJoin = 'round';
    ctx.strokeStyle = e.paint(0, 0, w, 0);
    ctx.lineWidth = o.thickness;
    if (o.glow > 0 && !getPrefs().perf?.lite) { ctx.shadowColor = e.tokens.accent; ctx.shadowBlur = 18 * o.glow; }
    lanes.forEach(([data, cy], i) => {
      const amp = (o.split ? h * 0.22 : h * 0.4) * o.gain / 1.6;
      ctx.globalAlpha = i === 0 || o.split ? 1 : 0.45;
      ctx.beginPath();
      const n = data.length || 1;
      for (let k = 0; k < n; k++) { const x = (k / (n - 1)) * w, y = cy - Math.max(-1, Math.min(1, data[k])) * amp; k ? ctx.lineTo(x, y) : ctx.moveTo(x, y); }
      ctx.stroke();
    });
    ctx.globalAlpha = 1; ctx.shadowBlur = 0;
    ctx.strokeStyle = rgba(e.tokens.fg, 0.14); ctx.lineWidth = 1;
    ctx.beginPath(); (o.split ? [h * 0.25, h * 0.75] : [h / 2]).forEach((y) => { ctx.moveTo(0, y); ctx.lineTo(w, y); }); ctx.stroke();
  },
};

export const lissajous = {
  id: 'lissajous', name: 'Vectorscope', blurb: 'Left against right: mono is a line, wide stereo fills the screen.',
  persist: true,
  options: [
    { key: 'fade', label: 'Trail length', type: 'range', min: 0.04, max: 0.5, step: 0.01, def: 0.16 },
    { key: 'glow', label: 'Glow', type: 'range', min: 0, max: 1, step: 0.05, def: 0.6 },
    { key: 'gain', label: 'Gain', type: 'range', min: 0.5, max: 4, step: 0.1, def: 1.8 },
  ],
  draw(ctx, e, o) {
    const { w, h } = e, cx = w / 2, cy = h / 2, R = Math.min(w, h) * 0.46 * o.gain / 1.6;
    ctx.globalCompositeOperation = 'destination-out';
    ctx.fillStyle = `rgba(0,0,0,${o.fade})`; ctx.fillRect(0, 0, w, h);
    ctx.globalCompositeOperation = 'source-over';
    const L = e.frame.scope_left, Rr = e.frame.scope_right, n = Math.min(L.length, Rr.length);
    ctx.strokeStyle = e.paint(cx - R, cy - R, cx + R, cy + R);
    ctx.lineWidth = 1.6; ctx.lineJoin = 'round';
    if (o.glow > 0 && !getPrefs().perf?.lite) { ctx.shadowColor = e.tokens.accent; ctx.shadowBlur = 16 * o.glow; }
    ctx.beginPath();
    for (let i = 0; i < n; i++) {
      const x = cx + ((L[i] - Rr[i]) * 0.7071) * R, y = cy - ((L[i] + Rr[i]) * 0.7071) * R;
      i ? ctx.lineTo(x, y) : ctx.moveTo(x, y);
    }
    ctx.stroke();
  },
};

export const blur = {
  id: 'blur', name: 'Wavelength', blurb: 'Soft vertical pods along a horizontal axis. Dark over light, light over dark.',
  blend: 'difference',
  options: [
    { key: 'pods', label: 'Pods', type: 'range', min: 8, max: 64, step: 1, def: 30 },
    { key: 'blur', label: 'Softness', type: 'range', min: 0, max: 30, step: 1, def: 14 },
    { key: 'height', label: 'Height', type: 'range', min: 0.2, max: 0.5, step: 0.01, def: 0.4 },
    { key: 'axis', label: 'Axis line', type: 'toggle', def: true },
  ],
  draw(ctx, e, o) {
    const { w, h } = e, cy = h / 2;
    const st = keep(e.store, 'blur', () => ({ v: new Float32Array(64) }));
    const target = resample(e.frame.bars, o.pods);
    const idle = 0.05 + 0.04 * Math.sin(e.t * 0.8);
    const vs = [];
    for (let i = 0; i < o.pods; i++) {
      // A bell-shaped weighting keeps the middle taller, like the pods in a printed waveform.
      const bell = 0.35 + 0.65 * Math.exp(-(((i / (o.pods - 1) - 0.5) / 0.32) ** 2));
      st.v[i] = ease(st.v[i], Math.max(idle * bell, target[i] * bell), e.dt, 20, 5);
      vs.push(st.v[i]);
    }
    const slot = w / o.pods;
    // The soft pods are drawn small and blurred once, then scaled up: a blur is low-frequency
    // anyway, and a filter on every pod of a full-size canvas costs a whole surface each.
    const sc = 0.25, lw = Math.max(8, Math.round(w * sc)), lh = Math.max(8, Math.round(h * sc));
    const layer = keep(e.store, 'blurLayers', () => ({ a: document.createElement('canvas'), b: document.createElement('canvas') }));
    if (layer.a.width !== lw || layer.a.height !== lh) { layer.a.width = layer.b.width = lw; layer.a.height = layer.b.height = lh; }
    const a = layer.a.getContext('2d'), b = layer.b.getContext('2d');
    a.setTransform(sc, 0, 0, sc, 0, 0); a.clearRect(0, 0, w, h);
    a.fillStyle = '#fff';
    for (let i = 0; i < o.pods; i++) {
      const hh = Math.max(2, vs[i] * h * o.height), ww = slot * (0.26 + vs[i] * 0.4);
      a.beginPath(); a.ellipse((i + 0.5) * slot, cy, ww, hh, 0, 0, Math.PI * 2); a.fill();
    }
    b.setTransform(1, 0, 0, 1, 0, 0); b.clearRect(0, 0, lw, lh);
    b.filter = o.blur > 0 ? `blur(${(o.blur * sc).toFixed(2)}px)` : 'none';
    b.drawImage(layer.a, 0, 0);
    b.filter = 'none';
    ctx.imageSmoothingEnabled = true; ctx.imageSmoothingQuality = 'high';
    ctx.drawImage(layer.b, 0, 0, w, h);
    ctx.strokeStyle = '#fff'; ctx.lineWidth = 1;
    ctx.globalAlpha = 0.55;
    ctx.beginPath();
    for (let i = 0; i < o.pods; i += 2) { const hh = Math.max(2, vs[i] * h * o.height * 0.9); ctx.moveTo((i + 0.5) * slot, cy - hh); ctx.lineTo((i + 0.5) * slot, cy + hh); }
    ctx.stroke();
    if (o.axis) { ctx.globalAlpha = 0.8; ctx.beginPath(); ctx.moveTo(0, cy); ctx.lineTo(w, cy); ctx.stroke(); }
    ctx.globalAlpha = 1;
  },
};
