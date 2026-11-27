// The filter panel of the Library: rules over every tag and file property, plus quick filters.
// It edits a plain filter object (see filters.js) and calls `onChange` when the results should be redrawn.

import { DATE_UNITS, FIELDS, PRESETS, QUALITY, emptyFilter, fieldById, newRule, opsFor, ruleActive } from '../filters.js';
import { clear, debounce, h } from '../util.js';
import { button, iconButton, select } from './controls.js';

const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);

/** A preset is "on" when the filter already holds each of its rules. */
const holds = (filter, preset) => preset.rules.every((p) => filter.rules.some((r) => r.field === p.field && r.op === p.op && same(r.a ?? '', p.a ?? '') && (p.unit ? r.unit === p.unit : true)));

export function filterPanel({ onChange }) {
  const filter = emptyFilter();
  let formats = [], playlists = [];
  const changed = debounce(() => onChange(), 180);
  const rulesBox = h('div', { class: 'lf-rules' });
  const chips = h('div', { class: 'lf-presets', role: 'group', 'aria-label': 'Quick filters' });
  const matchSeg = h('div', { class: 'segmented lf-match', role: 'tablist' },
    ['all', 'any'].map((m) => h('button', { class: 'seg-btn', type: 'button', role: 'tab', dataset: { match: m }, onclick: () => { filter.match = m; paintMatch(); onChange(); } }, m === 'all' ? 'All of' : 'Any of')));
  const el = h('div', { class: 'lf-panel', hidden: true },
    h('div', { class: 'lf-head' }, h('span', { class: 'lf-label' }, 'Quick filters'), chips),
    h('div', { class: 'lf-head' }, h('span', { class: 'lf-label' }, 'Show songs that match'), matchSeg, h('span', { class: 'lf-label' }, 'these rules')),
    rulesBox,
    h('div', { class: 'lf-foot' }, button('Add a filter', () => { filter.rules.push(newRule(pickField())); paintRules(); }, { iconName: 'plus' }), button('Clear filters', () => api.clear())));

  /** The first field that no rule uses yet, so a second click adds something new. */
  const pickField = () => FIELDS.find((f) => !filter.rules.some((r) => r.field === f.id))?.id ?? 'artist';

  const paintMatch = () => matchSeg.querySelectorAll('.seg-btn').forEach((b) => b.setAttribute('aria-selected', String(b.dataset.match === filter.match)));
  const paintChips = () => {
    clear(chips);
    for (const p of PRESETS) {
      const on = holds(filter, p);
      chips.append(h('button', { class: `lf-chip${on ? ' on' : ''}`, type: 'button', 'aria-pressed': String(on), onclick: () => {
        if (on) filter.rules = filter.rules.filter((r) => !p.rules.some((q) => q.field === r.field && q.op === r.op && same(r.a ?? '', q.a ?? '')));
        else filter.rules.push(...p.rules.filter((q) => !filter.rules.some((r) => r.field === q.field && r.op === q.op && same(r.a ?? '', q.a ?? ''))).map((q) => ({ ...newRule(q.field), ...q, a: q.a ?? (Array.isArray(newRule(q.field).a) ? [] : '') })));
        paintRules(); onChange();
      } }, p.label));
    }
  };

  const input = (rule, key, opts = {}) => h('input', { class: 'text-input lf-input', type: opts.type ?? 'text', value: rule[key] ?? '', placeholder: opts.hint ?? '', 'aria-label': opts.label ?? '', inputmode: opts.type === 'date' ? undefined : opts.mode,
    oninput: (e) => { rule[key] = e.target.value; refreshState(); changed(); } });

  /** Toggle buttons for choices (file types, qualities, playlists). */
  function choiceChips(rule, options) {
    const box = h('div', { class: 'lf-choices', role: 'group' });
    for (const [value, label] of options) {
      const b = h('button', { class: 'lf-chip', type: 'button', 'aria-pressed': String(rule.a.includes(value)), onclick: () => {
        rule.a = rule.a.includes(value) ? rule.a.filter((x) => x !== value) : [...rule.a, value];
        b.setAttribute('aria-pressed', String(rule.a.includes(value)));
        b.classList.toggle('on', rule.a.includes(value));
        refreshState(); onChange();
      } }, label);
      b.classList.toggle('on', rule.a.includes(value));
      box.append(b);
    }
    if (!options.length) box.append(h('span', { class: 'hint' }, 'Nothing to choose from yet.'));
    return box;
  }

  function valueControls(rule) {
    const f = fieldById(rule.field);
    const unary = ['empty', 'filled', 'never', 'ever', 'yes', 'no'].includes(rule.op);
    if (unary) return h('span', { class: 'lf-none' });
    if (f.kind === 'text') return input(rule, 'a', { label: `${f.label} value`, hint: 'words to look for' });
    if (f.kind === 'number') {
      const one = (key, label) => input(rule, key, { label, hint: f.hint, mode: 'decimal' });
      return rule.op === 'between' ? h('span', { class: 'lf-pair' }, one('a', `${f.label}, from`), h('span', { class: 'lf-and' }, 'and'), one('b', `${f.label}, to`)) : one('a', f.label);
    }
    if (f.kind === 'date') {
      if (rule.op === 'last') return h('span', { class: 'lf-pair' }, input(rule, 'a', { label: 'How many', hint: '30', mode: 'numeric' }),
        select({ label: 'Unit', value: rule.unit, options: DATE_UNITS.map(([value, label]) => ({ value, label })), onChange: (v) => { rule.unit = v; refreshState(); onChange(); } }));
      return rule.op === 'between' ? h('span', { class: 'lf-pair' }, input(rule, 'a', { type: 'date', label: 'From' }), h('span', { class: 'lf-and' }, 'and'), input(rule, 'b', { type: 'date', label: 'To' })) : input(rule, 'a', { type: 'date', label: f.label });
    }
    if (f.kind === 'choice') return choiceChips(rule, f.fixed ?? formats.map((x) => [x, x]));
    if (f.kind === 'playlist') return choiceChips(rule, [['*', 'Any playlist'], ...playlists.map((p) => [p.id, p.name])]);
    return h('span', { class: 'lf-none' });
  }

  function ruleRow(rule) {
    const f = fieldById(rule.field);
    const row = h('div', { class: 'lf-rule', dataset: { field: rule.field } },
      select({ label: 'Field', value: rule.field, options: FIELDS.map((x) => ({ value: x.id, label: x.label })), onChange: (v) => {
        const fresh = newRule(v); rule.field = v; rule.op = fresh.op; rule.a = fresh.a; rule.b = ''; rule.unit = 'days'; paintRules(); onChange();
      } }),
      select({ label: 'Condition', value: rule.op, options: opsFor(f).map(([value, label]) => ({ value, label })), onChange: (v) => {
        const wasList = Array.isArray(rule.a);
        rule.op = v; if (wasList && !Array.isArray(rule.a)) rule.a = []; paintRules(); onChange();
      } }),
      h('div', { class: 'lf-value' }, valueControls(rule)),
      iconButton('close', 'Remove this filter', () => { filter.rules = filter.rules.filter((r) => r !== rule); paintRules(); onChange(); }, { size: 16, cls: 'lf-remove' }));
    row.classList.toggle('idle', !ruleActive(rule));
    row._rule = rule;
    return row;
  }

  /** Marks rules that have no value yet (they do nothing) and the chips that are on. */
  function refreshState() {
    rulesBox.querySelectorAll('.lf-rule').forEach((row) => row.classList.toggle('idle', !ruleActive(row._rule)));
    paintChips();
  }

  function paintRules() {
    clear(rulesBox);
    if (!filter.rules.length) rulesBox.append(h('p', { class: 'hint lf-empty' }, 'No filters yet. Pick a quick filter above, or add one: any tag, file property, play count, date or playlist.'));
    else filter.rules.forEach((r) => rulesBox.append(ruleRow(r)));
    matchSeg.hidden = false;
    paintMatch(); paintChips();
    el.classList.toggle('has-rules', filter.rules.length > 0);
  }

  const api = {
    el,
    filter,
    /** What the choices are made from: the file types in the library and the playlists. */
    setLists(next) {
      if (next.formats) formats = next.formats;
      if (next.playlists) playlists = next.playlists;
      if (!el.hidden) paintRules();
    },
    show(on) { el.hidden = !on; if (on) paintRules(); },
    get visible() { return !el.hidden; },
    /** Rules that do something. */
    get count() { return filter.rules.filter(ruleActive).length; },
    clear() { filter.rules = []; filter.match = 'all'; paintRules(); onChange(); },
  };
  paintMatch();
  return api;
}
