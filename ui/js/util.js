// Small helpers shared by everything. No dependencies.

export const clamp = (x, lo, hi) => Math.min(hi, Math.max(lo, x));
export const lerp = (a, b, t) => a + (b - a) * t;
export const mix = lerp;
export const isObj = (v) => v !== null && typeof v === 'object' && !Array.isArray(v);

/** m:ss, or h:mm:ss for long tracks. Unknown values show as --:--. */
export function fmtTime(secs) {
  if (!Number.isFinite(secs) || secs < 0) return '--:--';
  const s = Math.floor(secs);
  const h = Math.floor(s / 3600), m = Math.floor((s % 3600) / 60), r = s % 60;
  const pad = (n) => String(n).padStart(2, '0');
  return h ? `${h}:${pad(m)}:${pad(r)}` : `${m}:${pad(r)}`;
}

/** "44.1 kHz · 16-bit"-style technical line parts, skipping what's unknown. */
export function fmtKhz(hz) { return hz ? `${+(hz / 1000).toFixed(1)} kHz` : ''; }

export function debounce(fn, ms) {
  let t;
  const d = (...a) => { clearTimeout(t); t = setTimeout(() => fn(...a), ms); };
  d.flush = (...a) => { clearTimeout(t); fn(...a); };
  return d;
}

export function deepMerge(base, over) {
  if (!isObj(base) || !isObj(over)) return over === undefined ? base : over;
  const out = { ...base };
  for (const k of Object.keys(over)) out[k] = isObj(base[k]) && isObj(over[k]) ? deepMerge(base[k], over[k]) : over[k];
  return out;
}

export const clone = (v) => (v === undefined ? v : JSON.parse(JSON.stringify(v)));

export function getPath(obj, path) {
  let cur = obj;
  for (const k of path.split('.')) { if (cur == null) return undefined; cur = cur[k]; }
  return cur;
}

/** Returns a copy of `obj` with `path` set (creating objects on the way). `undefined` deletes. */
export function setPath(obj, path, value) {
  const keys = path.split('.');
  const root = isObj(obj) ? { ...obj } : {};
  let cur = root;
  keys.forEach((k, i) => {
    if (i === keys.length - 1) { if (value === undefined) delete cur[k]; else cur[k] = value; }
    else { cur[k] = isObj(cur[k]) ? { ...cur[k] } : {}; cur = cur[k]; }
  });
  return root;
}

/**
 * Builds DOM: h('div', {class: 'x', onclick: fn, style: {color: 'red'}, dataset: {a: 1}}, child, ...).
 * Children can be strings, nodes, arrays or null/false (skipped).
 */
export function h(tag, attrs, ...children) {
  const el = document.createElement(tag);
  if (attrs) {
    for (const [k, v] of Object.entries(attrs)) {
      if (v == null || v === false) continue;
      if (k === 'class') el.className = v;
      else if (k === 'style' && isObj(v)) for (const [p, x] of Object.entries(v)) p.startsWith('--') ? el.style.setProperty(p, x) : (el.style[p] = x);
      else if (k === 'dataset') Object.assign(el.dataset, v);
      else if (k.startsWith('on') && typeof v === 'function') el.addEventListener(k.slice(2).toLowerCase(), v);
      else if (k === 'html') el.innerHTML = v;
      else if (v === true) el.setAttribute(k, '');
      else el.setAttribute(k, v);
    }
  }
  append(el, children);
  return el;
}
function append(el, children) {
  for (const c of children) {
    if (c == null || c === false) continue;
    if (Array.isArray(c)) append(el, c);
    else el.append(c instanceof Node ? c : document.createTextNode(String(c)));
  }
}
export function clear(el) { while (el.firstChild) el.removeChild(el.firstChild); return el; }
export const svg = (markup) => { const t = document.createElement('template'); t.innerHTML = markup.trim(); return t.content.firstChild; };

/** Sets `--name` custom properties on an element from an object. */
export function setVars(el, vars) { for (const [k, v] of Object.entries(vars)) el.style.setProperty(k, v); }

export function toast(message, kind = 'info') {
  const host = document.getElementById('toasts');
  if (!host) return;
  const t = h('div', { class: `toast toast-${kind}`, role: kind === 'error' ? 'alert' : 'status' }, message);
  host.append(t);
  setTimeout(() => t.classList.add('out'), 3800);
  setTimeout(() => t.remove(), 4300);
}

/** Pointer-drag helper: calls move(dx, dy, event) until release. */
export function drag(el, { start, move, end }) {
  el.addEventListener('pointerdown', (e) => {
    if (e.button !== 0) return;
    e.preventDefault();
    el.setPointerCapture(e.pointerId);
    const x0 = e.clientX, y0 = e.clientY;
    const ctx = start?.(e) ?? {};
    const onMove = (ev) => move?.(ev.clientX - x0, ev.clientY - y0, ev, ctx);
    const onUp = (ev) => {
      el.removeEventListener('pointermove', onMove);
      el.removeEventListener('pointerup', onUp);
      el.removeEventListener('pointercancel', onUp);
      end?.(ev, ctx);
    };
    el.addEventListener('pointermove', onMove);
    el.addEventListener('pointerup', onUp);
    el.addEventListener('pointercancel', onUp);
  });
}

export const prefersReducedMotion = () => matchMedia('(prefers-reduced-motion: reduce)').matches;
