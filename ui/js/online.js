// The online side of the player, from the front end: the switches, the question asked before going
// online for the first time, and the update check. Everything online is off until the user turns
// the main switch on; the lyrics lookup itself lives in controller.js beside the lyrics it feeds.

import { api } from './api.js';
import { emit, on } from './bus.js';
import { act, refreshSettings } from './controller.js';
import { state } from './state.js';
import { confirmDialog } from './ui/dialog.js';
import { laterVersion, showUpdateDialog } from './ui/updatedialog.js';

export const ONLINE_DEFAULTS = { enabled: false, radio: true, lyrics: true, auto_lyrics: true, lyrics_provider: 'auto', updates: true };

/** The online switches as saved (the main one is off until the user turns it on). */
export const online = () => ({ ...ONLINE_DEFAULTS, ...(state.settings?.online ?? {}) });

export async function setOnline(patch) {
  await act(api.setOnline({ ...online(), ...patch }));
  await refreshSettings();
}

/**
 * Explains what going online means and asks. Resolves to true once the main switch is on (or already
 * was), so a button that needs the internet can offer to turn it on instead of just failing.
 */
export async function askToGoOnline() {
  if (online().enabled) return true;
  const ok = await confirmDialog('Go online?',
    'Redshift stays offline unless you allow it. With this on, it can look up lyrics at BetterLyrics and LRCLIB (it sends the song’s title, artist, album and length), check GitHub for new versions and, only when you ask, search for radio stations (your search words go to radio-browser.info), play a station (Redshift connects straight to it; a plain http address is not encrypted), read a public playlist link or find a cover for your Discord card. Your music and library never leave this computer. You can turn it off again any time, and Settings > Online lists everything that was sent.',
    { ok: 'Turn on', danger: false });
  if (!ok) return false;
  await setOnline({ enabled: true });
  return true;
}

/** What the last update check found, shared by the startup check and Settings. */
export const updateStatus = { info: null, checking: false, error: '' };

export async function checkForUpdates(force = true) {
  if (updateStatus.checking) return updateStatus.info;
  updateStatus.checking = true; updateStatus.error = ''; emit('update');
  try { updateStatus.info = await api.checkUpdates(force); }
  catch (e) { updateStatus.error = String(e?.message ?? e); }
  updateStatus.checking = false; emit('update');
  return updateStatus.info;
}

/** Marks the Settings button while an update is waiting, so it is not missed after "Later". */
function markUpdate() {
  const on = !!updateStatus.info?.newer;
  document.querySelectorAll('.nav-btn[data-view="settings"]').forEach((b) => {
    b.toggleAttribute('data-update', on);
    b.title = on ? `Settings (update ${updateStatus.info.latest} available)` : 'Settings';
  });
}

/** Opens the update dialog for what the last check found. */
export function openUpdateDialog() {
  const info = updateStatus.info;
  return info?.newer ? showUpdateDialog(info) : Promise.resolve(null);
}

let started = false;
/**
 * A few seconds after the main window opens, and then every few hours while it stays open, ask
 * whether a newer version exists (only when the person allowed it). It asks about each version once;
 * after "Later" only the mark on Settings reminds.
 */
export function startupChecks() {
  if (started) return;
  started = true;
  const run = async () => {
    const o = online();
    if (!o.enabled || !o.updates) return;
    const u = await checkForUpdates(true);
    markUpdate();
    if (u?.newer && laterVersion() !== u.latest && !document.querySelector('.dialog-back')) openUpdateDialog();
  };
  setTimeout(run, 5000);
  setInterval(run, 6 * 3600 * 1000);
  on('update', markUpdate);
}
