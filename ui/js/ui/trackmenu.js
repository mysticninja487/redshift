// The "what can I do with these tracks" menu, shared by the queue, library and playlists.

import { api } from '../api.js';
import { act, playPaths, queueAdd, queueNext, toggleFavourite } from '../controller.js';
import { isServerPath, state } from '../state.js';
import { emit } from '../bus.js';
import { toast } from '../util.js';
import { confirmDialog, promptText } from './dialog.js';

/** Menu items acting on `paths`. `extra` items are added at the top. */
export async function trackMenuItems(paths, extra = []) {
  let lists = [], held = new Map(); // held: playlist id -> how many of these songs it already has
  try {
    lists = await api.playlists();
    if (paths.length && lists.length) held = new Map((await api.playlistMembership(paths)).map((m) => [m.id, m.contains]));
  } catch { /* the menu still works without playlists */ }
  const addTo = (id, name) => async () => {
    const have = held.get(id) ?? 0;
    if (have) {
      const text = paths.length === 1 ? `This song is already in \u201c${name}\u201d.` : `${have} of these ${paths.length} songs are already in \u201c${name}\u201d.`;
      if (!await confirmDialog('Already in this playlist', text, { ok: 'Add anyway', danger: false })) return;
    }
    await act(api.addToPlaylist(id, paths)); emit('playlists'); toast(`Added to ${name}`);
  };
  const removeFrom = (id, name) => async () => { await act(api.removeSongsFromPlaylist(id, paths)); emit('playlists'); toast(`Removed from ${name}`); };
  const inLists = lists.filter((p) => held.get(p.id));
  // The playlist screen has its own Remove from playlist (by position), so don't offer a second one there.
  const hasOwnRemove = extra.some((x) => x.label === 'Remove from playlist');
  // Songs on the person's server have no file here to edit tags in or save lyrics beside.
  const local = !paths.some(isServerPath);
  return [
    ...extra,
    { label: 'Play now', onClick: () => playPaths(paths, 0) },
    { label: 'Play next', onClick: () => queueNext(paths) },
    { label: 'Add to queue', onClick: () => queueAdd(paths) },
    { sep: true },
    ...(paths.length === 1 ? [{ label: 'Song info…', onClick: async () => { const { openSongInfo } = await import('./songinfo.js'); openSongInfo(paths[0]); } }] : []),
    ...(local ? [
      { label: paths.length > 1 ? `Edit tags (${paths.length} songs)…` : 'Edit tags…', disabled: !paths.length, onClick: async () => { const { openTagEditor } = await import('./tageditor.js'); openTagEditor(paths); } },
      { label: paths.length > 1 ? `Find lyrics online (${paths.length} songs)…` : 'Find lyrics online…', disabled: !paths.length, onClick: async () => { const { findLyricsFor } = await import('./lyricsbatch.js'); findLyricsFor(paths); } },
    ] : []),
    { label: paths.every((p) => state.favs.has(p)) ? 'Remove from favourites' : 'Add to favourites', onClick: () => toggleFavourite(paths) },
    {
      label: 'Add to playlist',
      submenu: [
        { label: 'New playlist…', onClick: async () => { const name = await promptText('New playlist', { ok: 'Create', placeholder: 'Playlist name' }); if (name) { await act(api.createPlaylist(name, paths)); emit('playlists'); toast(`Created ${name}`); } } },
        ...(lists.length ? [{ sep: true }] : []),
        ...lists.map((p) => ({ label: p.name, onClick: addTo(p.id, p.name) })),
      ],
    },
    ...(inLists.length && !hasOwnRemove ? [{ label: 'Remove from playlist', submenu: inLists.map((p) => ({ label: p.name, onClick: removeFrom(p.id, p.name) })) }] : []),
  ];
}
