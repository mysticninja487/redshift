// The "what can I do with these tracks" menu, shared by the queue, library and playlists.

import { api } from '../api.js';
import { act, playPaths, queueAdd, queueNext, toggleFavourite } from '../controller.js';
import { state } from '../state.js';
import { emit } from '../bus.js';
import { toast } from '../util.js';
import { promptText } from './dialog.js';

/** Menu items acting on `paths`. `extra` items are added at the top. */
export async function trackMenuItems(paths, extra = []) {
  let lists = [];
  try { lists = await api.playlists(); } catch { /* the menu still works without playlists */ }
  const addTo = (id, name) => async () => { await act(api.addToPlaylist(id, paths)); emit('playlists'); toast(`Added to ${name}`); };
  return [
    ...extra,
    { label: 'Play now', onClick: () => playPaths(paths, 0) },
    { label: 'Play next', onClick: () => queueNext(paths) },
    { label: 'Add to queue', onClick: () => queueAdd(paths) },
    { sep: true },
    ...(paths.length === 1 ? [{ label: 'Song info…', onClick: async () => { const { openSongInfo } = await import('./songinfo.js'); openSongInfo(paths[0]); } }] : []),
    { label: paths.length > 1 ? `Edit tags (${paths.length} songs)…` : 'Edit tags…', disabled: !paths.length, onClick: async () => { const { openTagEditor } = await import('./tageditor.js'); openTagEditor(paths); } },
    { label: paths.every((p) => state.favs.has(p)) ? 'Remove from favourites' : 'Add to favourites', onClick: () => toggleFavourite(paths) },
    {
      label: 'Add to playlist',
      submenu: [
        { label: 'New playlist…', onClick: async () => { const name = await promptText('New playlist', { ok: 'Create', placeholder: 'Playlist name' }); if (name) { await act(api.createPlaylist(name, paths)); emit('playlists'); toast(`Created ${name}`); } } },
        ...(lists.length ? [{ sep: true }] : []),
        ...lists.map((p) => ({ label: p.name, onClick: addTo(p.id, p.name) })),
      ],
    },
  ];
}
