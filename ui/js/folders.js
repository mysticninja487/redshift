// Browsing the library by folder. Works on the tracks' own paths (no file system access), so it
// shows exactly what's in the library. Paths are compared with `/` as the separator.

export const norm = (p) => p.replace(/\\/g, '/').replace(/\/+$/, '');
export const baseName = (p) => norm(p).split('/').pop() || p;

const byName = (a, b) => a.localeCompare(b, undefined, { numeric: true, sensitivity: 'base' });

/** The folders directly inside `dir` (each with every track beneath it) and the tracks directly in it. */
export function folderContents(tracks, dir) {
  const prefix = `${norm(dir)}/`;
  const dirs = new Map();
  const files = [];
  for (const t of tracks) {
    const p = norm(t.path);
    if (!p.startsWith(prefix)) continue;
    const rest = p.slice(prefix.length);
    const i = rest.indexOf('/');
    if (i < 0) { files.push(t); continue; }
    const name = rest.slice(0, i);
    let d = dirs.get(name);
    if (!d) dirs.set(name, (d = { name, path: prefix + name, tracks: [] }));
    d.tracks.push(t);
  }
  return {
    dirs: [...dirs.values()].sort((a, b) => byName(a.name, b.name)),
    files: files.sort((a, b) => byName(baseName(a.path), baseName(b.path))),
  };
}

/** Every track at any depth below `dir`, in folder then file-name order. */
export function tracksUnder(tracks, dir) {
  const prefix = `${norm(dir)}/`;
  return tracks.filter((t) => norm(t.path).startsWith(prefix)).sort((a, b) => byName(norm(a.path), norm(b.path)));
}

/** Breadcrumb parts from `root` down to `dir`: [{ name, path }]. */
export function crumbs(root, dir) {
  const r = norm(root), d = norm(dir);
  const out = [{ name: baseName(r) || r, path: r }];
  if (d.length > r.length) {
    let cur = r;
    for (const part of d.slice(r.length + 1).split('/')) { cur += `/${part}`; out.push({ name: part, path: cur }); }
  }
  return out;
}
