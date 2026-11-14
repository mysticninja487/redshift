// Settings: every choice the person can make, grouped into tabs. Controls write straight to the
// current skin's overrides (prefs.set), so each skin keeps its own colours, fonts and layout.

import { api, pickFile, pickSave } from '../api.js';
import { promptText } from '../ui/dialog.js';
import { on } from '../bus.js';
import { act, refreshPlayer, refreshSettings, setView } from '../controller.js';
import { ACTIONS, captureKeys, comboFor, comboFromEvent, prettyCombo, setCombo } from '../keys.js';
import { importFontFile, importedFonts, removeImportedFont } from '../fonts.js';
import { DEFAULT_AMBIENT, DEFAULT_MINI, DEFAULT_PREFS, setComparing, cfg, featureOn, getPrefs, isOverridden, resetSkin, set, setPref, setSkin, deleteLook, importLook, isCustomSkin, lookFile, renameLook, saveLook } from '../prefs.js';
import { ACCENTS, FONTS, PANELS, SLOTS, cleanFontName, fitsSlot, getSkin, skins } from '../skins.js';
import { icon } from '../ui/icons.js';
import { state } from '../state.js';
import { tokens } from '../theme.js';
import { clear, clone, getPath, h, toast } from '../util.js';
import { changedKeys } from './changes.js';
import { VIZ, getViz, vizOptions } from '../viz/index.js';
import { mountViz } from '../viz/engine.js';
import { button, colorInput, iconButton, segmented, select, slider as bareSlider, toggle } from '../ui/controls.js';
import { mount as mountEq } from '../panels/eq.js';
import { confirmDialog } from '../ui/dialog.js';
import { aboutBlock, openHelp } from '../ui/help.js';

// [id, label, icon]. The 'skin' entry only exists for skins that have options of their own.
const TABS = [
  ['appearance', 'Appearance', 'palette'], ['skin', '', 'record'], ['playback', 'Playback', 'play'], ['eq', 'Equaliser', 'eq'], ['lyrics', 'Lyrics & info', 'lyrics'],
  ['visual', 'Visualisers', 'viz'], ['layout', 'Layout', 'layout'], ['ambient', 'Ambient mode', 'ambient'], ['mini', 'Mini player', 'pip'], ['keys', 'Shortcuts', 'keyboard'], ['data', 'Data & reset', 'database'], ['help', 'Help & about', 'info'],
];
const HERO_MODES = [{ value: 'art', label: 'Cover art' }, { value: 'turntable', label: 'Turntable' }, { value: 'viz', label: 'Visualiser' }, { value: 'none', label: 'Nothing' }];
const SLOT_NAMES = { top: 'Top', left: 'Left', stage: 'Centre', right: 'Right', bottom: 'Bottom' };

// Rows that can be changed, so they can show it as it happens. Emptied each time a tab is drawn.
let liveRows = [];

/**
 * A labelled settings row. `path` (a setting of the current skin) or `gpath` (one every skin shares)
 * marks the row and adds a "back to default" button while the value differs from its default.
 */
function row(label, control, { hint, path, gpath, redo } = {}) {
  const key = path ? `skin:${path}` : gpath ? `shared:${gpath}` : null;
  const reset = key ? h('button', { class: 'row-reset', type: 'button', title: path ? 'Back to this skin\'s default' : 'Back to the default', 'aria-label': `Reset ${label}`, hidden: true,
    onclick: () => { if (path) set(path, undefined); else setPref(gpath, getPath(DEFAULT_PREFS, gpath)); (redo ?? (() => liveRender?.()))(); } }, 'Reset') : null;
  const el = h('div', { class: 'set-row' }, h('div', { class: 'set-label' }, h('span', null, label), hint && h('small', null, hint)), h('div', { class: 'set-control' }, control, reset));
  if (key) { liveRows.push({ key, el, reset }); markRow({ key, el, reset }, new Set(changedKeys(getPrefs()))); }
  return el;
}
let liveRender = null;
function markRow({ key, el, reset }, changed) {
  const on = changed.has(key);
  el.dataset.changed = String(on);
  if (reset) reset.hidden = !on;
}
const group = (title, ...rows) => h('section', { class: 'set-group' }, h('h3', null, title), ...rows);
const num = (v, d = 2) => +Number(v).toFixed(d);

/** A slider with its current value written out beside it, so the number is never a guess. */
function slider(opts) {
  const fmt = opts.format ?? ((v) => String(num(v)));
  const out = h('output', { class: 'set-val', 'aria-hidden': 'true' }, fmt(opts.value));
  const s = bareSlider({ ...opts, onInput: (v) => { out.textContent = fmt(v); opts.onInput?.(v); } });
  const move = s.set;
  s.set = (v) => { move(v); out.textContent = fmt(v); };
  return h('div', { class: 'set-slider' }, s, out);
}

export function mountSettings(root) {
  let tab = 'appearance';
  const tabs = h('nav', { class: 'set-nav', role: 'tablist', 'aria-orientation': 'vertical', 'aria-label': 'Settings sections' }, TABS.map(([id, name, ic]) =>
    h('button', { class: 'set-tab', type: 'button', role: 'tab', dataset: { tab: id }, onclick: () => { tab = id; render(); } }, icon(ic, 18), h('span', null, name))));
  // Hold to see the skin as it is without your changes: what did that setting actually do?
  const compareBtn = h('button', { class: 'cmp-btn', type: 'button', title: 'Hold to see this skin without your changes' }, icon('eye', 16), h('span', null, 'Hold to compare'));
  const badge = h('div', { class: 'cmp-badge', role: 'status', hidden: true });
  const hold = (on) => { setComparing(on); compareBtn.setAttribute('aria-pressed', String(on)); };
  compareBtn.addEventListener('pointerdown', (e) => { if (e.button === 0) hold(true); });
  compareBtn.addEventListener('keydown', (e) => { if ((e.key === ' ' || e.key === 'Enter') && !e.repeat) { e.preventDefault(); hold(true); } });
  compareBtn.addEventListener('keyup', (e) => { if (e.key === ' ' || e.key === 'Enter') hold(false); });
  for (const ev of ['pointerup', 'pointercancel', 'blur']) addEventListener(ev, () => hold(false));
  on('compare', (v) => {
    const any = changedKeys(getPrefs()).some((k) => k.startsWith('skin:'));
    badge.textContent = any ? `${getSkin(getPrefs().skin).name} without your changes` : 'You haven\'t changed this skin yet';
    badge.hidden = !v;
  });
  const head = h('header', { class: 'v-head' }, iconButton('chevronLeft', 'Back to now playing', () => setView('now'), { cls: 'v-back' }), h('h1', { class: 'v-title' }, 'Settings'), compareBtn);
  const body = h('div', { class: 'v-body set-body' });
  root.append(head, h('div', { class: 'set-shell' }, tabs, body), badge);
  let previews = [];
  let renderedSkin = null;

  /** Rebuilds the current tab. Controls don't rebuild themselves while being dragged. */
  function render() {
    previews.forEach((p) => p.destroy?.()); previews = [];
    liveRows = [];
    captureKeys(false);
    renderedSkin = getPrefs().skin;
    const mine = getSkin(getPrefs().skin);
    if (tab === 'skin' && !mine.options?.length) tab = 'appearance';
    tabs.querySelectorAll('.set-tab').forEach((b) => {
      const isSkin = b.dataset.tab === 'skin';
      b.hidden = isSkin && !mine.options?.length;
      if (isSkin) b.lastChild.textContent = `${mine.name} options`;
      b.setAttribute('aria-selected', String(b.dataset.tab === tab));
    });
    const st = body.scrollTop;
    clear(body).append(TAB_BUILDERS[tab]());
    body.scrollTop = st;
  }
  const again = () => render();
  liveRender = render;

  // Rows that differ from their default carry a dot and a Reset button; they follow along as things change.
  on('ui', () => {
    if (state.view !== 'settings') return;
    const keys = new Set(changedKeys(getPrefs()));
    liveRows = liveRows.filter((r) => r.el.isConnected);
    liveRows.forEach((r) => markRow(r, keys));
  });

  // ---------- Appearance ----------
  function appearance() {
    const c = cfg(), skin = skins.find((s) => s.id === getPrefs().skin);
    const cards = h('div', { class: 'skin-cards' }, skins.map((s) => {
      const p = s.palette[s.scheme];
      return h('button', { class: 'skin-card', type: 'button', 'aria-pressed': String(s.id === skin.id), onclick: () => { setSkin(s.id); render(); },
        style: { '--p-bg': p.bg, '--p-bg2': p.bg2, '--p-fg': p.fg, '--p-accent': p.accent } },
        h('span', { class: 'sc-preview' }, h('i', { class: 'sc-a' }), h('i', { class: 'sc-b' }), h('i', { class: 'sc-c' })),
        h('span', { class: 'sc-name' }, s.name, s.custom ? h('span', { class: 'sc-mine' }, 'Yours') : null), h('span', { class: 'sc-blurb' }, s.blurb));
    }));
    const mineRow = h('div', { class: 'set-row set-actions' },
      button('Save this look as a new skin…', async () => {
        const name = await promptText('Name your skin', { value: isCustomSkin() ? `${skin.name} copy` : `My ${skin.name}`, ok: 'Save skin' });
        if (name === null) return;
        if (!saveLook(name)) toast('That\'s the most skins you can keep. Delete one first.', 'error'); else toast(`Saved “${name}”`);
        render();
      }),
      isCustomSkin() ? button('Rename', async () => { const name = await promptText('Rename skin', { value: skin.name, ok: 'Rename' }); if (name !== null) { renameLook(skin.id, name); render(); } }) : null,
      isCustomSkin() ? button('Delete this skin', async () => { if (await confirmDialog('Delete skin?', `“${skin.name}” will be removed. Your music and the built-in skins aren\'t touched.`, { ok: 'Delete' })) { deleteLook(skin.id); render(); } }, { cls: 'danger' }) : null);

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
      group('Skin', h('p', { class: 'hint' }, 'A skin is a whole look: layout, type, decoration and a default visualiser. Your changes are remembered separately for each one. Like how this one turned out? Save it as a skin of your own.'), cards, mineRow,
        h('div', { class: 'set-row' }, button(isCustomSkin() ? 'Undo my changes to this skin' : 'Reset this skin to its defaults', async () => { if (await confirmDialog('Reset skin?', `All your changes to ${skin.name} since you ${isCustomSkin() ? 'saved it' : 'started'} will be forgotten.`, { ok: 'Reset' })) { resetSkin(); render(); } }))),
      group('Colour',
        row('Light or dark', segmented({ label: 'Colour scheme', options: [{ value: 'skin', label: 'Skin default' }, { value: 'dark', label: 'Dark' }, { value: 'light', label: 'Light' }], value: c.color.scheme, onChange: (v) => set('color.scheme', v) })),
        row('Accent from', segmented({ label: 'Where the accent colour comes from', options: [{ value: 'skin', label: 'Skin' }, { value: 'preset', label: 'Preset' }, { value: 'custom', label: 'Custom' }, { value: 'art', label: 'Album art' }], value: mode, onChange: (v) => { set('color.mode', v); buildColorBits(); } }), { hint: 'Album art follows the cover of whatever is playing.' }),
        colorBits,
        row('Glow', slider({ min: 0, max: 1, step: 0.05, value: c.color.glow, label: 'Glow', onInput: (v) => set('color.glow', v), format: (v) => `${Math.round(v * 100)}%` }), { path: 'color.glow', redo: again })),
      group('Type',
        row('Display font', fontSelect('type.display', skin.fonts.display), { hint: 'Titles and big numerals.', path: 'type.display', redo: again }),
        row('Body font', fontSelect('type.body', skin.fonts.body), { path: 'type.body', redo: again }),
        row('Mono font', fontSelect('type.mono', skin.fonts.mono), { hint: 'Times and technical details.', path: 'type.mono', redo: again }),
        importedFontList(),
        row('Text size', slider({ min: 0.8, max: 1.4, step: 0.05, value: c.type.scale, label: 'Text size', reset: 1, onInput: (v) => set('type.scale', v), format: (v) => `${Math.round(v * 100)}%` }), { path: 'type.scale', redo: again }),
        row('Letter spacing', slider({ min: -0.04, max: 0.2, step: 0.005, value: c.type.tracking, label: 'Letter spacing', reset: 0, onInput: (v) => set('type.tracking', v), format: (v) => `${num(v, 3)} em` }), { path: 'type.tracking', redo: again })),
      group('Shape & surface',
        row('Corner radius', slider({ min: 0, max: 40, step: 1, value: c.shape.radius ?? 4, label: 'Corner radius', onInput: (v) => set('shape.radius', v), format: (v) => `${v}px` }), { path: 'shape.radius', redo: again }),
        row('Spacing', segmented({ label: 'Spacing', options: [{ value: 'compact', label: 'Compact' }, { value: 'cozy', label: 'Cozy' }, { value: 'roomy', label: 'Roomy' }], value: c.shape.density, onChange: (v) => set('shape.density', v) }), { path: 'shape.density', redo: again }),
        row('Line weight', slider({ min: 0, max: 3, step: 0.5, value: c.shape.border, label: 'Line weight', onInput: (v) => set('shape.border', v), format: (v) => `${v}px` }), { path: 'shape.border', redo: again }),
        row('Shadows', slider({ min: 0, max: 2, step: 0.1, value: c.shape.shadow, label: 'Shadows', onInput: (v) => set('shape.shadow', v), format: (v) => `${Math.round(v * 100)}%` }), { path: 'shape.shadow', redo: again }),
        row('Frosted glass', slider({ min: 0, max: 1, step: 0.05, value: c.shape.glass, label: 'Frosted glass', onInput: (v) => set('shape.glass', v), format: (v) => `${Math.round(v * 100)}%` }), { path: 'shape.glass', redo: again }),
        row('Film grain', slider({ min: 0, max: 1, step: 0.05, value: c.shape.grain, label: 'Film grain', onInput: (v) => set('shape.grain', v), format: (v) => `${Math.round(v * 100)}%` }), { path: 'shape.grain', redo: again }),
        skin.hasDecor === false ? null : row('Decoration', toggle({ label: 'Decoration', value: c.decor, onChange: (v) => set('decor', v) }), { hint: 'Ghost headings, background shapes and similar flourishes.', path: 'decor', redo: again }),
        row('Reduce motion', segmented({ label: 'Reduce motion', options: [{ value: 'system', label: 'Follow system' }, { value: 'on', label: 'On' }, { value: 'off', label: 'Off' }], value: c.motion.reduce === true ? 'on' : c.motion.reduce === false ? 'off' : c.motion.reduce, onChange: (v) => set('motion.reduce', v) }), { path: 'motion.reduce', redo: again })));
  }
  /**
   * A font chooser: the built-in fonts, fonts the person imported, any font installed on the PC
   * (typed by name), and a shortcut to import a font file. The choice is stored per skin.
   */
  function fontSelect(path, def) {
    const cur = cfg().type[path.split('.')[1]] ?? def;
    const builtin = FONTS.map((f) => ({ value: f.id, label: f.label }));
    const imported = importedFonts().map((f) => ({ value: `file:${f.file}`, label: `${f.family} (imported)` }));
    const extra = [];
    if (cur.startsWith('local:')) extra.push({ value: cur, label: `${cleanFontName(cur.slice(6))} (installed)` });
    if (cur.startsWith('file:') && !imported.some((o) => o.value === cur)) extra.push({ value: cur, label: `${cur.slice(5)} (missing)` });
    const options = [...builtin, ...imported, ...extra, { value: '__local', label: 'Installed font by name…' }, { value: '__import', label: 'Import a font file…' }];
    const sel = select({ label: path, value: cur, options, onChange: async (v) => {
      if (v === '__local') {
        const name = cleanFontName((await promptText('Installed font', { ok: 'Use font', placeholder: 'Font name, e.g. Segoe UI or Consolas' })) ?? '');
        if (name) set(path, `local:${name}`);
        return render();
      }
      if (v === '__import') {
        const f = await pickFile([{ name: 'Fonts', extensions: ['ttf', 'otf', 'woff', 'woff2'] }]).catch(() => null);
        if (f) {
          const entry = await act(importFontFile(f));
          if (entry) { set(path, `file:${entry.file}`); toast(`Imported ${entry.family}`); }
        }
        return render();
      }
      set(path, v);
    } });
    sel.classList.add('font-select');
    return sel;
  }
  function importedFontList() {
    const list = importedFonts();
    if (!list.length) return null;
    return h('div', { class: 'font-list' }, list.map((f) => h('div', { class: 'font-item' },
      h('span', { class: 'font-sample', style: { fontFamily: `"${cleanFontName(`Redshift import ${f.file}`)}", sans-serif` } }, f.family), h('small', null, f.file),
      iconButton('trash', `Remove ${f.family}`, async () => { if (await confirmDialog('Remove font?', `${f.family} will be deleted from Redshift's font folder. Skins using it fall back to a default font.`, { ok: 'Remove' })) { await act(removeImportedFont(f.file)); render(); } }, { size: 16 }))));
  }

  // ---------- Layout ----------
  function layout() {
    const c = cfg(), slots = clone(c.layout.slots);
    for (const s of SLOTS) slots[s] ??= [];
    const placement = (id) => SLOTS.find((s) => slots[s].includes(id)) ?? 'hidden';
    const commit = () => { set('layout.slots', slots); render(); };
    // A slot that is already full can't take another panel: it would run off the window.
    const roomIn = (slot, id) => placement(id) === slot || fitsSlot(slot, [...slots[slot], id]);
    const move = (id, to) => { if (to !== 'hidden' && !roomIn(to, id)) { toast(`The ${SLOT_NAMES[to].toLowerCase()} slot is full. Move or hide something there first.`); render(); return; } for (const s of SLOTS) slots[s] = slots[s].filter((x) => x !== id); if (to !== 'hidden') slots[to].push(id); commit(); };
    const nudge = (id, d) => { const s = placement(id); const a = slots[s]; const i = a.indexOf(id), j = i + d; if (j < 0 || j >= a.length) return; [a[i], a[j]] = [a[j], a[i]]; commit(); };

    const map = h('div', { class: 'layout-map', 'aria-label': 'Where each panel sits' }, SLOTS.map((s) => h('div', { class: `lm-slot lm-${s}` }, h('b', null, SLOT_NAMES[s]), slots[s].map((id) => h('span', { class: 'lm-chip' }, PANELS.find((p) => p.id === id)?.name ?? id)))));
    const table = h('div', { class: 'panel-table' }, PANELS.map((p) => {
      const where = placement(p.id);
      return h('div', { class: `pt-row${where === 'hidden' ? ' off' : ''}` },
        h('div', { class: 'pt-name' }, h('b', null, p.name), h('small', null, p.blurb)),
        select({ label: `${p.name} position`, value: where, options: [...SLOTS.map((s) => (roomIn(s, p.id) ? { value: s, label: SLOT_NAMES[s] } : { value: s, label: `${SLOT_NAMES[s]} (full)`, disabled: true })), { value: 'hidden', label: 'Hidden' }], onChange: (v) => move(p.id, v) }),
        h('span', { class: 'pt-order' }, iconButton('chevronDown', `Move ${p.name} later`, () => nudge(p.id, 1), { size: 16, cls: 'down' }), iconButton('chevronDown', `Move ${p.name} earlier`, () => nudge(p.id, -1), { size: 16, cls: 'up' })));
    }));
    const size = (key, label, max) => { const side = key === 'left' || key === 'right'; return row(label, slider({ min: side ? 180 : 0, max, step: 10, value: Math.max(side ? 180 : 0, c.layout.sizes[key] || 0), label, onInput: (v) => set(`layout.sizes.${key}`, v), format: (v) => (v ? `${v}px` : 'Auto') }), { path: `layout.sizes.${key}`, redo: again }); };
    return h('div', { class: 'set-page' },
      group('Panels', h('p', { class: 'hint' }, 'Put each panel where you want it, or hide it. A panel adapts its look to the slot it lands in. A column holds about three tall panels and a strip five, so a full slot is greyed out.'),
        row('Show queue', toggle({ label: 'Show queue', value: featureOn('queue'), onChange: (v) => setPref('features.queue', v) }), { hint: 'A quick switch for the queue panel in every skin. Also on the Now Playing tools (Shift+Q).', gpath: 'features.queue' }), map, table,
        h('div', { class: 'set-row' }, button('Default layout for this skin', () => { set('layout.slots', undefined); render(); }))),
      group('Sizes', size('left', 'Left column', 560), size('right', 'Right column', 560), size('top', 'Top strip', 300), size('bottom', 'Bottom strip', 300),
        row('Mirror left and right', toggle({ label: 'Mirror left and right', value: c.layout.flip, onChange: (v) => set('layout.flip', v) }), { path: 'layout.flip', redo: again })));
  }


  /** The turntable centrepiece's own settings: shared by the Turntable skin's page and the centrepiece group. */
  function turntableRows() {
    const c = cfg();
    return [
      row('Spinning record', toggle({ label: 'Spinning record', value: c.motion.spin, onChange: (v) => set('motion.spin', v) }), { hint: 'The record turns while a song plays.', path: 'motion.spin', redo: again }),
      row('Tonearm', toggle({ label: 'Tonearm', value: c.hero.tonearm, onChange: (v) => set('hero.tonearm', v) }), { path: 'hero.tonearm', redo: again }),
      row('Plinth', toggle({ label: 'Plinth', value: c.hero.plinth, onChange: (v) => set('hero.plinth', v) }), { path: 'hero.plinth', redo: again }),
      row('Label', segmented({ label: 'Record label', options: [{ value: 'art', label: 'Cover' }, { value: 'accent', label: 'Accent' }], value: c.hero.label, onChange: (v) => set('hero.label', v) }), { path: 'hero.label', redo: again }),
    ];
  }

  // ---------- Skin options (only the current skin's own) ----------
  function skinTab() {
    const c = cfg(), skin = getSkin(getPrefs().skin);
    const REG = {
      turntable: () => group('Turntable', ...turntableRows()),
      glitch: () => group('Glitch', row('Glitch on the beat', toggle({ label: 'Glitch on the beat', value: c.hero.glitch, onChange: (v) => set('hero.glitch', v) }), { hint: 'The cover and title tear briefly on strong beats.', path: 'hero.glitch', redo: again })),
      paper: () => group('Print look',
        row('Red sun', toggle({ label: 'Sun behind the cover', value: c.hero.sun, onChange: (v) => set('hero.sun', v) }), { path: 'hero.sun', redo: again }),
        row('Halftone cover', toggle({ label: 'Halftone cover', value: c.hero.halftone, onChange: (v) => set('hero.halftone', v) }), { hint: 'Prints the cover as a coarse dot screen.', path: 'hero.halftone', redo: again })),
    };
    const o = skin.options ?? [], parts = [];
    if (o.includes('tonearm')) parts.push(REG.turntable());
    if (o.includes('glitch')) parts.push(REG.glitch());
    if (o.includes('sun') || o.includes('halftone')) parts.push(REG.paper());
    return h('div', { class: 'set-page' }, h('p', { class: 'hint' }, `These only exist in ${skin.name}. Switch skin and they are replaced by that skin's own options, if it has any.`), ...parts);
  }


  // ---------- Ambient mode ----------
  function ambientPage() {
    const a = { ...DEFAULT_AMBIENT, ...(getPrefs().ambient ?? {}) };
    const put = (k) => (v) => setPref(`ambient.${k}`, v);
    const vizOpts = [{ value: 'auto', label: 'Same as the skin' }, ...VIZ.map((v) => ({ value: v.id, label: v.name }))];
    return h('div', { class: 'set-page' },
      group('Ambient mode', h('p', { class: 'hint' }, 'Turns the whole window into one thing to look at: the visualiser, the lyrics, or both. Open it with the top-bar button or A, leave with Esc. Inside it, 1, 2 and 3 switch views and V changes the visualiser.'),
        row('Opens as', segmented({ label: 'Ambient mode opens as', options: [{ value: 'viz', label: 'Visualiser' }, { value: 'lyrics', label: 'Lyrics' }, { value: 'both', label: 'Both' }], value: a.mode, onChange: put('mode') }), { hint: 'It also remembers the last view you used.', gpath: 'ambient.mode' }),
        row('Fullscreen', toggle({ label: 'Go fullscreen', value: a.fullscreen, onChange: put('fullscreen') }), { hint: 'Fills the screen. Off keeps it inside the window. The screen is kept awake either way while ambient mode is open.', gpath: 'ambient.fullscreen' })),
      group('Look',
        row('Visualiser', select({ label: 'Ambient visualiser', value: a.viz, options: vizOpts, onChange: put('viz') }), { hint: 'Its own options are under Visualisers.', gpath: 'ambient.viz' }),
        row('Background', segmented({ label: 'Ambient background', options: [{ value: 'cover', label: 'Blurred cover' }, { value: 'theme', label: 'Skin colour' }, { value: 'black', label: 'Black' }], value: a.bg, onChange: put('bg') }), { hint: 'Blurred cover is the heaviest on the graphics card. Reduce effects softens it.', gpath: 'ambient.bg' }),
        row('Lyrics size', slider({ min: 0.6, max: 1.6, step: 0.05, value: a.lyricsSize, label: 'Ambient lyrics size', onInput: put('lyricsSize'), format: (v) => `${Math.round(v * 100)}%` }), { gpath: 'ambient.lyricsSize' })),
      group('Controls',
        row('Hide controls after', segmented({ label: 'Hide controls after', options: [{ value: 2, label: '2 s' }, { value: 3, label: '3 s' }, { value: 5, label: '5 s' }, { value: 0, label: 'Never' }], value: a.hideAfter, onChange: put('hideAfter') }), { hint: 'Move the mouse to bring them back.', gpath: 'ambient.hideAfter' }),
        row('Track title', segmented({ label: 'Track title', options: [{ value: 'controls', label: 'With controls' }, { value: 'always', label: 'Always' }, { value: 'never', label: 'Never' }], value: a.title, onChange: put('title') }), { hint: 'With controls also shows it for a few seconds when a new song starts.', gpath: 'ambient.title' })),
      group('Reset', h('div', { class: 'set-row' }, button('Back to the defaults', () => { setPref('ambient', { ...DEFAULT_AMBIENT }); render(); }))));
  }

  // ---------- Mini player ----------
  function miniPage() {
    const m = { ...DEFAULT_MINI, ...(getPrefs().mini ?? {}) };
    const put = (k) => (v) => setPref(`mini.${k}`, v);
    const pct = (v) => `${Math.round(v)}%`;
    return h('div', { class: 'set-page' },
      group('Window',
        row('Keep on top', toggle({ label: 'Keep the mini player on top', value: m.onTop, onChange: put('onTop') }), { hint: 'It stays above other windows. Also the pin button on the mini player itself.', gpath: 'mini.onTop' }),
        row('Opens as', segmented({ label: 'Mini player opens as', options: [{ value: 'last', label: 'Last used' }, { value: 'compact', label: 'Compact' }, { value: 'expanded', label: 'Expanded' }], value: m.open, onChange: put('open') }), { hint: 'Drag the bottom-right corner of the mini player to resize it. Each size is remembered.', gpath: 'mini.open' }),
        row('Expanded list', segmented({ label: 'Expanded list', options: [{ value: 'queue', label: 'Up next' }, { value: 'lyrics', label: 'Lyrics' }], value: m.list, onChange: put('list') }), { hint: 'What the expanded player shows beside the cover.', gpath: 'mini.list' })),
      group('Look',
        row('Background', segmented({ label: 'Mini player background', options: [{ value: 'cover', label: 'Blurred cover' }, { value: 'theme', label: 'Skin colour' }, { value: 'accent', label: 'Accent tint' }], value: m.bg, onChange: put('bg') }), { gpath: 'mini.bg' }),
        row('Opacity', slider({ min: 40, max: 100, step: 5, value: m.opacity, label: 'Mini player opacity', onInput: put('opacity'), format: pct }), { hint: 'Lower lets what is behind it show through.', gpath: 'mini.opacity' }),
        row('Corner radius', slider({ min: 0, max: 32, step: 2, value: m.radius, label: 'Mini player corner radius', onInput: put('radius'), format: (v) => `${v} px` }), { gpath: 'mini.radius' }),
        h('p', { class: 'hint' }, 'The mini player takes its colours and fonts from the current skin.')),
      group('Show',
        row('Cover in the compact player', toggle({ label: 'Cover in the compact player', value: m.cover, onChange: put('cover') }), { gpath: 'mini.cover' }),
        row('Artist and album', toggle({ label: 'Artist and album', value: m.artist, onChange: put('artist') }), { gpath: 'mini.artist' }),
        row('Progress line in the compact player', toggle({ label: 'Progress line in the compact player', value: m.progress, onChange: put('progress') }), { gpath: 'mini.progress' })),
      group('Reset', h('div', { class: 'set-row' }, button('Back to the defaults', () => { setPref('mini', { ...DEFAULT_MINI }); render(); }))));
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
      group('Visualisers', row('Show visualisers', toggle({ label: 'Show visualisers', value: featureOn('viz'), onChange: (v) => setPref('features.viz', v) }), { hint: 'Switch off to hide every visualiser and the level meters in all skins. Also on the Now Playing top bar (Shift+V).', gpath: 'features.viz' })),
      group('Centrepiece', row('Shows', segmented({ label: 'Centrepiece', options: HERO_MODES, value: c.hero.mode, onChange: (v) => { set('hero.mode', v); render(); } }), { path: 'hero.mode', redo: again }),
        // A skin that owns these shows them under its own heading instead (see skinTab).
        c.hero.mode === 'turntable' && !getSkin(getPrefs().skin).options?.includes('tonearm') ? h('div', null, ...turntableRows()) : null),
      group(c.hero.mode === 'viz' ? 'Centrepiece visualiser' : 'Main visualiser', pickGrid(heroTarget, cur), optionsFor(cur)),
      group('Visualiser panel', h('p', { class: 'hint' }, 'The Visualiser and Level meters panels can sit anywhere (see Layout).'), pickGrid('viz.side', c.viz.side), optionsFor(c.viz.side)),
      group('Drawing',
        row('Colour', select({ label: 'Visualiser colour', value: c.viz.color, options: [{ value: 'accent', label: 'Accent' }, { value: 'fg', label: 'Text colour' }, { value: 'gradient', label: 'Accent gradient' }, { value: 'art', label: 'Cover colours' }, { value: 'rainbow', label: 'Rainbow' }], onChange: (v) => set('viz.color', v) }), { path: 'viz.color', redo: again }),
        row('Frame rate', segmented({ label: 'Visualiser frame rate', options: [{ value: 30, label: '30' }, { value: 60, label: '60' }, { value: 120, label: '120' }], value: c.viz.fps, onChange: (v) => set('viz.fps', v) }), { hint: 'Lower saves battery.', path: 'viz.fps', redo: again })),
      group('Performance', h('p', { class: 'hint' }, 'Redshift stops animating when nothing is moving, so a paused player is nearly idle. These two cut the cost while music plays.'),
        row('Reduce effects', toggle({ label: 'Reduce effects', value: !!getPrefs().perf?.lite, onChange: (v) => setPref('perf.lite', v) }), { hint: 'Drops the blur behind panels, blend modes and film grain, and caps the visualiser at 30 fps. Looks a little flatter; uses much less GPU.', gpath: 'perf.lite' }),
        row('Slow down in the background', toggle({ label: 'Slow down in the background', value: getPrefs().perf?.background !== false, onChange: (v) => setPref('perf.background', v) }), { hint: 'Caps the visualiser at 20 fps while the window is not focused.', gpath: 'perf.background' })));
  }

  // ---------- Equaliser ----------
  // The same equaliser panel as the Now Playing drawer, mounted here so it can be set without leaving Settings.
  function eqTab() {
    const host = h('div', { class: 'eq-host eq-inline' });
    const panel = mountEq(host);
    previews.push(panel);
    return h('div', { class: 'set-page' }, group('Equaliser', h('p', { class: 'hint' }, 'Ten bands, a pre-amp, built-in presets and your own saved presets. Changes apply immediately. Press E on the Now Playing screen to open it as a drawer instead. Double-click a band to reset it.'), host));
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
      group('Library',
        row('Playing a track from a list', segmented({ label: 'Playing a track from a list', options: [{ value: 'single', label: 'Just that track' }, { value: 'list', label: 'Continue through the list' }], value: getPrefs().library.playMode ?? 'single', onChange: (v) => setPref('library.playMode', v) }),
          { hint: 'What double-clicking a track or pressing its play button does. The right-click menu always offers both.' }),
        row('Watch music folders', toggle({ label: 'Watch music folders', value: state.settings?.watch_folders ?? true, onChange: async (v) => { await act(api.setWatchFolders(v)); await refreshSettings(); render(); } }),
          { hint: 'Looks at your library folders about once a minute and adds, updates or removes songs when files change. Turn it off to rescan only by hand.' })),
      group('Play counts and history',
        row('A song counts as played after', slider({ min: 1, max: 100, step: 1, value: state.settings?.count_percent ?? 50, reset: 50, label: 'Percentage of a song to hear before it counts as played', format: (v) => `${Math.round(v)}%`,
          onChange: async (v) => { await act(api.setCountPercent(v)); await refreshSettings(); } }),
          { hint: 'How much of a song you need to hear for it to count as a play, as a share of its length. It sets the play counts, the Most played lists, the Stats screen and your mixes. Skipping past a song doesn\'t count. Double-click the slider to go back to 50%. Songs that are already counted stay counted.' }),
        row('Keep a listening history', toggle({ label: 'Keep a listening history', value: state.settings?.record_history ?? true, onChange: async (v) => { await act(api.setRecordHistory(v)); await refreshSettings(); render(); } }),
          { hint: 'Remembers when and for how long you listen, for the Stats screen and your mixes. It is stored on this computer only and never sent anywhere.' }),
        h('div', { class: 'set-row set-actions' }, button('Clear listening history…', async () => {
          if (!await confirmDialog('Clear listening history?', 'The Stats screen and your mixes start again from nothing. Play counts, favourites and playlists are not touched.', { ok: 'Clear history' })) return;
          try { await act(api.clearHistory()); toast('Listening history cleared'); } catch (e) { toast(`Could not clear it: ${e}`, 'error'); }
        }, { cls: 'danger' }))),
      group('Startup', startup()),
      group('Window', tray()));
  }

  /** What Redshift does when it opens. */
  function startup() {
    const on = state.settings?.restore_session ?? true;
    return row('Continue where you left off', toggle({ label: 'Continue where you left off', value: on, onChange: async (v) => { await act(api.setRestoreSession(v)); await refreshSettings(); render(); } }),
      { hint: 'Opens with your last queue loaded and the song you were on paused at the place you stopped. Nothing plays until you press play.' });
  }

  /** Minimise-to-tray options. They're saved by the app and also tell it to show or hide the tray icon. */
  function tray() {
    const t = state.settings?.tray ?? { minimize_to_tray: false, close_to_tray: false };
    const save = async (patch) => {
      const next = { ...t, ...patch };
      await act(api.setTray(next.minimize_to_tray, next.close_to_tray));
      await refreshSettings();
      render();
    };
    return h('div', null,
      row('Minimise to tray', toggle({ label: 'Minimise to tray', value: t.minimize_to_tray, onChange: (v) => save({ minimize_to_tray: v }) }), { hint: 'Minimising hides Redshift in the system tray. Music keeps playing. Click the tray icon to bring it back.' }),
      row('Close button hides to tray', toggle({ label: 'Close button hides to tray', value: t.close_to_tray, onChange: (v) => save({ close_to_tray: v }) }), { hint: 'The X button hides the window instead of quitting. Use Quit in the tray menu to exit.' }));
  }

  // ---------- Lyrics & info ----------
  /** The real lyrics styling (this skin's own CSS included) on made-up lines, stepping along like a song. */
  function lyricsPreview() {
    const text = ['Streetlights hum along the shore', 'I counted every passing car', 'Your name was written in the rain', 'Now the colours start to bend', 'Everything is moving slow', 'Hold me where the signal fades', 'Let the static carry on'];
    const lines = text.map((t) => h('p', { class: 'ly-line' }, t));
    const scroller = h('div', { class: 'ly-scroll' }, lines);
    const inner = h('div', { class: 'lyrics' }, scroller);
    const wrap = h('div', { class: 'panel ly-preview', dataset: { panel: 'lyrics', slot: 'right', synced: 'true' } }, inner);
    let at = 2;
    const paint = (jump = false) => {
      wrap.style.setProperty('--ly-size', String(cfg().lyrics.size));
      wrap.style.setProperty('--ly-dim', String(cfg().lyrics.dim));
      wrap.style.setProperty('--ly-blur', `${cfg().lyrics.blur}px`);
      wrap.dataset.align = inner.dataset.align = cfg().lyrics.align;
      lines.forEach((p, i) => { p.classList.toggle('active', i === at); p.classList.toggle('past', i < at); p.dataset.d = String(Math.min(4, Math.abs(i - at))); });
      const p = lines[at];
      scroller.scrollTo({ top: p.offsetTop - scroller.clientHeight / 2 + p.offsetHeight / 2, behavior: jump ? 'auto' : 'smooth' });
    };
    const off = on('ui', () => paint(true));
    const timer = setInterval(() => { at = (at + 1) % lines.length; paint(); }, 2600);
    previews.push({ destroy: () => { clearInterval(timer); off(); } });
    requestAnimationFrame(() => paint(true));
    return wrap;
  }

  function lyricsTab() {
    const c = cfg();
    const sample = lyricsPreview();
    return h('div', { class: 'set-page' },
      group('Lyrics', row('Show lyrics', toggle({ label: 'Show lyrics', value: featureOn('lyrics'), onChange: (v) => setPref('features.lyrics', v) }), { hint: 'Switch off to hide the lyrics panel in all skins. Also on the Now Playing top bar (L).', gpath: 'features.lyrics' }), sample,
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
        row('Show times', toggle({ label: 'Show times', value: c.seek.times, onChange: (v) => set('seek.times', v) }), { path: 'seek.times', redo: again }),
        row('Click total time for time left', toggle({ label: 'Click total time for time left', value: featureOn('timeLeft'), onChange: (v) => setPref('features.timeLeft', v) }), { hint: 'Clicking the right-hand time switches between the track length and the time remaining.' })),
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

  // ---------- Help ----------
  function helpTab() {
    return h('div', { class: 'set-page' },
      group('Help', h('p', { class: 'hint' }, 'A short guide to what Redshift can do, and every keyboard shortcut. F1 opens it from anywhere.'),
        h('div', { class: 'set-row set-actions' },
          button('Open the guide', () => openHelp('guide')),
          button('Shortcut list', () => openHelp('keys')))),
      group('About', aboutBlock()));
  }

  // ---------- Data ----------
  function data() {
    return h('div', { class: 'set-page' },
      group('Share a look', h('p', { class: 'hint' }, 'Save the current skin, with your changes, as a small file, or add a skin someone sent you. A loaded skin shows up with your own.'),
        h('div', { class: 'set-row set-actions' },
          button('Export this skin to a file…', async () => {
            const file = lookFile();
            const dest = await pickSave(`${file.name.replace(/[^\w .-]+/g, '').trim() || 'skin'}.redshift-skin.json`, [{ name: 'Redshift skin', extensions: ['json'] }]).catch(() => null);
            if (!dest) return;
            try { await api.writeLookFile(dest, JSON.stringify(file, null, 2)); toast('Skin saved'); } catch (e) { toast(`Couldn't save the file: ${e}`, 'error'); }
          }),
          button('Add a skin from a file…', async () => {
            const src = await pickFile([{ name: 'Redshift skin', extensions: ['json'] }]).catch(() => null);
            if (!src) return;
            let data = null;
            try { data = JSON.parse(await api.readLookFile(src)); } catch { /* reported below */ }
            const look = data ? importLook(data) : null;
            if (look) { toast(`Added “${look.name}”`); render(); } else toast('That file isn\'t a Redshift skin, or you have too many skins already.', 'error');
          }),
          button('Add a skin from the clipboard', async () => {
            let data = null;
            try { data = JSON.parse(await navigator.clipboard.readText()); } catch { /* reported below */ }
            const look = data ? importLook(data) : null;
            if (look) { toast(`Added “${look.name}”`); render(); } else toast('The clipboard doesn\'t contain a Redshift skin.', 'error');
          }))),
      group('Reset', row('Everything', button('Reset all appearance settings', async () => { if (await confirmDialog('Reset everything?', 'Every skin goes back to its defaults and shortcuts are restored. Your music, playlists and playback settings are untouched.', { ok: 'Reset all' })) { const { setPref } = await import('../prefs.js'); setPref('skins', {}); setPref('keys', {}); render(); } }, { cls: 'danger' }))));
  }

  const TAB_BUILDERS = { appearance, skin: skinTab, layout, visual, playback, eq: eqTab, lyrics: lyricsTab, ambient: ambientPage, mini: miniPage, keys: keysTab, data, help: helpTab };
  on('ui', () => { if (state.view === 'settings' && getPrefs().skin !== renderedSkin) render(); });
  on('view', (v) => { if (v === 'settings') render(); else { previews.forEach((p) => p.destroy?.()); previews = []; } });
  render();
  return { render };
}
