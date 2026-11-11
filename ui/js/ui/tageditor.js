// The tag editor: title, artist, album and the rest, the cover picture, and lyrics (plain or synced)
// for one song, or the shared fields of several. Only what was changed is written.

import { api, coverUrl, pickFile } from '../api.js';
import { stampCount, stripStamps, textOfLyrics } from '../lrc.js';
import { h, plural, toast } from '../util.js';
import { button } from './controls.js';
import { confirmDialog, modal } from './dialog.js';
import { syncLyrics } from './lyricsync.js';

const FIELDS = [
  { key: 'title', label: 'Title', wide: true, single: true },
  { key: 'artist', label: 'Artist' },
  { key: 'album', label: 'Album' },
  { key: 'album_artist', label: 'Album artist' },
  { key: 'genre', label: 'Genre' },
  { key: 'composer', label: 'Composer' },
  { key: 'year', label: 'Year', num: true, max: 9999 },
  { key: 'track_no', label: 'Track number', num: true, single: true },
  { key: 'track_total', label: 'Tracks in album', num: true },
  { key: 'disc_no', label: 'Disc number', num: true },
  { key: 'disc_total', label: 'Discs in set', num: true },
  { key: 'comment', label: 'Comment', wide: true, area: true },
];
const MAX_TRACKS = 500;
const IMAGE_FILTERS = [{ name: 'Pictures', extensions: ['jpg', 'jpeg', 'png'] }];
const LYRIC_TARGETS = [['tags', 'In the song’s tags'], ['file', 'In a .lrc file next to the song'], ['both', 'Both']];
const baseName = (p) => p.split(/[\\/]/).pop();

/**
 * Opens the editor for `paths`. `tab` is 'details', 'art' or 'lyrics'. Resolves to true when
 * something was saved.
 */
export async function openTagEditor(paths, { tab = 'details' } = {}) {
  paths = [...new Set(paths)].slice(0, MAX_TRACKS);
  if (!paths.length) return false;
  const single = paths.length === 1;
  let forms;
  try { forms = await Promise.all(paths.map((p) => api.tagForm(p))); } catch (e) { toast(`Can't read the tags: ${e?.message ?? e}`, 'error'); return false; }
  let currentLyrics = null;
  if (single) { try { currentLyrics = await api.lyrics(paths[0]); } catch { /* none */ } }

  let isDirty = () => false;
  const guard = async () => !isDirty() || confirmDialog('Discard your changes?', 'The edits you made to these tags haven’t been saved.', { ok: 'Discard' });
  return modal((box, done, cancel) => {
    const readOnly = forms.some((f) => !f.writable);
    const inputs = new Map(), touched = new Set(), initial = new Map();
    let cover = null; // null: unchanged; 'remove'; or { file, url }
    let saving = false;

    // ---- details ----
    const detailRows = FIELDS.filter((f) => single || !f.single).map((f) => {
      const vals = forms.map((x) => x[f.key] ?? '');
      const same = vals.every((v) => v === vals[0]);
      const start = same && vals[0] !== '' ? String(vals[0]) : '';
      initial.set(f.key, { start, mixed: !same });
      const input = h(f.area ? 'textarea' : 'input', { class: `text-input tg-input${f.area ? ' tg-area' : ''}`, id: `tg-${f.key}`, value: f.area ? undefined : start, rows: f.area ? 2 : undefined,
        type: f.area ? undefined : 'text', inputmode: f.num ? 'numeric' : undefined, placeholder: same ? '' : 'Different in each song', autocomplete: 'off', spellcheck: f.num ? 'false' : 'true',
        oninput: () => { touched.add(f.key); input.removeAttribute('aria-invalid'); touchedAny(); } });
      if (f.area) input.value = start;
      inputs.set(f.key, input);
      return h('div', { class: `tg-field${f.wide ? ' wide' : ''}${f.num ? ' num' : ''}` }, h('label', { for: `tg-${f.key}`, class: 'tg-label' }, f.label), input);
    });
    const details = h('div', { class: 'tg-pane tg-grid', role: 'tabpanel', 'aria-label': 'Details' }, detailRows);

    // ---- artwork ----
    const art = h('img', { class: 'tg-art-img', alt: '' });
    const artNote = h('p', { class: 'hint tg-art-note' });
    const artEmpty = h('div', { class: 'tg-art-empty' }, 'No artwork');
    const artFrame = h('div', { class: 'tg-art-frame' }, art, artEmpty);
    const hasCover = forms.some((f) => f.has_cover), mixedCover = !single && forms.some((f) => f.has_cover !== forms[0].has_cover);
    const undoCover = button('Undo change', () => { setCover(null); }, { cls: 'tg-undo' });
    const removeBtn = button('Remove artwork', () => setCover('remove'), { cls: 'danger' });
    function setCover(c) {
      if (cover && cover !== 'remove') URL.revokeObjectURL(cover.url);
      cover = c;
      const shown = cover === 'remove' ? null : cover ? cover.url : hasCover ? `${coverUrl(paths[0], 600)}` : null;
      art.hidden = !shown; artEmpty.hidden = !!shown;
      if (shown) art.src = shown; else art.removeAttribute('src');
      artEmpty.textContent = cover === 'remove' ? 'Artwork will be removed' : mixedCover ? 'The songs have different artwork' : 'No artwork';
      artNote.textContent = cover === 'remove' ? 'Saving takes the picture out of the file.'
        : cover ? `New picture${single ? '' : ` for all ${paths.length} songs`}, saved when you press Save.`
        : hasCover ? (single ? 'This picture is stored in the song.' : 'Showing the first song’s picture.')
        : 'Songs without a stored picture use a cover.jpg or folder.jpg in their folder, if there is one.';
      undoCover.hidden = !cover; removeBtn.disabled = cover === 'remove' || (!cover && !hasCover);
      touchedAny();
    }
    const chooseBtn = button('Choose picture…', async () => {
      let file = null;
      try { file = await pickFile(IMAGE_FILTERS); } catch (e) { toast(String(e?.message ?? e), 'error'); }
      if (!file) return;
      try { setCover({ file, url: URL.createObjectURL(await api.imagePreview(file)) }); } catch (e) { toast(`That picture can't be used: ${e?.message ?? e}`, 'error'); }
    }, { cls: 'primary' });
    const artPane = h('div', { class: 'tg-pane tg-art', role: 'tabpanel', 'aria-label': 'Artwork' }, artFrame,
      h('div', { class: 'tg-art-side' }, h('div', { class: 'tg-art-actions' }, chooseBtn, removeBtn, undoCover), artNote,
        h('p', { class: 'hint' }, 'JPEG and PNG pictures up to 16 MB. Large pictures make the file bigger, so around 1000 × 1000 is plenty.')));
    art.addEventListener('error', () => { if (!cover) { art.hidden = true; artEmpty.hidden = false; } });

    // ---- lyrics (one song at a time) ----
    const embedded = single ? forms[0].lyrics : '';
    const area = h('textarea', { class: 'text-input tg-lyrics', rows: 14, spellcheck: 'false', 'aria-label': 'Lyrics', placeholder: 'Paste or type the lyrics. Put [mm:ss.xx] in front of a line to time it.', oninput: () => { lyricsDirty = true; lyricsInfo(); touchedAny(); } });
    area.value = embedded;
    let lyricsDirty = false;
    const target = h('select', { class: 'select', 'aria-label': 'Save lyrics', onchange: () => touchedAny() }, LYRIC_TARGETS.map(([v, l]) => h('option', { value: v }, l)));
    const lyInfo = h('p', { class: 'hint tg-ly-info' });
    const sidecarNote = h('div', { class: 'hint warn tg-ly-side', hidden: true });
    const lyricsInfo = () => {
      const { total, stamped } = stampCount(area.value);
      lyInfo.textContent = !total ? 'No lyrics.' : stamped === total ? `${plural(total, 'line')}, all timed: they will follow the song.`
        : stamped ? `${stamped} of ${plural(total, 'line')} timed. Use Sync lyrics to time the rest.` : `${plural(total, 'line')}, not timed: they will scroll but not follow the song.`;
      syncBtn.disabled = !total; stripBtn.disabled = !stamped;
    };
    const syncBtn = button('Sync lyrics…', async () => {
      const out = await syncLyrics(paths[0], area.value);
      if (out != null) { area.value = out; lyricsDirty = true; lyricsInfo(); touchedAny(); }
    }, { cls: 'primary', iconName: 'lyrics' });
    const stripBtn = button('Remove times', () => { area.value = stripStamps(area.value); lyricsDirty = true; lyricsInfo(); touchedAny(); });
    const clearBtn = button('Clear', () => { area.value = ''; lyricsDirty = true; lyricsInfo(); touchedAny(); area.focus(); });
    if (single && !embedded && currentLyrics?.lines?.length) {
      const where = currentLyrics.source === 'sidecar' ? 'a lyrics file next to the song' : 'the song';
      sidecarNote.hidden = false;
      sidecarNote.append(`This song already has lyrics in ${where}. `, button('Load them here', () => {
        area.value = textOfLyrics(currentLyrics); lyricsDirty = true; sidecarNote.hidden = true; lyricsInfo(); touchedAny();
      }, { cls: 'sy-small' }));
    }
    const lyricsPane = h('div', { class: 'tg-pane tg-lyr', role: 'tabpanel', 'aria-label': 'Lyrics' }, sidecarNote, area, lyInfo,
      h('div', { class: 'tg-ly-actions' }, syncBtn, stripBtn, clearBtn, h('label', { class: 'tg-ly-target' }, 'Save', target)));
    if (single) lyricsInfo();

    // ---- tabs ----
    const TABS = [['details', 'Details', details], ['art', 'Artwork', artPane], ...(single ? [['lyrics', 'Lyrics', lyricsPane]] : [])];
    let active = TABS.some(([id]) => id === tab) ? tab : 'details';
    const tabBtns = TABS.map(([id, label]) => h('button', { class: 'lib-tab tg-tab', type: 'button', role: 'tab', dataset: { tab: id }, onclick: () => show(id) }, label));
    function show(id) {
      active = id;
      tabBtns.forEach((b) => { const on = b.dataset.tab === id; b.setAttribute('aria-selected', String(on)); b.tabIndex = on ? 0 : -1; });
      TABS.forEach(([t, , pane]) => { pane.hidden = t !== id; });
      if (id === 'lyrics') setTimeout(() => area.focus());
    }

    // ---- saving ----
    const parseNum = (f, v) => {
      const t = v.trim();
      if (!t) return null;
      if (!/^\d+$/.test(t) || +t > (f.max ?? 99999) || +t < 1) throw Object.assign(new Error(`${f.label} should be a whole number${f.max ? ` up to ${f.max}` : ''}.`), { key: f.key });
      return +t;
    };
    function collect() {
      const edit = {};
      for (const f of FIELDS) {
        if (!touched.has(f.key) || !inputs.has(f.key)) continue;
        const v = inputs.get(f.key).value, init = initial.get(f.key);
        if (!init.mixed && v === init.start) continue;
        edit[f.key] = f.num ? parseNum(f, v) : (v.trim() || null);
      }
      if (cover === 'remove') edit.cover = 'remove';
      else if (cover) edit.cover = { file: cover.file };
      const text = area.value, tgt = target.value;
      const lyricsChange = single && lyricsDirty && text !== embedded;
      if (lyricsChange && tgt !== 'file') edit.lyrics = text.trim() ? text : null;
      return { edit, fileLyrics: single && lyricsDirty && tgt !== 'tags' && text.trim() ? text : null };
    }
    const dirty = () => touched.size > 0 || !!cover || (single && lyricsDirty);
    isDirty = dirty;
    const saveBtn = button('Save', () => save(), { cls: 'primary' });
    const touchedAny = () => { saveBtn.disabled = saving || readOnly || !dirty(); };

    async function save() {
      let plan;
      try { plan = collect(); } catch (e) {
        if (e.key) { show('details'); const el = inputs.get(e.key); el?.setAttribute('aria-invalid', 'true'); el?.focus(); }
        toast(e.message, 'error'); return;
      }
      const { edit, fileLyrics } = plan;
      if (!Object.keys(edit).length && fileLyrics == null) { done(false); return; }
      saving = true; saveBtn.disabled = true; cancelBtn.disabled = true;
      const failures = [];
      let saved = 0;
      for (const [i, p] of paths.entries()) {
        saveBtn.textContent = paths.length > 1 ? `Saving ${i + 1} of ${paths.length}…` : 'Saving…';
        try {
          if (Object.keys(edit).length) await api.writeTags(p, edit);
          if (fileLyrics != null) await api.saveLyricsFile(p, fileLyrics, stampCount(fileLyrics).stamped > 0);
          saved++;
        } catch (e) { failures.push([p, String(e?.message ?? e)]); }
      }
      saving = false;
      if (failures.length) {
        toast(paths.length === 1 ? failures[0][1] : `${failures.length} of ${paths.length} songs couldn't be saved: ${failures[0][1]}`, 'error');
        if (!saved) { saveBtn.textContent = 'Save'; cancelBtn.disabled = false; touchedAny(); return; }
      } else toast(single ? `Saved ${forms[0].title || baseName(paths[0])}` : `Saved ${plural(saved, 'song')}`);
      if (cover && cover !== 'remove') URL.revokeObjectURL(cover.url);
      done(true);
    }

    const cancelBtn = button('Cancel', () => cancel());
    box.classList.add('tg-dialog');
    box.append(...[
      h('h3', { class: 'dialog-title' }, single ? 'Edit tags' : `Edit ${paths.length} songs`),
      h('p', { class: 'hint tg-file', title: paths[0] }, single ? baseName(paths[0]) : 'Only the fields you change are written to each song.'),
      readOnly && h('p', { class: 'hint warn' }, single ? 'This file is read-only, so its tags can’t be changed.' : 'Some of these files are read-only, so the tags can’t be saved.'),
      h('div', { class: 'lib-tabs tg-tabs', role: 'tablist' }, tabBtns),
      details, artPane, single && lyricsPane,
      h('div', { class: 'dialog-actions' }, cancelBtn, saveBtn)].filter(Boolean));
    setCover(null);
    show(active);
    touchedAny();
    setTimeout(() => (active === 'details' ? inputs.get('title') ?? inputs.get('artist') : null)?.focus());
    // Ctrl+Enter saves from anywhere in the form.
    box.addEventListener('keydown', (e) => { if ((e.ctrlKey || e.metaKey) && e.key === 'Enter' && !saveBtn.disabled) { e.preventDefault(); save(); } });
    box.addEventListener('keydown', (e) => { if (e.key === 'Enter' && e.target instanceof HTMLInputElement && !saveBtn.disabled) { e.preventDefault(); save(); } });
  }, { guard }).then((v) => v === true);
}
