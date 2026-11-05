// The equaliser: response curve, ten band sliders, pre-amp, presets and an on/off switch.

import { api } from '../api.js';
import { act, refreshSettings } from '../controller.js';
import { toast } from '../util.js';
import { state } from '../state.js';
import { tokens } from '../theme.js';
import { debounce, h } from '../util.js';
import { button, select, slider, toggle } from '../ui/controls.js';
import { confirmDialog, promptText } from '../ui/dialog.js';
import { rgba } from '../viz/util.js';
import { lifecycle } from './common.js';

const FREQ_LABEL = ['31', '62', '125', '250', '500', '1k', '2k', '4k', '8k', '16k'];

export function mount(el) {
  const L = lifecycle();
  let info = null, curve = [];
  const cv = h('canvas', { class: 'eq-curve', 'aria-hidden': 'true' });
  const sliders = [];
  const bandsEl = h('div', { class: 'eq-bands' });
  const pre = slider({ min: -12, max: 12, step: 0.5, value: 0, vertical: true, center: true, label: 'Pre-amp', reset: 0, format: (v) => `${v} dB`, onInput: (v) => { act(api.eqPreamp(v)); changed(); } });
  const preCol = h('div', { class: 'eq-band eq-pre' }, h('span', { class: 'eq-val' }, '0'), pre, h('span', { class: 'eq-freq' }, 'Pre'));
  const on = toggle({ value: true, label: 'Equaliser on', onChange: (v) => { act(api.eqEnabled(v)); changed(); } });
  const preset = select({ options: [{ value: '', label: 'Custom' }], value: '', label: 'Preset', onChange: (name) => { if (name) act(api.eqPreset(name).then(sync)); } });
  const saveBtn = button('Save…', async () => {
    const name = await promptText('Save preset', { ok: 'Save', placeholder: 'Preset name', value: '' });
    if (!name) return;
    const saved = await act(api.saveEqPreset(name));
    if (saved) { await sync(); preset.set(saved); toast(`Saved preset "${saved}"`); }
  }, { cls: 'eq-save' });
  const delBtn = button('Delete', async () => {
    const name = preset.value;
    if (!name || !(await confirmDialog('Delete preset?', `"${name}" will be removed. Built-in presets can't be deleted.`, { ok: 'Delete' }))) return;
    await act(api.deleteEqPreset(name));
    await sync();
  }, { cls: 'eq-del' });
  delBtn.hidden = true;
  const head = h('div', { class: 'panel-head eq-head' }, h('div', { class: 'q-title' }, 'Equaliser'), preset, saveBtn, delBtn, button('Reset', () => act(api.eqPreset('Flat').then(sync)), { cls: 'eq-reset' }), on);
  el.append(h('div', { class: 'eq' }, head, cv, h('div', { class: 'eq-grid' }, preCol, bandsEl)));

  const readout = () => {
    sliders.forEach(({ s, val }, i) => { const v = info.settings.gains_db[i]; s.set(v); val.textContent = v > 0 ? `+${v}` : String(v); });
    pre.set(info.settings.preamp_db); preCol.querySelector('.eq-val').textContent = String(info.settings.preamp_db);
    on.set(info.settings.enabled);
    el.classList.toggle('eq-off', !info.settings.enabled);
    const same = (g) => g.every((x, i) => Math.abs(x - info.settings.gains_db[i]) < 0.01);
    const custom = (info.custom_presets ?? []).find((p) => same(p.gains_db) && Math.abs(p.preamp_db - info.settings.preamp_db) < 0.01);
    const match = custom ? custom.name : info.presets.find(([, g]) => same(g))?.[0];
    fillPresets();
    preset.set(match ?? '');
    delBtn.hidden = !custom;
  };
  const refreshCurve = debounce(async () => { curve = await api.eqCurve(120).catch(() => []); draw(); }, 30);
  /** The preset list: Custom, the built-in presets, then the ones the person saved. */
  function fillPresets() {
    const saved = info.custom_presets ?? [];
    preset.replaceChildren(...[h('option', { value: '' }, 'Custom'),
      saved.length ? h('optgroup', { label: 'Saved' }, saved.map((p) => h('option', { value: p.name }, p.name))) : null,
      h('optgroup', { label: 'Built-in' }, info.presets.map(([n]) => h('option', { value: n }, n)))].filter(Boolean));
  }
  async function sync() { info = await api.eqInfo(); await refreshSettings(); readout(); refreshCurve(); }
  function changed() { if (info) { refreshCurve(); } }

  async function build() {
    info = await api.eqInfo();
    info.band_freqs_hz.forEach((_, i) => {
      const val = h('span', { class: 'eq-val' }, '0');
      const s = slider({ min: -12, max: 12, step: 0.5, value: info.settings.gains_db[i], vertical: true, center: true, label: `${FREQ_LABEL[i]} Hz`, reset: 0, format: (v) => `${v} dB`,
        onInput: (v) => { info.settings.gains_db[i] = v; val.textContent = v > 0 ? `+${v}` : String(v); act(api.eqBand(i, v)); changed(); preset.set(''); delBtn.hidden = true; } });
      sliders.push({ s, val });
      bandsEl.append(h('div', { class: 'eq-band' }, val, s, h('span', { class: 'eq-freq' }, FREQ_LABEL[i])));
    });
    readout(); refreshCurve();
  }

  function draw() {
    const r = cv.getBoundingClientRect(), dpr = Math.min(devicePixelRatio || 1, 2);
    const w = Math.max(1, Math.round(r.width)), hh = Math.max(1, Math.round(r.height));
    if (cv.width !== Math.round(w * dpr)) { cv.width = Math.round(w * dpr); cv.height = Math.round(hh * dpr); }
    const g = cv.getContext('2d'); g.setTransform(dpr, 0, 0, dpr, 0, 0); g.clearRect(0, 0, w, hh);
    const y = (db) => hh / 2 - (db / 15) * (hh / 2);
    g.lineWidth = 1; g.font = '10px "JetBrains Mono", monospace'; g.textBaseline = 'middle';
    for (const db of [-12, -6, 0, 6, 12]) {
      g.strokeStyle = rgba(tokens.fg, db === 0 ? 0.3 : 0.1); g.setLineDash(db === 0 ? [] : [3, 4]);
      g.beginPath(); g.moveTo(0, y(db)); g.lineTo(w, y(db)); g.stroke();
      g.fillStyle = rgba(tokens.fg, 0.4); g.fillText(db > 0 ? `+${db}` : String(db), 3, y(db) - 6);
    }
    g.setLineDash([]);
    if (!curve.length) return;
    const x = (f) => (Math.log(f / 20) / Math.log(1000)) * w;
    const off = !info?.settings.enabled;
    g.beginPath();
    curve.forEach(([f, db], i) => (i ? g.lineTo(x(f), y(db)) : g.moveTo(x(f), y(db))));
    g.strokeStyle = off ? rgba(tokens.fg, 0.35) : tokens.accent; g.lineWidth = 2; g.lineJoin = 'round'; g.stroke();
    g.lineTo(w, y(0)); g.lineTo(0, y(0)); g.closePath();
    g.fillStyle = rgba(off ? tokens.fg : tokens.accent, 0.14); g.fill();
  }
  new ResizeObserver(draw).observe(cv);
  L.on(['theme', 'settings'], () => { if (info && state.settings) { info.settings = state.settings.eq; info.custom_presets = state.settings.custom_eq_presets ?? info.custom_presets; readout(); refreshCurve(); } });
  build().catch((e) => console.error('equaliser failed to load', e));
  return { destroy: () => L.destroy() };
}
