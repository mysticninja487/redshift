// The sleep timer: stop the music after a while, or at the end of the song. The countdown itself
// runs in the app (a hidden window can't be trusted to keep time); this shows it and sets it.

import { api } from './api.js';
import { on } from './bus.js';
import { act } from './controller.js';
import { iconButton } from './ui/controls.js';
import { showMenu } from './ui/menu.js';
import { h, toast } from './util.js';

const CHOICES = [15, 30, 45, 60, 90];

/** "12 min" / "45 s" for the badge and tooltip. */
function left(secs) {
  if (secs >= 90) return `${Math.ceil(secs / 60)} min`;
  return `${Math.max(1, Math.ceil(secs))} s`;
}

/** The top-bar button. Clicking it opens the choices; while a timer runs it shows what is left. */
export function sleepButton() {
  const badge = h('span', { class: 'sleep-left', hidden: true });
  const btn = iconButton('timer', 'Sleep timer', (e) => openMenu(e.currentTarget), { cls: 'sleep-btn', size: 20 });
  btn.append(badge);
  let status = { active: false }, at = 0, tick = 0;

  const remaining = () => (status.remaining_secs == null ? null : Math.max(0, status.remaining_secs - (performance.now() - at) / 1000));
  function paint() {
    const r = remaining();
    btn.setAttribute('aria-pressed', String(!!status.active));
    badge.hidden = !status.active;
    badge.textContent = status.kind === 'song' ? 'song' : r == null ? '' : left(r);
    btn.title = btn.ariaLabel = !status.active ? 'Sleep timer'
      : status.kind === 'song' ? 'Sleep timer: stops when this song ends'
      : `Sleep timer: music stops in ${left(r ?? 0)}`;
  }
  function apply(s) {
    status = s ?? { active: false }; at = performance.now();
    clearInterval(tick);
    if (status.active) tick = setInterval(() => { paint(); if (status.kind === 'time' && (remaining() ?? 1) <= 0) refresh(); }, status.fading ? 250 : 1000);
    paint();
  }
  async function refresh() { try { apply(await api.sleepStatus()); } catch { apply(null); } }

  async function set(minutes) {
    const s = await act(api.setSleepTimer(minutes));
    if (!s?.active && minutes === null) { toast('Nothing is playing, so there is no song to stop after.'); return; }
    apply(s);
    if (s?.active) toast(minutes === null ? 'The music will stop when this song ends.' : `The music will fade out and stop in ${minutes} minutes.`);
  }
  function openMenu(anchor) {
    const r = anchor.getBoundingClientRect();
    const here = status.active ? (status.kind === 'song' ? 'song' : 'time') : 'off';
    showMenu([
      { label: here === 'off' ? '✓ Off' : 'Off', onClick: async () => { await act(api.cancelSleepTimer()); apply(null); toast('Sleep timer off.'); } },
      { sep: true },
      ...CHOICES.map((m) => ({ label: `${m} minutes`, onClick: () => set(m) })),
      { sep: true },
      { label: here === 'song' ? '✓ At the end of this song' : 'At the end of this song', onClick: () => set(null) },
    ], r.left, r.bottom + 6);
  }

  // The app pauses the music by itself when the time is up, so look again whenever playback changes.
  on('status', () => { if (status.active) refresh(); });
  on('view', refresh);
  refresh();
  return btn;
}
