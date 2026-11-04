// What every panel needs: a way to subscribe that cleans up after itself, and per-frame hooks.

import { on } from '../bus.js';
import { isPlaying } from '../state.js';

export function lifecycle() {
  const offs = [];
  const rafs = new Set();
  return {
    /** Subscribes to bus topics (a name or an array of names). */
    on(topics, fn) { for (const t of [].concat(topics)) offs.push(on(t, fn)); },
    /** Runs `fn(dt)` every animation frame; `whenPlaying` skips frames while paused. Returns stop. */
    frame(fn, { whenPlaying = false } = {}) {
      let id = 0, last = performance.now(), live = true;
      const tick = (now) => {
        if (!live) return;
        const dt = Math.min(0.1, (now - last) / 1000); last = now;
        if (!whenPlaying || isPlaying()) fn(dt);
        id = requestAnimationFrame(tick);
      };
      id = requestAnimationFrame(tick);
      const stop = () => { live = false; cancelAnimationFrame(id); rafs.delete(stop); };
      rafs.add(stop);
      return stop;
    },
    add(fn) { offs.push(fn); },
    destroy() { offs.forEach((f) => f()); rafs.forEach((f) => f()); offs.length = 0; },
  };
}
