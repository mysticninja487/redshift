// Languages. The interface is written in English, and this file translates it as it is drawn:
// every piece of text and every title / aria-label / placeholder on the page is looked up, whole,
// in the chosen language's dictionary (i18n/<id>.js) and swapped. Anything without an entry stays in
// English. Dictionary keys may hold {name} slots for text that has numbers or names in it
// ("Added {n} tracks to the queue"). Code that builds text out of pieces calls t().
//
// Marking an element translate="no" keeps its text as it is (song titles, lyrics, names).

import { on } from './bus.js';
import { getPrefs } from './prefs.js';

export const LANGUAGES = [['en', 'English'], ['bn', 'বাংলা'], ['es', 'Español'], ['fr', 'Français'], ['de', 'Deutsch'], ['ja', '日本語'], ['zh', '简体中文']];

const ATTRS = ['title', 'aria-label', 'placeholder', 'alt'];
const SKIP = new Set(['SCRIPT', 'STYLE', 'TEXTAREA', 'CODE', 'KBD']);

let lang = 'en';
let words = new Map();
let patterns = []; // [{ re, out, names }]
const texts = new WeakMap(); // text node -> { orig, shown }
const attrs = new WeakMap(); // element -> { [attr]: { orig, shown } }
let observer = null;

/** The language to use for a choice of 'auto' or a language id. */
export function resolveLanguage(choice) {
  const known = LANGUAGES.map(([id]) => id);
  if (known.includes(choice)) return choice;
  for (const tag of navigator.languages ?? [navigator.language ?? 'en']) {
    const base = String(tag).toLowerCase().split('-')[0];
    if (known.includes(base)) return base;
  }
  return 'en';
}

export const currentLanguage = () => lang;

const escapeRe = (s) => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');

function compile(dict) {
  words = new Map();
  patterns = [];
  for (const [en, out] of Object.entries(dict)) {
    if (!/\{\w+\}/.test(en)) { words.set(en, out); continue; }
    const names = [];
    // {n} and {m} hold numbers, so "{n} plays" can't swallow a song called "Halden Vey plays".
    const src = escapeRe(en).replace(/\\\{(\w+)\\\}/g, (_, n) => { names.push(n); return /^[nm]\d*$/.test(n) ? '(\\d[\\d.,]*)' : '(.+?)'; });
    patterns.push({ re: new RegExp(`^${src}$`, 's'), out, names });
  }
}

/** The translation of one string, or the string itself. */
export function tr(text) {
  if (lang === 'en' || !text) return text;
  const hit = words.get(text);
  if (hit !== undefined) return hit;
  for (const p of patterns) {
    const m = p.re.exec(text);
    if (m) return p.out.replace(/\{(\w+)\}/g, (_, n) => { const i = p.names.indexOf(n); return i >= 0 ? tr(m[i + 1]) : ''; });
  }
  return text;
}

/** For text assembled in code: t('Added {n} tracks', { n: 3 }). */
export function t(text, vars) {
  const base = vars ? text : tr(text);
  if (!vars) return base;
  const out = lang === 'en' ? text : (words.get(text) ?? text);
  return out.replace(/\{(\w+)\}/g, (_, k) => String(vars[k] ?? ''));
}

const lead = (s) => s.match(/^\s*/)[0];
const trail = (s) => s.match(/\s*$/)[0];
// Things that come from the user's music (titles, artists, lyrics, playlist and folder names). A song called
// "Home" or a genre called "Rock" must never be turned into another language.
const USER_TEXT = ['[translate="no"]', '.t-row:not(.t-head) .t-title', '.t-row:not(.t-head) .t-artist', '.t-row:not(.t-head) .t-album', '.q-name', '.q-artist', '.mini-title', '.mini-meta', '.mini-artist',
  '.mini-row-title', '.mini-row-artist', '.lyrics', '.ly-line', '.am-lyrics', '.am-title', '.am-artist', '.i-title', '.i-artist', '.i-album',
  '.i-album-name', '.d-title', '.d-artist', '.ac-title', '.ac-artist', '.ar-name', '.detail-title', '.pl-name', '.folder-path', '.si-path',
  '.si-sub', '.st-top-title', '.st-mix-name', '.st-genre-name', '.sty-title', '.sty-sub', '.quiet-album', '.quiet-meta'].join(',');
const skipped = (el) => !el || SKIP.has(el.tagName) || !!el.closest?.(USER_TEXT);

function doText(node) {
  const rec = texts.get(node);
  const now = node.data;
  const orig = rec && now === rec.shown ? rec.orig : now;
  const core = orig.trim();
  if (!core || skipped(node.parentElement)) { if (rec) texts.delete(node); return; }
  const out = tr(core);
  const shown = out === core ? orig : lead(orig) + out + trail(orig);
  texts.set(node, { orig, shown });
  if (node.data !== shown) node.data = shown;
}

function doAttrs(el) {
  if (skipped(el)) return;
  let recs = attrs.get(el);
  for (const a of ATTRS) {
    if (!el.hasAttribute(a)) { if (recs?.[a]) delete recs[a]; continue; }
    const now = el.getAttribute(a);
    const rec = recs?.[a];
    const orig = rec && now === rec.shown ? rec.orig : now;
    const shown = tr(orig.trim()) === orig.trim() ? orig : tr(orig.trim());
    (recs ??= attrs.set(el, {}).get(el))[a] = { orig, shown };
    if (now !== shown) el.setAttribute(a, shown);
  }
}

function walk(root) {
  if (root.nodeType === Node.TEXT_NODE) return doText(root);
  if (root.nodeType !== Node.ELEMENT_NODE) return;
  if (SKIP.has(root.tagName)) return;
  doAttrs(root);
  for (const child of root.childNodes) walk(child);
}

function watch() {
  if (observer) return;
  observer = new MutationObserver((list) => {
    for (const m of list) {
      if (m.type === 'characterData') doText(m.target);
      else if (m.type === 'attributes') doAttrs(m.target);
      else for (const n of m.addedNodes) walk(n);
    }
  });
  observer.observe(document.body, { subtree: true, childList: true, characterData: true, attributes: true, attributeFilter: ATTRS });
}

async function load(id) {
  if (id === 'en') return {};
  try { return (await import(`./i18n/${id}.js`)).default; } catch { return {}; }
}

/** Switches the interface to `id` ('auto' follows the system). Safe to call again. */
export async function setLanguage(choice) {
  const id = resolveLanguage(choice);
  const dict = await load(id);
  lang = id;
  compile(dict);
  document.documentElement.lang = id;
  document.title = tr('Redshift');
  if (document.body) { walk(document.body); watch(); }
}

/** Loads the saved language, then keeps the page in it as it changes. */
export async function initI18n() {
  let applied = getPrefs().language ?? 'auto';
  await setLanguage(applied);
  on('ui', () => {
    const next = getPrefs().language ?? 'auto';
    if (next !== applied) { applied = next; setLanguage(next); }
  });
}
