// Stats: how much you've listened today, this week, this month and this year, what you played most,
// what your taste looks like, and mixes made from it. Everything is counted on this computer.

import { api, COVER, coverUrl } from '../api.js';
import { on } from '../bus.js';
import { act, playPaths, queueAdd, setView } from '../controller.js';
import { state } from '../state.js';
import { h, plural, toast } from '../util.js';
import { button, iconButton } from '../ui/controls.js';
import { promptText } from '../ui/dialog.js';
import { icon } from '../ui/icons.js';

// ---------- time ----------

const PERIODS = [
  { id: 'day', label: 'Today', prev: 'yesterday' },
  { id: 'week', label: 'This week', prev: 'last week' },
  { id: 'month', label: 'This month', prev: 'last month' },
  { id: 'year', label: 'This year', prev: 'last year' },
];
const WEEKDAYS = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'];
const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];

const secs = (d) => Math.floor(d.getTime() / 1000);

/** Where a period starts and ends, and the one before it, in the person's own calendar. */
export function periodRange(id, now = new Date()) {
  const y = now.getFullYear(), m = now.getMonth(), d = now.getDate();
  const day = (date) => (date.getDay() + 6) % 7; // Monday = 0
  let start, next, prevStart;
  if (id === 'day') { start = new Date(y, m, d); next = new Date(y, m, d + 1); prevStart = new Date(y, m, d - 1); }
  else if (id === 'week') { start = new Date(y, m, d - day(now)); next = new Date(y, m, d - day(now) + 7); prevStart = new Date(y, m, d - day(now) - 7); }
  else if (id === 'month') { start = new Date(y, m, 1); next = new Date(y, m + 1, 1); prevStart = new Date(y, m - 1, 1); }
  else { start = new Date(y, 0, 1); next = new Date(y + 1, 0, 1); prevStart = new Date(y - 1, 0, 1); }
  const from = secs(start), end = secs(next), t = secs(now);
  const prevFrom = secs(prevStart);
  // The earlier period is measured over the same stretch, so "this week so far" meets "last week by the same point".
  const elapsed = Math.max(0, t - from);
  return { id, from, to: t + 1, end, prevFrom, prevTo: Math.min(from, prevFrom + elapsed + 1), start, next };
}

/** The bars of a period's chart: `{ from, to, label, title, current, future }`. */
export function chartBuckets(id, now = new Date()) {
  const r = periodRange(id, now), t = secs(now), out = [];
  const bar = (a, b, label, title) => out.push({ from: secs(a), to: secs(b), label, title, current: t >= secs(a) && t < secs(b), future: secs(a) > t });
  const { start } = r, y = start.getFullYear(), m = start.getMonth(), d = start.getDate();
  if (id === 'day') for (let i = 0; i < 24; i++) bar(new Date(y, m, d, i), new Date(y, m, d, i + 1), i % 6 === 0 ? String(i) : '', `${String(i).padStart(2, '0')}:00`);
  else if (id === 'week') for (let i = 0; i < 7; i++) bar(new Date(y, m, d + i), new Date(y, m, d + i + 1), WEEKDAYS[i][0], `${WEEKDAYS[i]} ${new Date(y, m, d + i).getDate()} ${MONTHS[new Date(y, m, d + i).getMonth()]}`);
  else if (id === 'month') { const n = new Date(y, m + 1, 0).getDate(); for (let i = 1; i <= n; i++) bar(new Date(y, m, i), new Date(y, m, i + 1), i === 1 || i % 5 === 0 ? String(i) : '', `${i} ${MONTHS[m]}`); }
  else for (let i = 0; i < 12; i++) bar(new Date(y, i, 1), new Date(y, i + 1, 1), MONTHS[i][0], `${MONTHS[i]} ${y}`);
  return out;
}

export function fmtDuration(s) {
  s = Math.max(0, Math.round(s));
  const hrs = Math.floor(s / 3600), min = Math.floor((s % 3600) / 60);
  if (hrs) return `${hrs}h ${String(min).padStart(2, '0')}m`;
  if (min) return `${min}m`;
  return s ? `${s}s` : '0m';
}
/** "6h 42m" as parts so the units can be set smaller. */
function durationParts(s) {
  s = Math.max(0, Math.round(s));
  const hrs = Math.floor(s / 3600), min = Math.floor((s % 3600) / 60);
  if (hrs) return [[String(hrs), 'h'], [String(min).padStart(2, '0'), 'm']];
  if (min) return [[String(min), 'm']];
  return [[String(s), 's']];
}
const timeEl = (s, cls = '') => h('span', { class: `st-time ${cls}`, 'aria-label': fmtDuration(s) }, durationParts(s).flatMap(([n, u]) => [n, h('small', null, u)]));

/** How this compares with the earlier stretch: `{ dir, text }`. */
export function delta(cur, prev) {
  if (!prev && !cur) return null;
  if (!prev) return { dir: 'new', text: 'New' };
  const pct = Math.round(((cur - prev) / prev) * 100);
  if (pct === 0) return { dir: 'same', text: 'Same' };
  return { dir: pct > 0 ? 'up' : 'down', text: `${Math.abs(pct)}%` };
}
const deltaEl = (dl, vs) => (dl ? h('span', { class: `st-delta is-${dl.dir}`, title: vs ? `Compared with ${vs} by this time` : '' }, dl.dir === 'up' ? '▲ ' : dl.dir === 'down' ? '▼ ' : '', dl.text) : null);

// ---------- taste ----------

const PARTS = [
  { name: 'mornings', from: 5, to: 12, nick: 'Early bird' },
  { name: 'afternoons', from: 12, to: 17, nick: 'Daytime listener' },
  { name: 'evenings', from: 17, to: 22, nick: 'Evening listener' },
  { name: 'late nights', from: 22, to: 29, nick: 'Night owl' }, // 22:00 to 05:00
];
export function partOfDay(hours) {
  const total = hours.reduce((a, b) => a + b, 0);
  if (!total) return null;
  const sums = PARTS.map((p) => { let s = 0; for (let x = p.from; x < p.to; x++) s += hours[x % 24]; return s; });
  const i = sums.indexOf(Math.max(...sums));
  return { ...PARTS[i], share: sums[i] / total };
}
export function tasteLine(genres) {
  const real = genres.filter((g) => g.name !== 'Unknown');
  const total = real.reduce((a, g) => a + g.secs, 0);
  if (!total) return null;
  const [a, b, c] = real;
  if (a.secs / total >= 0.55) return `Mostly ${a.name}`;
  if (!b) return a.name;
  if (!c || c.secs / total < 0.12) return `${a.name} and ${b.name}`;
  return `${a.name}, ${b.name} and ${c.name}`;
}

// ---------- the view ----------

export function mountStats(root) {
  let period = 'week';
  let token = 0, timer = 0, loaded = false;

  const back = iconButton('chevronLeft', 'Back to now playing', () => setView('now'), { cls: 'v-back' });
  const head = h('header', { class: 'v-head' }, back, h('h1', { class: 'v-title' }, 'Stats'));
  const body = h('div', { class: 'v-body st-body' });
  root.append(head, body);

  const tiles = PERIODS.map((p) => {
    const el = h('button', { class: 'st-tile', type: 'button', role: 'tab', dataset: { period: p.id }, onclick: () => { period = p.id; paintTabs(); load(); } },
      h('span', { class: 'st-tile-name' }, p.label), h('span', { class: 'st-tile-val' }), h('span', { class: 'st-tile-delta' }));
    return { id: p.id, el, val: el.querySelector('.st-tile-val'), dl: el.querySelector('.st-tile-delta') };
  });
  const tabs = h('div', { class: 'st-tiles', role: 'tablist', 'aria-label': 'Period' }, tiles.map((t) => t.el));
  const detail = h('div', { class: 'st-detail', role: 'tabpanel' });
  const mixBox = h('section', { class: 'st-sec st-mixes', 'aria-labelledby': 'st-mix-h' });
  const foot = h('p', { class: 'hint st-foot' });
  body.append(tabs, detail, mixBox, foot);

  function paintTabs() {
    for (const t of tiles) { t.el.setAttribute('aria-selected', String(t.id === period)); t.el.tabIndex = t.id === period ? 0 : -1; }
  }
  tabs.addEventListener('keydown', (e) => {
    const i = PERIODS.findIndex((p) => p.id === period);
    const n = e.key === 'ArrowRight' ? i + 1 : e.key === 'ArrowLeft' ? i - 1 : null;
    if (n == null) return;
    e.preventDefault();
    period = PERIODS[(n + PERIODS.length) % PERIODS.length].id; paintTabs(); load();
    tiles.find((t) => t.id === period).el.focus();
  });

  const recording = () => state.settings?.record_history !== false;

  async function load() {
    const mine = ++token;
    const now = new Date();
    const tz = -now.getTimezoneOffset();
    const ranges = PERIODS.map((p) => periodRange(p.id, now));
    const cur = periodRange(period, now), buckets = chartBuckets(period, now);
    try {
      const [totals, report, bars, mixes] = await Promise.all([
        api.listeningTotals(ranges.flatMap((r) => [[r.from, r.to], [r.prevFrom, r.prevTo]])),
        api.listeningReport(cur.from, cur.to, tz),
        api.listeningTotals(buckets.map((b) => [b.from, b.to])),
        api.mixes(),
      ]);
      if (mine !== token) return;
      loaded = true;
      PERIODS.forEach((p, i) => {
        const t = tiles[i], c = totals[i * 2], pv = totals[i * 2 + 1];
        t.val.replaceChildren(timeEl(c.secs));
        t.dl.replaceChildren(...[deltaEl(delta(c.secs, pv.secs), p.prev)].filter(Boolean));
      });
      const everything = totals.some((t) => t.secs > 0);
      paintDetail(report, bars, buckets, totals[PERIODS.findIndex((p) => p.id === period) * 2 + 1], everything);
      paintMixes(mixes);
      foot.textContent = recording()
        ? 'Counted on this computer only. Nothing is sent anywhere. You can turn the history off or clear it in Settings.'
        : 'Listening history is off, so nothing new is being counted. Turn it on in Settings.';
    } catch (e) {
      if (mine !== token) return;
      console.error(e);
      detail.replaceChildren(h('div', { class: 'empty st-empty' }, icon('record', 36), h('p', null, 'Could not load your stats.')));
    }
  }

  // ---- the chosen period ----

  function paintDetail(r, bars, buckets, prev, everything) {
    const p = PERIODS.find((x) => x.id === period);
    if (!r.secs && !r.plays) {
      detail.replaceChildren(h('div', { class: 'empty st-empty' }, icon('record', 36),
        h('p', null, everything ? `Nothing ${period === 'day' ? 'yet today' : `${p.label.toLowerCase()} so far`}.` : recording() ? 'Nothing to show yet. Play some music and this fills in.' : 'Listening history is off, so there is nothing to show.')));
      return;
    }
    const dl = delta(r.secs, prev.secs);
    const hero = h('section', { class: 'st-sec st-hero', 'aria-label': `${p.label} in numbers` },
      h('div', { class: 'st-hero-main' },
        h('p', { class: 'st-eyebrow' }, period === 'day' ? 'Listened today' : `Listened ${p.label.toLowerCase()}`),
        h('div', { class: 'st-big' }, timeEl(r.secs, 'is-big'), deltaEl(dl, p.prev)),
        h('p', { class: 'st-facts' }, [
          plural(r.plays, 'play'), plural(r.tracks_heard, 'song'), plural(r.artists_heard, 'artist'),
          r.new_tracks ? `${r.new_tracks} new to you` : null,
        ].filter(Boolean).join(' · '))),
      chart(bars, buckets));
    detail.replaceChildren(hero, tops(r), taste(r));
  }

  function chart(bars, buckets) {
    const max = Math.max(1, ...bars.map((b) => b.secs));
    const best = bars.reduce((m, b, i) => (b.secs > bars[m].secs ? i : m), 0);
    const cols = buckets.map((b, i) => h('div', { class: `st-col${b.current ? ' is-now' : ''}${b.future ? ' is-future' : ''}${bars[i].secs ? '' : ' is-zero'}`, title: `${b.title} · ${bars[i].secs ? fmtDuration(bars[i].secs) : 'nothing'}` },
      h('i', { style: { height: `${Math.max(bars[i].secs ? 4 : 0, (bars[i].secs / max) * 100)}%` } }), h('span', null, b.label)));
    const wrap = h('div', { class: 'st-chart', role: 'img', 'aria-label': bars[best].secs ? `Listening time by ${period === 'day' ? 'hour' : period === 'year' ? 'month' : 'day'}. Most: ${buckets[best].title}, ${fmtDuration(bars[best].secs)}.` : 'No listening in this period.', dataset: { n: String(cols.length) } }, cols);
    return wrap;
  }

  function cover(path, cls = 'st-cover') {
    const img = h('img', { class: 'cover-img', alt: '', loading: 'lazy', draggable: 'false' });
    const el = h('div', { class: cls }, h('div', { class: 'cover-ph' }, icon('note', 26)), img);
    img.onload = () => el.classList.add('has-cover');
    img.src = coverUrl(path, COVER.card);
    return el;
  }

  function tops(r) {
    const [song] = r.top_tracks, [artist] = r.top_artists, [album] = r.top_albums;
    const card = (kind, title, sub, note, path, onclick) => h(onclick ? 'button' : 'div', { class: 'st-top', type: onclick ? 'button' : undefined, onclick, title: onclick ? 'Play it' : undefined },
      cover(path), h('div', { class: 'st-top-text' }, h('span', { class: 'st-kind' }, kind), h('b', { class: 'st-top-title' }, title), h('span', { class: 'st-top-sub' }, sub), h('span', { class: 'st-top-note' }, note)));
    const list = (title, items) => h('div', { class: 'st-list' }, h('h3', { class: 'st-h3' }, title), items.length ? h('ol', null, items) : h('p', { class: 'hint' }, 'Nothing yet.'));
    const li = (n, name, sub, note, onclick) => h('li', null, h(onclick ? 'button' : 'div', { class: 'st-li', type: onclick ? 'button' : undefined, onclick },
      h('span', { class: 'st-n' }, String(n)), h('span', { class: 'st-li-text' }, h('b', null, name), sub ? h('small', null, sub) : null), h('span', { class: 'st-li-note' }, note)));
    const playsNote = (x) => (x.plays ? plural(x.plays, 'play') : fmtDuration(x.secs));
    return h('section', { class: 'st-sec st-tops', 'aria-labelledby': 'st-tops-h' },
      h('h2', { class: 'st-h2', id: 'st-tops-h' }, 'Most played'),
      h('div', { class: 'st-top-row' },
        song ? card('Song', song.title, song.artist, `${playsNote(song)} · ${fmtDuration(song.secs)}`, song.path, () => playPaths([song.path], 0)) : null,
        artist ? card('Artist', artist.name, plural(artist.tracks, 'song'), `${fmtDuration(artist.secs)} listened`, artist.path) : null,
        album ? card('Album', album.album, album.artist, `${fmtDuration(album.secs)} listened`, album.path) : null),
      h('div', { class: 'st-lists' },
        list('Songs', r.top_tracks.slice(0, 5).map((t, i) => li(i + 1, t.title, t.artist, playsNote(t), () => playPaths([t.path], 0)))),
        list('Artists', r.top_artists.slice(0, 5).map((a, i) => li(i + 1, a.name, plural(a.tracks, 'song'), fmtDuration(a.secs)))),
        list('Albums', r.top_albums.slice(0, 5).map((a, i) => li(i + 1, a.album, a.artist, fmtDuration(a.secs))))));
  }

  function taste(r) {
    const real = r.genres.filter((g) => g.name !== 'Unknown');
    const total = real.reduce((a, g) => a + g.secs, 0);
    const line = tasteLine(r.genres), part = partOfDay(r.hours);
    const top = r.decades.reduce((m, d) => (!m || d.secs > m.secs ? d : m), null);
    const decadeTotal = r.decades.reduce((a, d) => a + d.secs, 0);
    const bars = real.slice(0, 6).map((g) => h('li', { class: 'st-genre' },
      h('span', { class: 'st-genre-name' }, g.name),
      h('span', { class: 'st-genre-bar', role: 'presentation' }, h('i', { style: { width: `${Math.max(3, (g.secs / total) * 100)}%` } })),
      h('span', { class: 'st-genre-pct' }, `${Math.round((g.secs / total) * 100)}%`)));
    const facts = [];
    if (part) facts.push(['Time of day', part.nick, `${Math.round(part.share * 100)}% of your listening is in the ${part.name}`]);
    if (r.tracks_heard >= 3) facts.push(['Discovery', `${Math.round((r.new_tracks / r.tracks_heard) * 100)}% new`, `${r.new_tracks} of the ${r.tracks_heard} songs were new to you`]);
    if (r.plays && r.tracks_heard) facts.push(['Replays', `${(r.plays / r.tracks_heard).toFixed(1)}×`, 'plays per song']);
    if (top && decadeTotal) facts.push(['Era', `${top.decade}s`, `${Math.round((top.secs / decadeTotal) * 100)}% of the music you played is from then`]);
    return h('section', { class: 'st-sec st-taste', 'aria-labelledby': 'st-taste-h' },
      h('h2', { class: 'st-h2', id: 'st-taste-h' }, 'Your taste'),
      line ? h('p', { class: 'st-taste-line' }, line) : null,
      h('div', { class: 'st-taste-grid' },
        bars.length ? h('ul', { class: 'st-genres', 'aria-label': 'Genres' }, bars) : h('p', { class: 'hint' }, 'Your songs have no genre tags, so there is nothing to break down. The tag editor can add them.'),
        facts.length ? h('dl', { class: 'st-factlist' }, facts.map(([k, v, s]) => h('div', { class: 'st-fact' }, h('dt', null, k), h('dd', null, h('b', null, v), h('span', null, s))))) : null));
  }

  // ---- mixes ----

  function paintMixes(mixes) {
    mixBox.replaceChildren(h('h2', { class: 'st-h2', id: 'st-mix-h' }, 'Mixes for you'));
    if (!mixes.length) {
      mixBox.append(h('p', { class: 'hint' }, 'Mixes appear once Redshift has seen what you play. They change every day.'));
      return;
    }
    mixBox.append(h('p', { class: 'hint' }, 'Made from what you play, on this computer. They change every day.'),
      h('div', { class: 'st-mix-row' }, mixes.map(mixCard)));
  }

  const HUES = { repeat: 12, rediscover: 270, genre: 200, artist: 150, fresh: 45, favourites: 340 };
  function mixCard(m) {
    const name = h('h3', { class: 'st-mix-name' }, m.name);
    const tracks = m.tracks;
    const art = h('div', { class: 'st-mix-art', 'aria-hidden': 'true' }, tracks.slice(0, 3).map((p, i) => cover(p, `st-mix-cover c${i}`)));
    return h('article', { class: 'st-mix', dataset: { kind: m.kind, mix: m.id }, style: { '--h': String(HUES[m.kind] ?? 200) } },
      art, h('div', { class: 'st-mix-text' }, name, h('p', null, m.blurb), h('span', { class: 'st-mix-count' }, plural(tracks.length, 'song'))),
      h('div', { class: 'st-mix-actions' },
        button('Play', () => playPaths(tracks, 0), { cls: 'primary', iconName: 'play' }),
        button('Add to queue', () => queueAdd(tracks), { cls: 'ghost' }),
        button('Save as playlist', async () => {
          const n = await promptText('Save as playlist', { value: `${m.name} · ${new Date().toLocaleDateString(undefined, { day: 'numeric', month: 'short' })}`, ok: 'Save playlist' });
          if (n === null) return;
          await act(api.createPlaylist(n, tracks)); toast(`Saved “${n}”`);
        }, { cls: 'ghost' })));
  }

  // ---- when to refresh ----

  paintTabs();
  const visible = () => state.view === 'stats';
  const tickOn = () => { clearInterval(timer); timer = setInterval(() => { if (visible() && !document.hidden) load(); }, 20000); };
  on('view', (v) => { if (v === 'stats') { load(); tickOn(); } else clearInterval(timer); });
  if (visible()) { load(); tickOn(); }
  return { reload: () => load(), get loaded() { return loaded; } };
}
