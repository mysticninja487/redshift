// Visualisers drawn from the frequency spectrum: bars, dots, ring, level meters, spectrogram, ridges.

import { rgba, ease, keep, resample } from './util.js';
import { parseHex, mixHex } from '../color.js';

export const bars = {
  id: 'bars', name: 'Bars', blurb: 'Classic spectrum bars with peak caps.',
  options: [
    { key: 'count', label: 'Bars', type: 'range', min: 8, max: 128, step: 1, def: 48 },
    { key: 'gap', label: 'Gap', type: 'range', min: 0, max: 0.8, step: 0.05, def: 0.35 },
    { key: 'round', label: 'Rounded', type: 'toggle', def: true },
    { key: 'peaks', label: 'Peak caps', type: 'toggle', def: true },
    { key: 'mirror', label: 'Mirror', type: 'toggle', def: false },
  ],
  draw(ctx, e, o) {
    const { w, h } = e;
    const v = resample(e.frame.bars, o.count), pk = resample(e.frame.peaks, o.count);
    const slot = w / o.count, bw = Math.max(1, slot * (1 - o.gap));
    ctx.fillStyle = e.paint(0, h, 0, 0);
    for (let i = 0; i < o.count; i++) {
      const x = i * slot + (slot - bw) / 2;
      const bh = Math.max(o.mirror ? 1 : 2, v[i] * h * (o.mirror ? 0.46 : 0.96));
      ctx.beginPath();
      if (o.mirror) ctx.roundRect(x, h / 2 - bh, bw, bh * 2, o.round ? bw / 2 : 0);
      else ctx.roundRect(x, h - bh, bw, bh, o.round ? [bw / 2, bw / 2, 0, 0] : 0);
      ctx.fill();
    }
    if (o.peaks) {
      ctx.fillStyle = e.tokens.fg;
      for (let i = 0; i < o.count; i++) {
        const x = i * slot + (slot - bw) / 2, y = o.mirror ? h / 2 - pk[i] * h * 0.46 - 3 : h - pk[i] * h * 0.96 - 3;
        if (pk[i] > 0.02) ctx.fillRect(x, y, bw, 2);
        if (o.mirror && pk[i] > 0.02) ctx.fillRect(x, h - y - 2, bw, 2);
      }
    }
  },
};

export const dots = {
  id: 'dots', name: 'Dot matrix', blurb: 'An LED grid that lights up from the bottom.',
  options: [
    { key: 'cols', label: 'Columns', type: 'range', min: 8, max: 64, step: 1, def: 28 },
    { key: 'rows', label: 'Rows', type: 'range', min: 4, max: 32, step: 1, def: 14 },
    { key: 'size', label: 'Dot size', type: 'range', min: 0.2, max: 0.5, step: 0.02, def: 0.34 },
    { key: 'peaks', label: 'Peak dots', type: 'toggle', def: true },
  ],
  draw(ctx, e, o) {
    const { w, h } = e;
    const v = resample(e.frame.bars, o.cols), pk = resample(e.frame.peaks, o.cols);
    const cell = Math.min(w / o.cols, h / o.rows), r = cell * o.size;
    const ox = (w - cell * o.cols) / 2, oy = (h - cell * o.rows) / 2;
    const lit = e.paint(ox, oy + cell * o.rows, ox, oy);
    for (let c = 0; c < o.cols; c++) {
      const level = v[c] * o.rows, peakRow = Math.round(pk[c] * o.rows);
      for (let r0 = 0; r0 < o.rows; r0++) {
        const on = r0 + 0.5 < level;
        const isPeak = o.peaks && r0 === peakRow - 1 && peakRow > 1;
        ctx.beginPath();
        ctx.arc(ox + c * cell + cell / 2, oy + (o.rows - 1 - r0) * cell + cell / 2, r, 0, Math.PI * 2);
        ctx.fillStyle = on ? lit : isPeak ? e.tokens.fg : rgba(mixHex(e.tokens.fg, e.tokens.bg, 0.5), 0.35);
        ctx.fill();
      }
    }
  },
};

export const ring = {
  id: 'ring', name: 'Ring', blurb: 'Spectrum around a circle, with the cover in the middle.',
  options: [
    { key: 'count', label: 'Bars', type: 'range', min: 24, max: 180, step: 2, def: 96 },
    { key: 'spin', label: 'Rotation', type: 'range', min: -0.6, max: 0.6, step: 0.02, def: 0.08 },
    { key: 'inner', label: 'Inner radius', type: 'range', min: 0.15, max: 0.4, step: 0.01, def: 0.27 },
    { key: 'cover', label: 'Cover in centre', type: 'toggle', def: true },
    { key: 'mirror', label: 'Inward bars', type: 'toggle', def: false },
  ],
  draw(ctx, e, o) {
    const { w, h } = e, cx = w / 2, cy = h / 2, m = Math.min(w, h);
    const R0 = m * o.inner * (1 + e.bass * 0.05 + e.beat * 0.03), maxLen = m * (0.5 - o.inner) * 0.92;
    const v = resample(e.frame.bars, o.count);
    const rot = e.t * o.spin * Math.PI * 2 - Math.PI / 2;
    ctx.lineCap = 'round';
    ctx.lineWidth = Math.max(1.5, ((Math.PI * 2 * R0) / o.count) * 0.52);
    ctx.strokeStyle = e.paint(cx - m / 2, cy - m / 2, cx + m / 2, cy + m / 2);
    ctx.beginPath();
    for (let i = 0; i < o.count; i++) {
      // The spectrum wraps symmetrically so there is no seam where it starts and ends.
      const k = i < o.count / 2 ? i * 2 : (o.count - 1 - i) * 2;
      const a = rot + (i / o.count) * Math.PI * 2, len = 3 + v[Math.min(o.count - 1, k)] * maxLen;
      ctx.moveTo(cx + Math.cos(a) * R0, cy + Math.sin(a) * R0);
      ctx.lineTo(cx + Math.cos(a) * (R0 + len), cy + Math.sin(a) * (R0 + len));
      if (o.mirror) { ctx.moveTo(cx + Math.cos(a) * (R0 - 4), cy + Math.sin(a) * (R0 - 4)); ctx.lineTo(cx + Math.cos(a) * (R0 - 4 - len * 0.35), cy + Math.sin(a) * (R0 - 4 - len * 0.35)); }
    }
    ctx.stroke();
    ctx.save();
    ctx.beginPath(); ctx.arc(cx, cy, Math.max(1, R0 - 8), 0, Math.PI * 2); ctx.clip();
    if (o.cover && e.coverImg?.complete && e.coverImg.naturalWidth) {
      ctx.translate(cx, cy); ctx.rotate(e.t * o.spin * Math.PI * 2 * 0.5);
      const s = Math.max(2, (R0 - 8) * 2), img = e.coverImg, iw = img.naturalWidth, ih = img.naturalHeight, side = Math.min(iw, ih);
      // Fill the circle with the middle of the picture, so a cover that isn't square isn't squeezed.
      ctx.drawImage(img, (iw - side) / 2, (ih - side) / 2, side, side, -s / 2, -s / 2, s, s);
    } else { ctx.fillStyle = rgba(e.tokens.accent, 0.12); ctx.fillRect(0, 0, w, h); }
    ctx.restore();
  },
};

export const vu = {
  id: 'vu', name: 'Level meters', blurb: 'Left and right levels in segments, with peak hold.',
  options: [
    { key: 'segments', label: 'Segments', type: 'range', min: 12, max: 60, step: 1, def: 32 },
    { key: 'vertical', label: 'Vertical', type: 'toggle', def: false },
  ],
  draw(ctx, e, o) {
    const { w, h } = e, st = keep(e.store, 'vu', () => ({ l: 0, r: 0, pl: 0, pr: 0, hl: 0, hr: 0 }));
    const db = (x) => Math.min(1, Math.max(0, (20 * Math.log10(Math.max(x, 1e-4)) + 54) / 54));
    st.l = ease(st.l, db(e.frame.rms_left), e.dt, 30, 6); st.r = ease(st.r, db(e.frame.rms_right), e.dt, 30, 6);
    const pl = db(e.frame.peak_left), pr = db(e.frame.peak_right);
    st.hl = pl >= st.hl ? pl : Math.max(0, st.hl - e.dt * 0.25); st.hr = pr >= st.hr ? pr : Math.max(0, st.hr - e.dt * 0.25);
    const n = o.segments, off = rgba(mixHex(e.tokens.fg, e.tokens.bg, 0.6), 0.28);
    const lanes = [[st.l, st.hl], [st.r, st.hr]];
    const lane = (o.vertical ? w : h) / 2, gap = lane * 0.22, thick = lane - gap, len = o.vertical ? h : w, seg = len / n;
    lanes.forEach(([lv, pk], li) => {
      for (let i = 0; i < n; i++) {
        const on = (i + 0.5) / n < lv, hot = i / n > 0.84;
        ctx.fillStyle = on ? (hot ? e.tokens.accent2 : e.tokens.accent) : (Math.round(pk * n) - 1 === i ? e.tokens.fg : off);
        if (o.vertical) ctx.fillRect(li * lane + gap / 2, len - (i + 1) * seg + 1, thick, seg - 2);
        else ctx.fillRect(i * seg + 1, li * lane + gap / 2, seg - 2, thick);
      }
    });
  },
};

export const spectrogram = {
  id: 'spectrogram', name: 'Spectrogram', blurb: 'Frequency against time, scrolling left like a printout.',
  options: [
    { key: 'speed', label: 'Scroll speed', type: 'range', min: 1, max: 4, step: 1, def: 2 },
    { key: 'gain', label: 'Brightness', type: 'range', min: 0.6, max: 2, step: 0.05, def: 1.15 },
  ],
  draw(ctx, e, o) {
    const { w, h } = e, rows = e.frame.bars.length;
    const st = keep(e.store, 'sg', () => ({ cv: null, key: '' }));
    const cols = Math.ceil(w / 2);
    const key = `${cols}x${rows}:${e.tokens.accent}:${e.tokens.fg}`;
    if (!st.cv || st.key !== key) {
      st.cv = document.createElement('canvas'); st.cv.width = cols; st.cv.height = rows; st.key = key;
      const [ar, ag, ab] = parseHex(e.tokens.accent), [fr, fg, fb] = parseHex(e.tokens.fg);
      st.ramp = Array.from({ length: 256 }, (_, i) => {
        const t = i / 255;
        if (t < 0.6) return `rgba(${ar},${ag},${ab},${(t / 0.6) ** 1.4 * 0.95})`;
        const k = (t - 0.6) / 0.4;
        return `rgba(${Math.round(ar + (fr - ar) * k)},${Math.round(ag + (fg - ag) * k)},${Math.round(ab + (fb - ab) * k)},0.95)`;
      });
    }
    const c = st.cv.getContext('2d');
    c.globalCompositeOperation = 'copy';
    c.drawImage(st.cv, -o.speed, 0);
    c.globalCompositeOperation = 'source-over';
    c.clearRect(cols - o.speed, 0, o.speed, rows);
    for (let i = 0; i < rows; i++) {
      const v = Math.min(1, e.frame.bars[i] * o.gain);
      if (v < 0.03) continue;
      c.fillStyle = st.ramp[Math.round(v * 255)];
      c.fillRect(cols - o.speed, rows - 1 - i, o.speed, 1);
    }
    ctx.imageSmoothingEnabled = true;
    ctx.drawImage(st.cv, 0, 0, w, h);
  },
};

export const ridges = {
  id: 'ridges', name: 'Ridges', blurb: 'Stacked spectrum lines receding into the distance.',
  options: [
    { key: 'lines', label: 'Lines', type: 'range', min: 8, max: 48, step: 1, def: 26 },
    { key: 'height', label: 'Height', type: 'range', min: 0.1, max: 0.5, step: 0.01, def: 0.26 },
  ],
  draw(ctx, e, o) {
    const { w, h } = e, n = 72, hist = e.spec;
    if (hist.length < 2) return;
    const stride = Math.max(1, Math.floor(hist.length / o.lines));
    ctx.lineWidth = 1.4; ctx.lineJoin = 'round';
    for (let li = 0; li < o.lines; li++) {
      // Oldest line first (at the back), newest (front) drawn last.
      const frame = hist[Math.max(0, hist.length - 1 - (o.lines - 1 - li) * stride)];
      const v = resample(frame, n);
      const base = h * 0.18 + (li / (o.lines - 1)) * h * 0.74;
      const pts = [];
      for (let i = 0; i < n; i++) {
        const x = (i / (n - 1)) * w, edge = Math.sin((i / (n - 1)) * Math.PI) ** 0.8;
        pts.push([x, base - v[i] * h * o.height * edge]);
      }
      const path = new Path2D();
      path.moveTo(pts[0][0], pts[0][1]);
      for (let i = 1; i < n - 1; i++) path.quadraticCurveTo(pts[i][0], pts[i][1], (pts[i][0] + pts[i + 1][0]) / 2, (pts[i][1] + pts[i + 1][1]) / 2);
      path.lineTo(pts[n - 1][0], pts[n - 1][1]);
      const fill = new Path2D(path);
      fill.lineTo(w, h + 2); fill.lineTo(0, h + 2); fill.closePath();
      ctx.globalCompositeOperation = 'destination-out'; // hides the lines behind this one
      ctx.fill(fill);
      ctx.globalCompositeOperation = 'source-over';
      const front = li / (o.lines - 1);
      ctx.strokeStyle = li === o.lines - 1 ? e.paint(0, 0, w, 0) : rgba(e.tokens.fg, 0.25 + front * 0.6);
      ctx.stroke(path);
    }
  },
};
