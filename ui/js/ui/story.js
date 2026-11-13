// "Play story": the period's numbers as a short slideshow. Tap the right or left side (or use the
// arrow keys) to move, hold to pause. It moves on by itself unless the system asks for less motion.

import { COVER, coverUrl } from '../api.js';
import { h, plural } from '../util.js';
import { icon } from './icons.js';
import { iconButton, button } from './controls.js';
import { modal } from './dialog.js';

const SLIDE_MS = 6500;

/**
 * `data`: { title, period, secs, deltaText, deltaDir, fmt(secs), report, bestDay: { title, secs } | null,
 * part: { nick, name, share } | null, taste: string | null, onPlayMix?: () => void }
 */
export function playStory(data) {
  const r = data.report;
  const slides = [];
  slides.push({ kind: 'intro', build: () => [
    h('p', { class: 'sty-eyebrow' }, data.eyebrow),
    h('h2', { class: 'sty-title' }, data.title),
    h('p', { class: 'sty-num' }, data.fmt(data.secs)),
    data.deltaText ? h('p', { class: `sty-delta is-${data.deltaDir}` }, `${data.deltaDir === 'up' ? '▲ ' : data.deltaDir === 'down' ? '▼ ' : ''}${data.deltaText} ${data.vs}`) : null,
    h('p', { class: 'sty-sub' }, [plural(r.plays, 'play'), plural(r.tracks_heard, 'song'), plural(r.artists_heard, 'artist')].join(' · '))] });
  const [song] = r.top_tracks, [artist] = r.top_artists, [album] = r.top_albums;
  if (song) slides.push({ kind: 'song', build: () => [h('p', { class: 'sty-eyebrow' }, 'Your most played song'), art(song.path), h('h2', { class: 'sty-name' }, song.title), h('p', { class: 'sty-sub' }, `${song.artist} · ${song.plays ? plural(song.plays, 'play') : data.fmt(song.secs)}`)] });
  if (artist) slides.push({ kind: 'artist', build: () => [h('p', { class: 'sty-eyebrow' }, 'Your top artist'), art(artist.path, true), h('h2', { class: 'sty-name' }, artist.name), h('p', { class: 'sty-sub' }, `${data.fmt(artist.secs)} across ${plural(artist.tracks, 'song')}`)] });
  if (album) slides.push({ kind: 'album', build: () => [h('p', { class: 'sty-eyebrow' }, 'Your top album'), art(album.path), h('h2', { class: 'sty-name' }, album.album), h('p', { class: 'sty-sub' }, `${album.artist} · ${data.fmt(album.secs)}`)] });
  if (data.taste) slides.push({ kind: 'taste', build: () => [h('p', { class: 'sty-eyebrow' }, 'Your sound'), h('h2', { class: 'sty-title' }, data.taste),
    h('ul', { class: 'sty-genres' }, r.genres.filter((g) => g.name !== 'Unknown').slice(0, 3).map((g, i) => h('li', null, h('b', null, g.name), h('small', null, `${Math.round((g.secs / r.genres.filter((x) => x.name !== 'Unknown').reduce((a, x) => a + x.secs, 0)) * 100)}%`))))] });
  if (data.part || data.bestDay) slides.push({ kind: 'when', build: () => [h('p', { class: 'sty-eyebrow' }, 'When you listen'),
    data.part ? h('h2', { class: 'sty-title' }, data.part.nick) : null,
    data.part ? h('p', { class: 'sty-sub' }, `${Math.round(data.part.share * 100)}% of it in the ${data.part.name}`) : null,
    data.bestDay ? h('p', { class: 'sty-sub sty-best' }, `Your biggest ${data.bestLabel}: ${data.bestDay.title}, ${data.fmt(data.bestDay.secs)}`) : null] });
  slides.push({ kind: 'end', build: (close) => [h('p', { class: 'sty-eyebrow' }, 'That was'), h('h2', { class: 'sty-title' }, data.title),
    h('div', { class: 'sty-actions' }, data.onPlayMix ? button('Play my mix', () => { close(); data.onPlayMix(); }, { cls: 'primary', iconName: 'play' }) : null, button('Done', () => close()))] });

  function art(path, round = false) {
    const img = h('img', { class: 'cover-img', alt: '', draggable: 'false' });
    const el = h('div', { class: `sty-art${round ? ' is-round' : ''}` }, h('div', { class: 'cover-ph' }, icon('note', 40)), img);
    img.onload = () => el.classList.add('has-cover');
    img.src = coverUrl(path, COVER.card);
    return el;
  }

  const calm = matchMedia('(prefers-reduced-motion: reduce)').matches;
  return modal((box, done) => {
    let i = 0, timer = 0, started = 0, left = SLIDE_MS, held = false, paused = calm;
    box.classList.add('story');
    const bars = slides.map(() => h('span', { class: 'sty-seg' }, h('i')));
    const stage = h('div', { class: 'sty-stage', 'aria-live': 'polite' });
    const pauseBtn = iconButton('pause', 'Pause', () => { paused = !paused; syncTimer(); paintPause(); }, { size: 18, cls: 'sty-pause' });
    const cleanup = () => { clearTimeout(timer); document.removeEventListener('keydown', onKey, true); };
    document.addEventListener('story-closing', cleanup, { once: true });
    const close = () => { cleanup(); done(null); };
    const top = h('div', { class: 'sty-top' }, h('div', { class: 'sty-bars', role: 'presentation' }, bars), h('div', { class: 'sty-tools' }, pauseBtn, iconButton('close', 'Close story', close, { size: 18 })));
    const prev = h('button', { class: 'sty-hit sty-prev', type: 'button', 'aria-label': 'Previous', onclick: () => go(i - 1) });
    const next = h('button', { class: 'sty-hit sty-next', type: 'button', 'aria-label': 'Next', onclick: () => go(i + 1) });
    box.append(stage, prev, next, top);

    function paintPause() { pauseBtn.replaceChildren(icon(paused ? 'play' : 'pause', 18)); pauseBtn.title = pauseBtn.ariaLabel = paused ? 'Play' : 'Pause'; }
    function show() {
      stage.dataset.kind = slides[i].kind;
      stage.replaceChildren(...slides[i].build(close).filter(Boolean));
      stage.classList.remove('in'); void stage.offsetWidth; stage.classList.add('in');
      bars.forEach((b, n) => { b.classList.toggle('is-done', n < i); b.classList.toggle('is-now', n === i); const fill = b.firstChild; fill.style.transition = 'none'; fill.style.transform = `scaleX(${n < i ? 1 : 0})`; });
      prev.hidden = i === 0; next.hidden = i === slides.length - 1;
      left = SLIDE_MS; syncTimer();
    }
    function syncTimer() {
      clearTimeout(timer);
      const fill = bars[i].firstChild, last = i === slides.length - 1;
      if (paused || held || last) { const done = last ? 1 : 1 - left / SLIDE_MS; fill.style.transition = 'none'; fill.style.transform = `scaleX(${last ? 1 : done})`; return; }
      started = performance.now();
      void fill.offsetWidth;
      fill.style.transition = `transform ${left}ms linear`; fill.style.transform = 'scaleX(1)';
      timer = setTimeout(() => go(i + 1), left);
    }
    function freeze() { if (!paused && !held) left = Math.max(0, left - (performance.now() - started)); }
    function go(n) { if (n < 0 || n >= slides.length) return; i = n; show(); }
    box.addEventListener('pointerdown', (e) => { if (e.target.closest('button, a')?.classList.contains('sty-hit') || !e.target.closest('button')) { freeze(); held = true; clearTimeout(timer); const fill = bars[i].firstChild; fill.style.transition = 'none'; fill.style.transform = `scaleX(${1 - left / SLIDE_MS})`; } });
    const release = () => { if (held) { held = false; syncTimer(); } };
    box.addEventListener('pointerup', release); box.addEventListener('pointercancel', release); box.addEventListener('pointerleave', release);
    function onKey(e) {
      if (e.key === 'ArrowRight' || e.key === 'ArrowDown') { e.preventDefault(); e.stopPropagation(); go(i + 1); }
      else if (e.key === 'ArrowLeft' || e.key === 'ArrowUp') { e.preventDefault(); e.stopPropagation(); go(i - 1); }
      else if (e.key === ' ' && !e.target.closest('button')) { e.preventDefault(); e.stopPropagation(); freeze(); paused = !paused; syncTimer(); paintPause(); }
    }
    document.addEventListener('keydown', onKey, true);
    paintPause(); show();
  }, { guard: () => { /* Escape or a click outside */ document.dispatchEvent(new CustomEvent('story-closing')); return true; } });
}
