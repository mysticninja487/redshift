// "Redshift X is available": what is new, and a button that downloads it and installs it.

import { api, listen } from '../api.js';
import { h } from '../util.js';
import { button } from './controls.js';
import { modal } from './dialog.js';

const mb = (n) => (n / 1048576).toFixed(n < 10485760 ? 1 : 0);
const LATER_KEY = 'rs.update.later';

/** The version the person last said "Later" to, so startup does not ask again about the same one. */
export const laterVersion = () => { try { return localStorage.getItem(LATER_KEY) ?? ''; } catch { return ''; } };
const rememberLater = (v) => { try { localStorage.setItem(LATER_KEY, v); } catch { /* not remembered */ } };

/** Resolves to 'installing' when an update was started (the app is about to restart), else null. */
export function showUpdateDialog(info) {
  return modal((box, done) => {
    box.classList.add('up-dialog');
    const asset = info.asset;
    const status = h('p', { class: 'up-status', role: 'status', hidden: true });
    const bar = h('div', { class: 'up-bar', hidden: true }, h('i'));
    const fill = bar.firstChild;
    const later = button('Later', () => { rememberLater(info.latest); done(null); });
    const page = button('Open release page', () => api.openProjectPage(info.url).catch(() => {}));
    const go = button('Update now', () => install(), { cls: 'primary' });
    go.hidden = !asset;
    const actions = h('div', { class: 'dialog-actions' }, page, later, go);
    box.append(
      h('h3', { class: 'dialog-title' }, `Redshift ${info.latest} is available`),
      h('p', { class: 'dialog-text' }, `You have ${info.current}.`),
      info.notes ? h('pre', { class: 'up-notes', translate: 'no' }, info.notes) : '',
      asset ? '' : h('p', { class: 'hint' }, 'This release has no file that can be installed from here, so it has to be downloaded from its page.'),
      bar, status, actions);

    const say = (text, kind = '') => { status.hidden = !text; status.textContent = text; status.dataset.kind = kind; };
    async function install() {
      for (const b of [go, later, page]) b.disabled = true;
      bar.hidden = false; fill.style.width = '0%';
      say('Downloading…');
      let off = null;
      try { off = await listen('update-progress', ({ done: d, total }) => {
        fill.style.width = total ? `${Math.min(100, (d / total) * 100)}%` : '50%';
        say(total ? `Downloading: ${mb(d)} MB of ${mb(total)} MB` : `Downloading: ${mb(d)} MB`);
      }); } catch { /* no progress, still downloads */ }
      try {
        const path = await api.downloadUpdate(asset);
        fill.style.width = '100%';
        say('Installing… Redshift will close and start again.');
        await api.installUpdate(path, asset.kind);
        // the app exits a moment after this
      } catch (e) {
        say(String(e?.message ?? e), 'error');
        bar.hidden = true;
        for (const b of [go, later, page]) b.disabled = false;
      }
      try { await off?.(); } catch { /* nothing to undo */ }
    }
  });
}
