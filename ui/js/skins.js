// The skin registry. A skin is a whole composition (grid, type, decoration, default hero and
// visualiser), not just a colour set; CSS for each lives in css/skins/<id>.css. Anything a skin
// decides here is only a default: the person's own choices are stored per skin on top of it.
//
// `vizOff` says what a skin's centrepiece becomes when visualisers are switched off and the skin's
// hero is a visualiser: 'quiet' (the album set as type) or 'none' (the skin's type and colour alone).
//
// Slots are the places panels can sit: top, left, stage (the big middle), right, bottom. Each skin
// gives them its own geometry; a panel dropped in a different slot restyles itself to fit.

export const SLOTS = ['top', 'left', 'stage', 'right', 'bottom'];

export const PANELS = [
  { id: 'hero', name: 'Hero', blurb: 'The big centrepiece: turntable, cover art or a visualiser.' },
  { id: 'info', name: 'Now playing', blurb: 'Title, artist, album and the file\'s technical details.' },
  { id: 'transport', name: 'Controls', blurb: 'Play, skip, shuffle, repeat and volume.' },
  { id: 'seek', name: 'Progress', blurb: 'Position bar with elapsed and remaining time.' },
  { id: 'queue', name: 'Up next', blurb: 'The play queue; drag to reorder.' },
  { id: 'lyrics', name: 'Lyrics', blurb: 'Synced lyrics that follow the song.' },
  { id: 'viz', name: 'Visualiser', blurb: 'A second visualiser, independent of the hero.' },
  { id: 'meters', name: 'Level meters', blurb: 'Left and right level with peak hold.' },
];

/** How much room each panel wants, and how much a slot has. A column holds about three tall panels and a strip five;
    Settings greys out a slot that is full, so a layout can't be built that runs off the window. */
export const PANEL_WEIGHT = { hero: 2, info: 2, transport: 1, seek: 1, queue: 2, lyrics: 2, viz: 2, meters: 1, eq: 2 };
export const SLOT_CAPACITY = { left: 6, right: 6, stage: 9, top: 5, bottom: 5 };
export const slotLoad = (ids, strip) => ids.reduce((n, id) => n + (strip ? 1 : (PANEL_WEIGHT[id] ?? 2)), 0);
export const fitsSlot = (slot, ids) => slotLoad(ids, slot === 'top' || slot === 'bottom') <= SLOT_CAPACITY[slot];

export const FONTS = [
  { id: 'Inter', label: 'Inter', stack: '"Inter", "Segoe UI", system-ui, sans-serif' },
  { id: 'Space Grotesk', label: 'Space Grotesk', stack: '"Space Grotesk", "Segoe UI", system-ui, sans-serif' },
  { id: 'Cormorant Garamond', label: 'Cormorant Garamond', stack: '"Cormorant Garamond", Georgia, "Times New Roman", serif' },
  { id: 'Barlow Condensed', label: 'Barlow Condensed', stack: '"Barlow Condensed", "Arial Narrow", "Segoe UI", sans-serif' },
  { id: 'JetBrains Mono', label: 'JetBrains Mono', stack: '"JetBrains Mono", Consolas, "Cascadia Mono", monospace' },
  { id: 'Silkscreen', label: 'Silkscreen (pixel)', stack: '"Silkscreen", "JetBrains Mono", monospace' },
  { id: 'system', label: 'System UI', stack: '"Segoe UI Variable", "Segoe UI", system-ui, sans-serif' },
  { id: 'Georgia', label: 'Georgia', stack: 'Georgia, "Times New Roman", serif' },
  { id: 'Bahnschrift', label: 'Bahnschrift', stack: 'Bahnschrift, "Arial Narrow", "Segoe UI", sans-serif' },
];
/** CSS family name an imported font file is registered under. */
export const importedFamily = (file) => `Redshift import ${cleanFontName(file)}`;
/** Makes a typed font name safe to put inside a CSS string. */
export const cleanFontName = (name) => String(name).replace(/["'\\;{}<>\n\r]/g, '').trim().slice(0, 80);

/**
 * A CSS font stack for a font id: one of the built-in ids, `file:<name>` for a font the person
 * imported, or `local:<Family>` for any font installed on their PC.
 */
export function fontStack(id) {
  if (typeof id === 'string' && id.startsWith('file:')) return `"${importedFamily(id.slice(5))}", "Segoe UI", system-ui, sans-serif`;
  if (typeof id === 'string' && id.startsWith('local:')) {
    const name = cleanFontName(id.slice(6));
    if (name) return `"${name}", "Segoe UI", system-ui, sans-serif`;
  }
  return FONTS.find((f) => f.id === id)?.stack ?? FONTS[0].stack;
}

/** Accent presets for the "Preset" colour mode. */
export const ACCENTS = [
  { id: 'crimson', name: 'Crimson', hex: '#e2312d' },
  { id: 'vermilion', name: 'Vermilion', hex: '#ff5a36' },
  { id: 'tangerine', name: 'Tangerine', hex: '#ff7a1a' },
  { id: 'amber', name: 'Amber', hex: '#f2b134' },
  { id: 'lime', name: 'Lime', hex: '#b6e63c' },
  { id: 'mint', name: 'Mint', hex: '#34d6a0' },
  { id: 'cyan', name: 'Cyan', hex: '#2fc3e6' },
  { id: 'azure', name: 'Azure', hex: '#3b82f6' },
  { id: 'violet', name: 'Violet', hex: '#9b7cff' },
  { id: 'rose', name: 'Rose', hex: '#ff5c9d' },
  { id: 'graphite', name: 'Graphite', hex: '#8a8f98' },
];

const SKINS = [
  {
    id: 'classic',
    name: 'Classic',
    blurb: 'The familiar layout: a sidebar, the cover and track in the middle, lyrics and the queue beside them, a player bar along the bottom.',
    scheme: 'dark',
    hasDecor: false,
    vizOff: 'none',
    palette: {
      dark: { bg: '#101216', bg2: '#171a20', bg3: '#21252d', fg: '#eceef2', accent: '#e2312d' },
      light: { bg: '#f2f3f6', bg2: '#ffffff', bg3: '#e6e8ed', fg: '#15181d', accent: '#d42a26' },
    },
    fonts: { display: 'Inter', body: 'Inter', mono: 'JetBrains Mono' },
    defaults: {
      hero: { mode: 'art' },
      viz: { main: 'bars', side: 'bars' },
      seek: { style: 'line' },
      shape: { radius: 10, shadow: 0.7 },
      decor: false,
      layout: { slots: { left: [], stage: ['hero', 'info'], right: ['lyrics', 'queue'], bottom: ['transport', 'seek'], top: [] } },
    },
  },
  {
    id: 'turntable',
    name: 'Turntable',
    blurb: 'A record on the deck. Charcoal and warm paper split on the diagonal, with the tonearm tracking the song.',
    scheme: 'dark',
    palette: {
      dark: { bg: '#14161a', bg2: '#1b1e24', bg3: '#262a33', fg: '#f0ebdf', accent: '#e2312d', paper: '#e3d8c3', ink: '#1b1a18' },
      light: { bg: '#e3d8c3', bg2: '#ebe2d0', bg3: '#f3ecdd', fg: '#1b1a18', accent: '#d42a26', paper: '#14161a', ink: '#f0ebdf' },
    },
    fonts: { display: 'Space Grotesk', body: 'Inter', mono: 'JetBrains Mono' },
    // Options only this skin has (Settings shows them under the skin's own heading).
    options: ['spin', 'tonearm', 'plinth', 'label'],
    defaults: {
      hero: { mode: 'turntable', tonearm: true },
      viz: { main: 'bars', side: 'bars' },
      seek: { style: 'wave' },
      shape: { radius: 4 },
      layout: { slots: { left: ['queue'], stage: ['hero'], right: ['info', 'lyrics'], bottom: ['seek', 'transport'], top: [] } },
    },
  },
  {
    id: 'noir',
    name: 'Noir Studio',
    vizOff: 'quiet',
    blurb: 'A record sleeve in the dark. Hairline columns, a serif title and the whole song drawn as fine vertical lines.',
    scheme: 'dark',
    palette: {
      dark: { bg: '#0b0b0e', bg2: '#111116', bg3: '#1a1a21', fg: '#ece8e1', accent: '#e0303c' },
      light: { bg: '#e9e6e0', bg2: '#f0eee9', bg3: '#f7f5f1', fg: '#111114', accent: '#c9232f' },
    },
    fonts: { display: 'Cormorant Garamond', body: 'Barlow Condensed', mono: 'JetBrains Mono' },
    defaults: {
      hero: { mode: 'viz', viz: 'lines' },
      viz: { main: 'lines', side: 'lines' },
      seek: { style: 'line' },
      shape: { radius: 0, border: 1 },
      decor: true,
      layout: { slots: { stage: ['hero'], bottom: ['info', 'transport', 'seek'], left: [], right: [], top: [] } },
    },
  },
  {
    id: 'wavelength',
    name: 'Wavelength',
    vizOff: 'none',
    blurb: 'Giant grotesk type over a blurred waveform. The background sweeps from light to dark through the cover\'s colours.',
    scheme: 'dark',
    palette: {
      dark: { bg: '#0a0b0e', bg2: '#111318', bg3: '#1a1d24', fg: '#f3f2ef', accent: '#ff5a36', g1: '#e9e9ee', g2: '#8d97ab', g3: '#0a1018', g4: '#4b2a19' },
      light: { bg: '#dedee0', bg2: '#e8e8ea', bg3: '#f2f2f3', fg: '#0c0c0e', accent: '#e8431f', g1: '#f3f3f6', g2: '#c3c8d3', g3: '#a9b0bf', g4: '#d7c3b3' },
    },
    fonts: { display: 'Inter', body: 'Inter', mono: 'JetBrains Mono' },
    defaults: {
      hero: { mode: 'viz', viz: 'blur' },
      viz: { main: 'blur', side: 'bars' },
      seek: { style: 'line' },
      shape: { radius: 0 },
      lyrics: { dim: 0.65 },
      layout: { slots: { stage: ['hero', 'info'], bottom: ['transport', 'seek'], left: [], right: [], top: [] } },
    },
  },
  {
    id: 'hud',
    name: 'Mist HUD',
    blurb: 'Cold grey haze, hairline targeting marks and crimson cut-corner chips. Titles glitch on the beat.',
    scheme: 'light',
    palette: {
      light: { bg: '#cdd1d4', bg2: '#dadddf', bg3: '#eceeef', fg: '#14171a', accent: '#e5274a' },
      dark: { bg: '#080b10', bg2: '#0f141b', bg3: '#171e28', fg: '#e9edf2', accent: '#ff7a2f' },
    },
    fonts: { display: 'Barlow Condensed', body: 'Barlow Condensed', mono: 'JetBrains Mono' },
    options: ['glitch'],
    defaults: {
      hero: { mode: 'art', glitch: true },
      viz: { main: 'spectrogram', side: 'ridges' },
      seek: { style: 'ticks' },
      shape: { radius: 0, grain: 0.5 },
      layout: { slots: { left: ['meters', 'queue'], stage: ['hero', 'info'], right: ['lyrics'], bottom: ['seek', 'transport'], top: [] } },
    },
  },
  {
    id: 'paper',
    name: 'Paper',
    blurb: 'A printed page. Halftone cover on a red sun, ruled axes and a timeline you read like a ruler.',
    scheme: 'light',
    palette: {
      light: { bg: '#e7e3da', bg2: '#eeebe3', bg3: '#f5f2ec', fg: '#1a1918', accent: '#c8201e' },
      dark: { bg: '#161519', bg2: '#1d1c21', bg3: '#26252b', fg: '#e8e3da', accent: '#e24b44' },
    },
    fonts: { display: 'Cormorant Garamond', body: 'Inter', mono: 'JetBrains Mono' },
    options: ['sun', 'halftone'],
    defaults: {
      hero: { mode: 'art', halftone: true, sun: true },
      viz: { main: 'lines', side: 'lines' },
      seek: { style: 'ticks' },
      shape: { radius: 0, grain: 0.6 },
      layout: { slots: { left: ['info'], stage: ['hero'], right: ['lyrics'], bottom: ['seek', 'transport'], top: [] } },
    },
  },
  {
    id: 'soft',
    name: 'Soft Console',
    vizOff: 'none',
    blurb: 'Pressed-plastic controls, real knobs and a glowing sphere on its orbit. Quiet, tactile, a little clinical.',
    scheme: 'light',
    palette: {
      light: { bg: '#e5e7eb', bg2: '#eceef1', bg3: '#f6f7f9', fg: '#24262b', accent: '#d3203f' },
      dark: { bg: '#181a1f', bg2: '#1f2229', bg3: '#2a2e37', fg: '#e5e7eb', accent: '#ff3b5c' },
    },
    fonts: { display: 'Space Grotesk', body: 'Inter', mono: 'JetBrains Mono' },
    hasDecor: false, // no background decoration layers, so Settings leaves out the Decoration switch
    defaults: {
      hero: { mode: 'viz', viz: 'orb' },
      viz: { main: 'orb', side: 'bars' },
      seek: { style: 'line' },
      shape: { radius: 18, shadow: 1.2 },
      transport: { volume: 'knob' },
      layout: { slots: { left: [], stage: ['hero', 'info'], right: ['queue'], bottom: ['transport', 'seek'], top: [] } },
    },
  },
  {
    id: 'pixel',
    name: 'Pixel Mono',
    vizOff: 'quiet',
    blurb: 'A pocket recorder\'s spec sheet. Pill buttons, dotted outlines, pixel display type on a grainy orange glow.',
    scheme: 'light',
    palette: {
      light: { bg: '#e8e4e0', bg2: '#f0ede9', bg3: '#faf8f6', fg: '#131313', accent: '#ff6a13' },
      dark: { bg: '#11100f', bg2: '#1a1918', bg3: '#242221', fg: '#eee8e1', accent: '#ff7a2b' },
    },
    fonts: { display: 'Silkscreen', body: 'JetBrains Mono', mono: 'JetBrains Mono' },
    defaults: {
      hero: { mode: 'viz', viz: 'dots' },
      viz: { main: 'dots', side: 'dots' },
      seek: { style: 'dots' },
      shape: { radius: 999, grain: 0.7 },
      layout: { slots: { left: ['info'], stage: ['hero'], right: ['queue'], bottom: ['seek', 'transport'], top: [] } },
    },
  },
];

export const skins = SKINS;
export const getSkin = (id) => SKINS.find((s) => s.id === id) ?? SKINS[0];
