// Whether the window is out of sight (hidden to the tray, or minimised). The backend tells us, because
// the web page itself isn't told: nothing animates or polls while it is, and everything starts again
// when the window comes back.

import { listen } from './api.js';
import { emit } from './bus.js';

let hidden = false;
/** True while nobody can see the window. */
export const asleep = () => hidden || document.hidden;

export function initHost() {
  try {
    listen('host-visible', (visible) => {
      hidden = !visible;
      document.documentElement.dataset.asleep = String(hidden);
      if (visible) { emit('awake'); document.dispatchEvent(new Event('visibilitychange')); }
    });
  } catch { /* plain browser: the page's own visibility is all there is */ }
}
