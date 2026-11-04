// Settings: every choice the person can make, grouped into tabs. Controls write straight to the
// current skin's overrides (prefs.set), so each skin keeps its own colours, fonts and layout.

import { api, pickFile, pickSave } from '../api.js';
import { on } from '../bus.js';
import { act, refreshPlayer, refreshSettings, setView } from '../controller.js';
import { ACTIONS, captureKeys, comboFor, comboFromEvent, prettyCombo, setCombo } from '../keys.js';
import { cfg, exportSkinOverrides, getPrefs, importSkinOverrides, isOverridden, resetSkin, set, setSkin } from '../prefs.js';
import { ACCENTS, FONTS, PANELS, SLOTS, skins } from '../skins.js';
import { state } from '../state.js';
import { tokens } from '../theme.js';
import { clear, clone, h, toast } from '../util.js';
import { VIZ, getViz, vizOptions } from '../viz/index.js';
import { mountViz } from '../viz/engine.js';
import { button, colorInput, iconButton, segmented, select, slider, toggle } from '../ui/controls.js';
import { confirmDialog } from '../ui/dialog.js';

const TABS = [
  ['appearance', 'Appearance'], ['layout', 'Layout'], ['visual', 'Visualisers'], ['playback', 'Playback'], ['lyrics', 'Lyrics & info'], ['keys', 'Shortcuts'], ['data', 'Data'],
];
const HERO_MODES = [{ value: 'art', label: 'Cover art' }, { value: 'turntable', label: 'Turntable' }, { value: 'viz', label: 'Visualiser' }, { value: 'none', label: 'Nothing' }];
const SLOT_NAMES = { top: 'Top', left: 'Left', stage: 'Centre', right: 'Right', bottom: 'Bottom' };

/** A labelled settings row. `path` adds a "back to default" button while the value is overridden. */
function row(label, control, { hint, path, redo } = {}) {
  const reset = path ? h('button', { class: 'row-reset', type: 'button', title: 'Back to this skin\'s default', 'aria-label': `Reset ${label}`, hidden: !isOverridden(path), onclick: () => { set(path, undefined); redo?.(); } }, 'Reset') : null;
  return h('div', { class: 'set-row' }, h('div', { class: 'set-label' }, h('span', null, label), hint && h('small', null, hint)), h('div', { class: 'set-control' }, control, reset));
}
const group = (title, ...rows) => h('section', { class: 'set-group' }, h('h3', null, title), ...rows);
const num = (v, d = 2) => +Number(v).toFixed(d);

export function mountSettings(root) {
  let tab = 'appearance';
  const tabs = h('div', { class: 'lib-tabs set-tabs', role: 'tablist' }, TABS.map(([id, name]) => h('button', { class: 'lib-tab', type: 'button', role: 'tab', dataset: { tab: id }, onclick: () => { tab = id; render(); } }, name)));
  const head = h('header', { class: 'v-head' }, iconButton('chevronLeft', 'Back to now playing', () => setView('now'), { cls: 'v-back' }), h('h1', { class: 'v-title' }, 'Settings'), tabs);
  const body = h('div', { class: 'v-body set-body' });
  root.append(head, body);
  let previews = [];
  let renderedSkin = null;

  /** Rebuilds the current tab. Controls don't rebuild themselves while being dragged. */
  function render() {
    previews.forEach((p) => p.destroy?.()); previews = [];
    captureKeys(false);
    renderedSkin = getPrefs().skin;
    tabs.querySelectorAll('.lib-tab').forEach((b) => b.setAttribute('aria-selected', String(b.dataset.tab === tab)));
    const st = body.scrollTop;
    clear(body).append(TAB_BUILDERS[tab]());
    body.scrollTop = st;
  }
  const again = () => render();

  // ---------- Appearance ----------
  function appearance() {
    const c = cfg(), skin = skins.find((s) => s.id === getPrefs().skin);
    const cards = h('div', { class: 'skin-cards' }, skins.map((s) => {
      const p = s.palette[s.scheme];
      return h('button', { class: 'skin-card', type: 'button', 'aria-pressed': String(s.id === skin.id), onclick: () => { setSkin(s.id); render(); },
        style: { '--p-bg': p.bg, '--p-bg2': p.bg2, '--p-fg': p.fg, '--p-accent': p.accent } },
        h('span', { class: 'sc-preview' }, h('i', { class: 'sc-a' }), h('i', { class: 'sc-b' }), h('i', { class: 'sc-c' })),
        h('span', { class: 'sc-name' }, s.name), h('span', { class: 'sc-blurb' }, s.blurb));
    }));

    const mode = c.color.mode;
    const colorBits = h('div', { class: 'color-bits' });
    const buildColorBits = () => {
      const m = cfg().color.mode, cc = cfg().color;
      clear(colorBits);
      if (m === 'preset') colorBits.append(h('div', { class: 'swatches', role: 'radiogroup', 'aria-label': 'Accent presets' }, ACCENTS.map((a) => h('button', { class: 'swatch', type: 'button', role: 'radio', title: a.name, 'aria-label': a.name, 'aria-checked': String(cc.preset === a.id), style: { '--sw': a.hex }, onclick: () => { set('color.preset', a.id); buildColorBits(); } }))));
      if (m === 'custom') colorBits.append(
        row('Accent', colorInput({ value: cc.accent ?? tokens.accent, label: 'Accent colour', onInput: (v) => set('color.accent', v) })),
        row('Background', colorInput({ value: cc.bg ?? tokens.bg, label: 'Background colour', onInput: (v) => set('color.bg', v) }), { hint: 'Leave alone to keep the skin\'s own.' }),
        row('Text', colorInput({ value: cc.fg ?? tokens.fg, label: 'Text colour', onInput: (v) => set('color.fg', v) })),
        h('div', { class: 'set-row' }, button('Clear custom colours', () => { set('color.accent', undefined); set('color.bg', undefined); set('color.fg', undefined); render(); })));
      if (m === 'art') colorBits.append(
        row('Cover strength', slider({ min: 0, max: 1, step: 0.05, value: cc.artMix, label: 'Cover colour strength', onInput: (v) => set('color.artMix', v), format: (v) => `${Math.round(v * 100)}%` }), { hint: 'How much the cover tints the whole interface. Grey covers keep the skin\'s accent.', path: 'color.artMix', redo: again }));
    };
    buildColorBits();

    return h('div', { class: 'set-page' },
      group('Skin', h('p', { class: 'hint' }, 'A skin is a whole look: layout, type, decoration and a default visualiser. Your changes are remembered separately for each one.'), cards,
        h('div', { class: 'set-row' }, button('Reset this skin to its defaults', async () => { if (await confirmDialog('Reset skin?', `All your changes to ${skin.name} will be forgotten.`, { ok: 'Reset' })) { resetSkin(); render(); } }))),
      group('Colour',
        row('Light or dark', segmented({ label: 'Colour scheme', options: [{ value: 'skin', label: 'Skin default' }, { value: 'dark', label: 'Dark' }, { value: 'light', label: 'Light' }], value: c.color.scheme, onChange: (v) => set('color.scheme', v) })),
        row('Accent from', segmented({ label: 'Where the accent colour comes from', options: [{ value: 'skin', label: 'Skin' }, { value: 'preset', label: 'Preset' }, { value: 'custom', label: 'Custom' }, { value: 'art', label: 'Album art' }], value: mode, onChange: (v) => { set('color.mode', v); buildColorBits(); } }), { hint: 'Album art follows the cover of whatever is playing.' }),
        colorBits,
        row('Glow', slider({ min: 0, max: 1, step: 0.05, value: c.color.glow, label: 'Glow', onInput: (v) => set('color.glow', v), format: (v) => `${Math.round(v * 100)}%` }), { path: 'color.glow', redo: again })),
      group('Type',
        row('Display font', fontSelect('type.display', skin.fonts.display), { hint: 'Titles and big numerals.', path: 'type.display', redo: again }),
        row('Body font', fontSelect('type.body', skin.fonts.body), { path: 'type.body', redo: again }),
        row('Mono font', fontSelect('type.mono', skin.fonts.mono), { hint: 'Times and technical details.', path: 'type.mono', redo: again }),
        row('Text size', slider({ min: 0.8, max: 1.4, step: 0.05, value: c.type.scale, label: 'Text size', reset: 1, onInput: (v) => set('type.scale', v), format: (v) => `${Math.round(v * 100)}%` }), { path: 'type.scale', redo: again }),
        row('Letter spacing', slider({ min: -0.04, max: 0.2, step: 0.005, value: c.type.tracking, label: 'Letter spacing', reset: 0, onInput: (v) => set('type.tracking', v), format: (v) => `${num(v, 3)} em` }), { path: 'type.tracking', redo: again })),
      group('Shape & surface',
        row('Corner radius', slider({ min: 0, max: 40, step: 1, value: c.shape.radius ?? 4, label: 'Corner radius', onInput: (v) => set('shape.radius', v), format: (v) => `${v}px` }), { path: 'shape.radius', redo: again }),
        row('Spacing', segmented({ label: 'Spacing', options: [{ value: 'compact', label: 'Compact' }, { value: 'cozy', label: 'Cozy' }, { value: 'roomy', label: 'Roomy' }], value: c.shape.density, onChange: (v) => set('shape.density', v) }), { path: 'shape.density', redo: again }),
        row('Line weight', slider({ min: 0, max: 3, step: 0.5, value: c.shape.border, label: 'Line weight', onInput: (v) => set('shape.border', v), format: (v) => `${v}px` }), { path: 'shape.border', redo: again }),
        row('Shadows', slider({ min: 0, max: 2, step: 0.1, value: c.shape.shadow, label: 'Shadows', onInput: (v) => set('shape.shadow', v), format: (v) => `${Math.round(v * 100)}%` }), { path: 'shape.shadow', redo: again }),
        row('Frosted glass', slider({ min: 0, max: 1, step: 0.05, value: c.shape.glass, label: 'Frosted glass', onInput: (v) => set('shape.glass', v), format: (v) => `${Math.round(v * 100)}%` }), { path: 'shape.glass', redo: again }),
        row('Film grain', slider({ min: 0, max: 1, step: 0.05, value: c.shape.grain, label: 'Film grain', onInput: (v) => set('shape.grain', v), format: (v) => `${Math.round(v * 100)}%` }), { path: 'shape.grain', redo: again }),
        row('Decoration', toggle({ label: 'Decoration', value: c.decor, onChange: (v) => set('decor', v) }), { hint: 'Ghost headings, background shapes and similar flourishes.', path: 'decor', redo: again }),
        row('Reduce motion', segmented({ label: 'Reduce motion', options: [{ value: 'system', label: 'Follow system' }, { value: 'on', label: 'On' }, { value: 'off', label: 'Off' }], value: c.motion.reduce === true ? 'on' : c.motion.reduce === false ? 'off' : c.motion.reduce, onChange: (v) => set('motion.reduce', v) }), { path: 'motion.reduce', redo: again }),
        row('Spinning record', toggle({ label: 'Spinning record', value: c.motion.spin, onChange: (v) => set('motion.spin', v) }), { path: 'motion.spin', redo: again })));
  }
  function fontSelect(path, def) {
    const cur = cfg().type[path.split('.')[1]] ?? def;
    return select({ label: path, value: cur, options: FONTS.map((f) => ({ value: f.id, label: f.label })), onChange: (v) => set(path, v) });
  }

  // ---------- Layout ----------
  function layout() {
    const c = cfg(), slots = clone(c.layout.slots);
    for (const s of SLOTS) slots[s] ??= [];
    const placement = (id) => SLOTS.find((s) => slots[s].includes(id)) ?? 'hidden';
    const commit = () => { set('layout.slots', slots); render(); };
    const move = (id, to) => { for (const s of SLOTS) slots[s] = slots[s].filter((x) => x !== id); if (to !== 'hidden') slots[to].push(id); commit(); };
    const nudge = (id, d) => { const s = placement(id); const a = slots[s]; const i = a.indexOf(id), j = i + d; if (j < 0 || j >= a.length) return; [a[i], a[j]] = [a[j], a[i]]; commit(); };

    const map = h('div', { class: 'layout-map', 'aria-label': 'Where each panel sits' }, SLOTS.map((s) => h('div', { class: `lm-slot lm-${s}` }, h('b', null, SLOT_NAMES[s]), slots[s].map((id) => h('span', { class: 'lm-chip' }, PANELS.find((p) => p.id === id)?.name ?? id)))));
    const table = h('div', { class: 'panel-table' }, PANELS.map((p) => {
      const where = placement(p.id);
      return h('div', { class: `pt-row${where === 'hidden' ? ' off' : ''}` },
        h('div', { class: 'pt-name' }, h('b', null, p.name), h('small', null, p.blurb)),
        select({ label: `${p.name} position`, value: where, options: [...SLOTS.map((s) => ({ value: s, label: SLOT_NAMES[s] })), { value: 'hidden', label: 'Hidden' }], onChange: (v) => move(p.id, v) }),
        h('span', { class: 'pt-order' }, iconButton('chevronDown', `Move ${p.name} later`, () => nudge(p.id, 1), { size: 16, cls: 'down' }), iconButton('chevronDown', `Move ${p.name} earlier`, () => nudge(p.id, -1), { size: 16, cls: 'up' })));
    }));
    const size = (key, label, max) => row(label, slider({ min: 0, max, step: 10, value: c.layout.sizes[key], label, onInput: (v) => set(`layout.sizes.${key}`, v), format: (v) => (v ? `${v}px` : 'Auto') }), { path: `layout.sizes.${key}`, redo: again });
    return h('div', { class: 'set-page' },
      group('Panels', h('p', { class: 'hint' }, 'Put each panel where you want it, or hide it. A panel adapts its look to the slot it lands in.'), map, table,
        h('div', { class: 'set-row' }, button('Default layout for this skin', () => { set('layout.slots', undefined); render(); }))),
      group('Sizes', size('left', 'Left column', 560), size('right', 'Right column', 560), size('top', 'Top strip', 300), size('bottom', 'Bottom strip', 300),
        row('Mirror left and right', toggle({ label: 'Mirror left and right', value: c.layout.flip, onChange: (v) => set('layout.flip', v) }), { path: 'layout.flip', redo: again })));
  }

  // ---------- Visualisers ----------
  function visual() {
    const c = cfg();
    const pickGrid = (target, current) => h('div', { class: 'viz-grid' }, VIZ.map((v) => {
      const cv = h('canvas', { class: 'viz-thumb', width: 160, height: 80 });
      const card = h('button', { class: 'viz-card', type: 'button', 'aria-pressed': String(v.id === current), onclick: () => { set(target, v.id); render(); } }, cv, h('span', { class: 'vc-name' }, v.name), h('span', { class: 'vc-blurb' }, v.blurb));
      requestAnimationFrame(() => { try { previews.push(mountViz(cv, { id: v.id })); } catch (e) { console.warn('preview failed', v.id, e); } });
      return card;
    }));
    const heroTarget = c.hero.mode === 'viz' ? 'hero.viz' : 'viz.main';
    const optionsFor = (id) => {
      const v = getViz(id);
      if (!v) return null;
      return h('div', { class: 'viz-options' }, v.options.map((o) => {
        const cur = vizOptions(id)[o.key];
        const ctl = o.type === 'toggle'
          ? toggle({ label: o.label, value: cur, onChange: (val) => set(`viz.opts.${id}.${o.key}`, val) })
          : slider({ min: o.min, max: o.max, step: o.step, value: cur, label: o.label, reset: o.def, onInput: (val) => set(`viz.opts.${id}.${o.key}`, val), format: (val) => String(num(val)) });
        return row(o.label, ctl, { path: `viz.opts.${id}.${o.key}`, redo: again });
      }));
    };
    const cur = c.hero.mode === 'viz' ? c.hero.viz : c.viz.main;
    return h('div', { class: 'set-page' },
      group('Centrepiece', row('Shows', segmented({ label: 'Centrepiece', options: HERO_MODES, value: c.hero.mode, onChange: (v) => { set('hero.mode', v); render(); } }), { path: 'hero.mode', redo: again }),
        c.hero.mode === 'turntable' && h('div', null,
          row('Tonearm', toggle({ label: 'Tonearm', value: c.hero.tonearm, onChange: (v) => set('hero.tonearm', v) }), { path: 'hero.tonearm', redo: again }),
          row('Plinth', toggle({ label: 'Plinth', value: c.hero.plinth, onChange: (v) => set('hero.plinth', v) }), { path: 'hero.plinth', redo: again }),
          row('Label', segmented({ label: 'Record label', options: [{ value: 'art', label: 'Cover' }, { value: 'accent', label: 'Accent' }], value: c.hero.label, onChange: (v) => set('hero.label', v) }), { path: 'hero.label', redo: again })),
        c.hero.mode === 'art' && h('div', null,
          row('Red sun', toggle({ label: 'Sun behind the cover', value: c.hero.sun, onChange: (v) => set('hero.sun', v) }), { path: 'hero.sun', redo: again }),
          row('Halftone', toggle({ label: 'Halftone cover', value: c.hero.halftone, onChange: (v) => set('hero.halftone', v) }), { path: 'hero.halftone', redo: again }),
          row('Glitch on the beat', toggle({ label: 'Glitch on the beat', value: c.hero.glitch, onChange: (v) => set('hero.glitch', v) }), { path: 'hero.glitch', redo: again }))),
      group(c.hero.mode === 'viz' ? 'Centrepiece visualiser' : 'Main visualiser', pickGrid(heroTarget, cur), optionsFor(cur)),
      group('Visualiser panel', h('p', { class: 'hint' }, 'The Visualiser and Level meters panels can sit anywhere (see Layout).'), pickGrid('viz.side', c.viz.side), optionsFor(c.viz.side)),
      group('Drawing',
        row('Colour', select({ label: 'Visualiser colour', value: c.viz.color, options: [{ value: 'accent', label: 'Accent' }, { value: 'fg', label: 'Text colour' }, { value: 'gradient', label: 'Accent gradient' }, { value: 'art', label: 'Cover colours' }, { value: 'rainbow', label: 'Rainbow' }], onChange: (v) => set('viz.color', v) }), { path: 'viz.color', redo: again }),
        row('Frame rate', segmented({ label: 'Visualiser frame rate', options: [{ value: 30, label: '30' }, { value: 60, label: '60' }, { value: 120, label: '120' }], value: c.viz.fps, onChange: (v) => set('viz.fps', v) }), { hint: 'Lower saves battery.', path: 'viz.fps', redo: again })));
  }

  // ---------- Playback ----------
  function playback() {
    const p = state.player;
    const dev = h('div', { class: 'set-row-inline' }, select({ label: 'Output device', value: '', options: [{ value: '', label: 'Loading…' }] }));
    api.outputDevices().then((list) => {
      const sel = select({ label: 'Output device', value: state.settings?.device ?? '', options: [{ value: '', label: 'System default' }, ...list.map((d) => ({ value: d.id, label: d.is_default ? `${d.name} (default)` : d.name }))], onChange: async (v) => { await act(api.setOutputDevice(v || null)); refreshPlayer(); } });
      dev.replaceChildren(sel);
    }).catch(() => dev.replaceChildren(h('span', { class: 'hint' }, 'Devices unavailable.')));
    return h('div', { class: 'set-page' },
      group('Transitions', row('Crossfade', slider({ min: 0, max: 12, step: 0.5, value: p.crossfade_secs, label: 'Crossfade', onChange: (v) => act(api.setCrossfade(v)).then(refreshSettings), onInput: (v) => api.setCrossfade(v).catch(() => {}), format: (v) => (v ? `${v} s` : 'Off (gapless)') }), { hint: 'Songs overlap for this long. Off plays them gap-free.' })),
      group('Loudness',
        row('ReplayGain', segmented({ label: 'ReplayGain', options: [{ value: 'off', label: 'Off' }, { value: 'track', label: 'Track' }, { value: 'album', label: 'Album' }], value: p.replaygain, onChange: (v) => act(api.setReplayGain(v, state.player.replaygain_preamp_db)).then(refreshPlayer) }), { hint: 'Evens out loudness using tags in the files. Never pushes a song past clipping.' }),
        row('Pre-amp', slider({ min: -12, max: 12, step: 0.5, value: p.replaygain_preamp_db, center: true, reset: 0, label: 'ReplayGain pre-amp', onChange: (v) => act(api.setReplayGain(state.player.replaygain, v)).then(refreshPlayer), format: (v) => `${v > 0 ? '+' : ''}${v} dB` }))),
      group('Output', row('Device', dev), h('p', { class: 'hint' }, p.sample_rate ? `Running at ${p.sample_rate / 1000} kHz. Songs at other rates are converted.` : '')),
      group('Equaliser', h('p', { class: 'hint' }, 'The equaliser is a panel. Add it from Layout to adjust the ten bands and presets.'), button('Show the equaliser panel', () => { const s = clone(cfg().layout.slots); for (const k of SLOTS) s[k] = (s[k] ?? []).filter((x) => x !== 'eq'); (s.right ??= []).push('eq'); set('layout.slots', s); toast('Equaliser added to the right column'); })));
  }

  // ---------- Lyrics & info ----------
  function lyricsTab() {
    const c = cfg();
    const sample = h('div', { class: 'lyrics ly-sample' }, h('div', { class: 'ly-scroll' }, ['A line that already passed', 'The line being sung right now', 'And the next one coming up', 'Then the one after that'].map((t, i) => h('div', { class: `ly-line${i === 0 ? ' past' : i === 1 ? ' active' : ''}`, dataset: { d: String(Math.abs(i - 1)) } }, t))));
    return h('div', { class: 'set-page' },
      group('Lyrics', sample,
        row('Text size', slider({ min: 0.7, max: 1.8, step: 0.05, value: c.lyrics.size, label: 'Lyrics size', reset: 1, onInput: (v) => set('lyrics.size', v), format: (v) => `${Math.round(v * 100)}%` }), { path: 'lyrics.size', redo: again }),
        row('Alignment', segmented({ label: 'Lyrics alignment', options: [{ value: 'left', label: 'Left' }, { value: 'center', label: 'Centre' }, { value: 'right', label: 'Right' }], value: c.lyrics.align, onChange: (v) => set('lyrics.align', v) }), { path: 'lyrics.align', redo: again }),
        row('Fade other lines', slider({ min: 0, max: 0.9, step: 0.05, value: c.lyrics.dim, label: 'Fade other lines', onInput: (v) => set('lyrics.dim', v), format: (v) => `${Math.round(v * 100)}%` }), { path: 'lyrics.dim', redo: again }),
        row('Blur distant lines', slider({ min: 0, max: 4, step: 0.25, value: c.lyrics.blur, label: 'Blur distant lines', onInput: (v) => set('lyrics.blur', v), format: (v) => `${v}px` }), { path: 'lyrics.blur', redo: again }),
        row('Word-by-word highlight', toggle({ label: 'Word highlight', value: c.lyrics.words, onChange: (v) => set('lyrics.words', v) }), { hint: 'Only for files whose lyrics carry word timing.', path: 'lyrics.words', redo: again })),
      group('Now playing',
        row('Technical details', toggle({ label: 'Technical details', value: c.info.format, onChange: (v) => set('info.format', v) }), { hint: 'Codec, sample rate and bit depth.', path: 'info.format', redo: again }),
        row('ReplayGain note', toggle({ label: 'ReplayGain note', value: c.info.replaygain, onChange: (v) => set('info.replaygain', v) }), { path: 'info.replaygain', redo: again }),
        row('Small cover', toggle({ label: 'Small cover', value: c.info.cover, onChange: (v) => set('info.cover', v) }), { path: 'info.cover', redo: again })),
      group('Progress bar',
        row('Style', segmented({ label: 'Progress bar style', options: [{ value: 'line', label: 'Line' }, { value: 'wave', label: 'Waveform' }, { value: 'ticks', label: 'Ruler' }, { value: 'dots', label: 'Dots' }], value: c.seek.style, onChange: (v) => set('seek.style', v) }), { path: 'seek.style', redo: again }),
        row('Show times', toggle({ label: 'Show times', value: c.seek.times, onChange: (v) => set('seek.times', v) }), { path: 'seek.times', redo: again })),
      group('Controls',
        row('Volume control', segmented({ label: 'Volume control', options: [{ value: 'slider', label: 'Slider' }, { value: 'knob', label: 'Knob' }], value: c.transport.volume, onChange: (v) => set('transport.volume', v) }), { path: 'transport.volume', redo: again }),
        row('Mute button', toggle({ label: 'Mute button', value: c.transport.mute, onChange: (v) => set('transport.mute', v) }), { path: 'transport.mute', redo: again })));
  }

  // ---------- Shortcuts ----------
  function keysTab() {
    const rows = ACTIONS.map((a) => {
      const btn = h('button', { class: 'key-btn', type: 'button' }, prettyCombo(comboFor(a.id)));
      const stop = () => { captureKeys(false); btn.classList.remove('capturing'); btn.textContent = prettyCombo(comboFor(a.id)); document.removeEventListener('keydown', onKey, true); };
      function onKey(e) {
        e.preventDefault(); e.stopPropagation();
        if (e.key === 'Escape') return stop();
        const combo = comboFromEvent(e);
        if (!combo) return;
        const clash = ACTIONS.find((o) => o.id !== a.id && comboFor(o.id) === combo);
        if (clash) { toast(`${prettyCombo(combo)} is already used for "${clash.label}"`, 'error'); return; }
        setCombo(a.id, combo); stop();
        render();
      }
      btn.addEventListener('click', () => { captureKeys(true); btn.classList.add('capturing'); btn.textContent = 'Press a key…'; document.addEventListener('keydown', onKey, true); btn.addEventListener('blur', stop, { once: true }); });
      const custom = getPrefs().keys[a.id];
      return h('div', { class: 'set-row' }, h('div', { class: 'set-label' }, h('span', null, a.label)), h('div', { class: 'set-control' }, btn, custom && h('button', { class: 'row-reset', type: 'button', onclick: () => { setCombo(a.id, null); render(); } }, 'Reset')));
    });
    return h('div', { class: 'set-page' }, group('Keyboard', h('p', { class: 'hint' }, 'Click a shortcut, then press the keys you want. Escape cancels.'), ...rows, h('div', { class: 'set-row' }, button('Restore all defaults', () => { import('../prefs.js').then((m) => { m.setPref('keys', {}); render(); }); }))));
  }

  // ---------- Data ----------
  function data() {
    return h('div', { class: 'set-page' },
      group('Share a look', h('p', { class: 'hint' }, 'Export your changes to the current skin as a small file, or load one someone sent you.'),
        h('div', { class: 'set-row' },
          button('Export this skin\'s settings', async () => {
            const dest = await pickSave(`redshift-${getPrefs().skin}.json`, [{ name: 'Redshift theme', extensions: ['json'] }]).catch(() => null);
            if (dest !== null) { navigator.clipboard?.writeText(JSON.stringify({ skin: getPrefs().skin, settings: exportSkinOverrides() }, null, 2)).catch(() => {}); toast('Settings copied to the clipboard'); }
          }),
          button('Import from clipboard', async () => {
            try { const t = JSON.parse(await navigator.clipboard.readText()); if (!t || typeof t.settings !== 'object') throw new Error('not a Redshift theme'); if (t.skin) setSkin(t.skin); importSkinOverrides(t.settings); toast('Theme applied'); render(); } catch { toast('The clipboard doesn\'t contain a Redshift theme.', 'error'); }
          }))),
      group('Reset', row('Everything', button('Reset all appearance settings', async () => { if (await confirmDialog('Reset everything?', 'Every skin goes back to its defaults and shortcuts are restored. Your music, playlists and playback settings are untouched.', { ok: 'Reset all' })) { const { setPref } = await import('../prefs.js'); setPref('skins', {}); setPref('keys', {}); render(); } }, { cls: 'danger' }))));
  }

  const TAB_BUILDERS = { appearance, layout, visual, playback, lyrics: lyricsTab, keys: keysTab, data };
  on('ui', () => { if (state.view === 'settings' && getPrefs().skin !== renderedSkin) render(); });
  on('view', (v) => { if (v === 'settings') render(); else { previews.forEach((p) => p.destroy?.()); previews = []; } });
  render();
  return { render };
}
