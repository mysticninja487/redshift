// Fonts the person imported: each file is registered with the browser under a family name that
// skins.js's fontStack() knows how to ask for ("file:<name>" ids).

import { api, fontUrl } from './api.js';
import { importedFamily } from './skins.js';

let list = [];
const loaded = new Set();

export const importedFonts = () => list;

async function register(entry) {
  if (loaded.has(entry.file)) return;
  loaded.add(entry.file);
  try {
    const face = new FontFace(importedFamily(entry.file), `url("${fontUrl(entry.file)}")`);
    document.fonts.add(await face.load());
  } catch (e) {
    loaded.delete(entry.file);
    console.warn(`font ${entry.file} could not be loaded`, e);
  }
}

/** Reads the list of imported fonts and registers them all. Never throws. */
export async function loadImportedFonts() {
  try { list = await api.fonts(); } catch { list = []; }
  await Promise.all(list.map(register));
  return list;
}

/** Imports the font file at `path`, registers it, and returns its entry. */
export async function importFontFile(path) {
  const entry = await api.importFont(path);
  loaded.delete(entry.file); // a re-import may have changed the file
  await register(entry);
  list = await api.fonts();
  return entry;
}

export async function removeImportedFont(file) {
  await api.removeFont(file);
  list = list.filter((f) => f.file !== file);
  loaded.delete(file);
}
