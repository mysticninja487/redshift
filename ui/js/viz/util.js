// Helpers shared by the visualisers.

/** Linear resample of `arr` to `n` points. */
export function resample(arr, n) {
  const out = new Float32Array(n);
  const m = arr.length;
  if (!m) return out;
  for (let i = 0; i < n; i++) {
    const x = (i / Math.max(1, n - 1)) * (m - 1);
    const a = Math.floor(x), b = Math.min(m - 1, a + 1), f = x - a;
    out[i] = arr[a] * (1 - f) + arr[b] * f;
  }
  return out;
}

export const smoothstep = (a, b, x) => { const t = Math.min(1, Math.max(0, (x - a) / (b - a))); return t * t * (3 - 2 * t); };

/** Reads `store[key]`, creating it with `make()` the first time. */
export function keep(store, key, make) { return (store[key] ??= make()); }

/** Eases `cur` toward `target` (frame-rate independent). Rising is faster than falling when asked. */
export function ease(cur, target, dt, up = 24, down = 8) { const k = 1 - Math.exp(-(target > cur ? up : down) * dt); return cur + (target - cur) * k; }

export function rgba(hex, a) {
  const n = parseInt(hex.slice(1), 16);
  return `rgba(${(n >> 16) & 255},${(n >> 8) & 255},${n & 255},${a})`;
}
