import assert from 'node:assert/strict';
import { applyFilter, sortResults, parseTime, dayStart, ago, ruleActive, newRule, emptyFilter, isLossless, isHiRes, formatsIn, detailLine, fold, playlistsNeeded, PRESETS, FIELDS, opsFor, fieldById } from '../js/filters.js';

const T = (o) => ({ title: '', artist: '', album: '', album_artist: '', genre: '', duration_secs: 200, format: 'flac', path: 'D:\\Music\\x.flac', size: 30e6, added: 0, modified: 0, ...o });
const NOW = Date.UTC(2026, 10, 27, 12) / 1000;
const day = 86400;
const tracks = [
  T({ path: 'D:\\Music\\Pink Floyd\\The Wall\\01 In the Flesh.flac', title: 'In the Flesh?', artist: 'Pink Floyd', album: 'The Wall', album_artist: 'Pink Floyd', genre: 'Rock', year: 1979, track_no: 1, disc_no: 1, duration_secs: 199, sample_rate: 44100, bit_depth: 16, channels: 2, format: 'flac', size: 25e6, added: NOW - 3 * day }),
  T({ path: 'D:\\Music\\Björk\\Debut\\Venus as a Boy.mp3', title: 'Venus as a Boy', artist: 'Björk', album: 'Debut', album_artist: 'Björk', genre: 'Electronic', year: 1993, track_no: 5, duration_secs: 281, sample_rate: 44100, channels: 2, bitrate_kbps: 320, format: 'mp3', size: 11e6, added: NOW - 100 * day, has_embedded_lyrics: true }),
  T({ path: 'D:\\Music\\Miles\\Kind of Blue\\So What.flac', title: 'So What', artist: 'Miles Davis', album: 'Kind of Blue', album_artist: 'Miles Davis', genre: 'Jazz', year: 1959, track_no: 1, duration_secs: 545, sample_rate: 96000, bit_depth: 24, channels: 2, format: 'flac', size: 90e6, added: NOW - 400 * day, replaygain_track_db: -6.5 }),
  T({ path: 'D:\\Music\\Misc\\untitled.m4a', title: 'Untitled', artist: 'Unknown Artist', album: 'Unknown Album', album_artist: 'Unknown Artist', genre: '', duration_secs: 60, sample_rate: 48000, bitrate_kbps: 256, format: 'm4a', size: 2e6, added: NOW - 1 * day }),
  T({ path: 'D:\\Music\\Misc\\alac.m4a', title: 'Lossless m4a', artist: 'X', album: 'Y', album_artist: 'X', genre: 'Rock', year: 2001, sample_rate: 44100, bit_depth: 16, format: 'm4a', size: 28e6, added: NOW - 50 * day }),
];
const stats = new Map([[tracks[0].path, { plays: 12, last_played: NOW - 2 * day }], [tracks[1].path, { plays: 1, last_played: NOW - 60 * day }]]);
const favs = new Set([tracks[2].path]);
const playlists = new Map([['p1', new Set([tracks[0].path, tracks[2].path])], ['p2', new Set([tracks[1].path])]]);
const ctx = { stats, favs, playlists, now: NOW };
const names = (f) => applyFilter(tracks, f, ctx).map((t) => t.title);
const F = (text, rules = [], match = 'all') => ({ text, match, rules: rules.map((r) => ({ ...newRule(r.field), ...r })) });

// text search: every word, any field, accents and case ignored, file name too
assert.deepEqual(names(F('pink wall')), ['In the Flesh?']);
assert.deepEqual(names(F('bjork')), ['Venus as a Boy'], 'accent-insensitive');
assert.deepEqual(names(F('jazz')), ['So What'], 'genre');
assert.deepEqual(names(F('alac')), ['Lossless m4a'], 'file name');
assert.deepEqual(names(F('')), tracks.map((t) => t.title), 'no filter, everything, in order');
assert.equal(fold('Café Ñandú'), 'cafe nandu');

// text rules
assert.deepEqual(names(F('', [{ field: 'artist', op: 'contains', a: 'miles' }])), ['So What']);
assert.deepEqual(names(F('', [{ field: 'artist', op: 'not', a: 'a' }])), ['In the Flesh?', 'Venus as a Boy', 'Lossless m4a']);
assert.deepEqual(names(F('', [{ field: 'artist', op: 'is', a: 'pink floyd' }])), ['In the Flesh?']);
assert.deepEqual(names(F('', [{ field: 'title', op: 'starts', a: 'so w' }])), ['So What']);
assert.deepEqual(names(F('', [{ field: 'title', op: 'ends', a: 'boy' }])), ['Venus as a Boy']);
assert.deepEqual(names(F('', [{ field: 'genre', op: 'empty' }])), ['Untitled']);
assert.equal(names(F('', [{ field: 'genre', op: 'filled' }])).length, 4);
assert.deepEqual(names(F('', [{ field: 'path', op: 'contains', a: 'kind of blue' }])), ['So What'], 'folder');
assert.deepEqual(names(F('', [{ field: 'path', op: 'starts', a: 'venus' }])), ['Venus as a Boy'], 'starts with: the file name');
assert.deepEqual(names(F('', [{ field: 'path', op: 'ends', a: 'alac' }])), ['Lossless m4a'], 'ends with: the file name without extension');

// numbers
assert.deepEqual(names(F('', [{ field: 'year', op: 'eq', a: '1993' }])), ['Venus as a Boy']);
assert.deepEqual(names(F('', [{ field: 'year', op: 'ge', a: '1990' }])), ['Venus as a Boy', 'Lossless m4a']);
assert.deepEqual(names(F('', [{ field: 'year', op: 'between', a: '2001', b: '1970' }])), ['In the Flesh?', 'Venus as a Boy', 'Lossless m4a'], 'the bounds may be in either order');
assert.deepEqual(names(F('', [{ field: 'year', op: 'empty' }])), ['Untitled']);
assert.deepEqual(names(F('', [{ field: 'length', op: 'ge', a: '5:00' }])), ['So What']);
assert.deepEqual(names(F('', [{ field: 'length', op: 'le', a: '1' }])), ['Untitled'], 'a bare number is minutes');
assert.deepEqual(names(F('', [{ field: 'length', op: 'eq', a: '3:19' }])), ['In the Flesh?']);
assert.deepEqual(names(F('', [{ field: 'sampleRate', op: 'eq', a: '96' }])), ['So What']);
assert.deepEqual(names(F('', [{ field: 'sampleRate', op: 'ge', a: '48' }])), ['So What', 'Untitled']);
assert.deepEqual(names(F('', [{ field: 'bitDepth', op: 'ge', a: '24' }])), ['So What']);
assert.deepEqual(names(F('', [{ field: 'bitrate', op: 'ge', a: '300' }])), ['Venus as a Boy']);
assert.deepEqual(names(F('', [{ field: 'size', op: 'ge', a: '50' }])), ['So What']);
assert.deepEqual(names(F('', [{ field: 'gain', op: 'le', a: '-6' }])), ['So What']);
assert.deepEqual(names(F('', [{ field: 'plays', op: 'ge', a: '5' }])), ['In the Flesh?']);
assert.equal(names(F('', [{ field: 'plays', op: 'eq', a: '0' }])).length, 3, 'zero plays is a real value');
assert.deepEqual(names(F('', [{ field: 'trackNo', op: 'eq', a: '1' }])), ['In the Flesh?', 'So What']);

// dates, measured from "now"
assert.deepEqual(names(F('', [{ field: 'added', op: 'last', a: '7', unit: 'days' }])), ['In the Flesh?', 'Untitled']);
assert.deepEqual(names(F('', [{ field: 'added', op: 'last', a: '2', unit: 'months' }])), ['In the Flesh?', 'Untitled', 'Lossless m4a']);
assert.deepEqual(names(F('', [{ field: 'lastPlayed', op: 'never' }])).length, 3);
assert.deepEqual(names(F('', [{ field: 'lastPlayed', op: 'last', a: '1', unit: 'weeks' }])), ['In the Flesh?']);
assert.deepEqual(names(F('', [{ field: 'lastPlayed', op: 'ever' }])), ['In the Flesh?', 'Venus as a Boy']);
const ymd = (secs) => { const d = new Date(secs * 1000); return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`; };
assert.deepEqual(names(F('', [{ field: 'added', op: 'on', a: ymd(NOW - 1 * day) }])), ['Untitled'], 'a whole day');
assert.deepEqual(names(F('', [{ field: 'added', op: 'before', a: ymd(NOW - 300 * day) }])), ['So What']);
assert.deepEqual(names(F('', [{ field: 'added', op: 'after', a: ymd(NOW - 60 * day) }])), ['In the Flesh?', 'Untitled', 'Lossless m4a'], 'after = from the next day on');
assert.deepEqual(names(F('', [{ field: 'added', op: 'between', a: ymd(NOW - 110 * day), b: ymd(NOW - 40 * day) }])), ['Venus as a Boy', 'Lossless m4a']);
assert.equal(dayStart('nope'), null); assert.ok(dayStart('2026-11-27') > 0);
assert.ok(Math.abs(ago(2, 'weeks', NOW) - (NOW - 14 * day)) < 1);

// choices, quality, yes/no
assert.deepEqual(names(F('', [{ field: 'format', op: 'any', a: ['MP3', 'M4A'] }])), ['Venus as a Boy', 'Untitled', 'Lossless m4a']);
assert.deepEqual(names(F('', [{ field: 'format', op: 'none', a: ['FLAC'] }])), ['Venus as a Boy', 'Untitled', 'Lossless m4a']);
assert.deepEqual(names(F('', [{ field: 'quality', op: 'any', a: ['lossless'] }])), ['In the Flesh?', 'So What', 'Lossless m4a'], 'an m4a with a bit depth is ALAC');
assert.deepEqual(names(F('', [{ field: 'quality', op: 'any', a: ['lossy'] }])), ['Venus as a Boy', 'Untitled']);
assert.deepEqual(names(F('', [{ field: 'quality', op: 'any', a: ['hires'] }])), ['So What']);
assert.deepEqual(names(F('', [{ field: 'quality', op: 'any', a: ['cd'] }])), ['In the Flesh?', 'Lossless m4a']);
assert.deepEqual(names(F('', [{ field: 'lyrics', op: 'yes' }])), ['Venus as a Boy']);
assert.equal(names(F('', [{ field: 'lyrics', op: 'no' }])).length, 4);
assert.deepEqual(names(F('', [{ field: 'favourite', op: 'yes' }])), ['So What']);
assert.equal(names(F('', [{ field: 'favourite', op: 'no' }])).length, 4);

// playlists
assert.deepEqual(names(F('', [{ field: 'playlist', op: 'in', a: ['p1'] }])), ['In the Flesh?', 'So What']);
assert.deepEqual(names(F('', [{ field: 'playlist', op: 'in', a: ['p1', 'p2'] }])), ['In the Flesh?', 'Venus as a Boy', 'So What']);
assert.deepEqual(names(F('', [{ field: 'playlist', op: 'in', a: ['*'] }])), ['In the Flesh?', 'Venus as a Boy', 'So What'], 'any playlist');
assert.deepEqual(names(F('', [{ field: 'playlist', op: 'notin', a: ['*'] }])), ['Untitled', 'Lossless m4a']);
assert.deepEqual(names(F('', [{ field: 'playlist', op: 'notin', a: ['p1'] }])), ['Venus as a Boy', 'Untitled', 'Lossless m4a']);
assert.deepEqual([...playlistsNeeded(F('', [{ field: 'playlist', op: 'in', a: ['p2'] }, { field: 'artist', op: 'contains', a: 'x' }]))], ['p2']);

// combining: all / any, with text, and unfinished rules are ignored
const two = [{ field: 'quality', op: 'any', a: ['lossless'] }, { field: 'year', op: 'ge', a: '1990' }];
assert.deepEqual(names(F('', two, 'all')), ['Lossless m4a']);
assert.deepEqual(names(F('', two, 'any')), ['In the Flesh?', 'Venus as a Boy', 'So What', 'Lossless m4a']);
assert.deepEqual(names(F('rock', two, 'all')), ['Lossless m4a']);
assert.equal(names(F('', [{ field: 'artist', op: 'contains', a: '' }, { field: 'year', op: 'between', a: '1990', b: '' }])).length, 5, 'rules without values do nothing');
assert.equal(ruleActive({ ...newRule('plays'), op: 'eq', a: '0' }), true);
assert.equal(ruleActive({ ...newRule('genre'), op: 'empty' }), true, 'no value needed');
assert.equal(ruleActive({ ...newRule('added'), op: 'last', a: '0' }), false);
assert.equal(ruleActive({ ...newRule('playlist'), op: 'in', a: [] }), false);

// sorting keeps library order for ties
const base = applyFilter(tracks, emptyFilter(), ctx);
const sorted = (k) => sortResults(base, k, ctx).map((t) => t.title);
assert.deepEqual(sorted('title'), ['In the Flesh?', 'Lossless m4a', 'So What', 'Untitled', 'Venus as a Boy']);
assert.deepEqual(sorted('year-new'), ['Lossless m4a', 'Venus as a Boy', 'In the Flesh?', 'So What', 'Untitled']);
assert.deepEqual(sorted('year-old').at(-1), 'Untitled', 'no year goes last');
assert.deepEqual(sorted('plays').slice(0, 2), ['In the Flesh?', 'Venus as a Boy']);
assert.deepEqual(sorted('played').slice(0, 2), ['In the Flesh?', 'Venus as a Boy']);
assert.deepEqual(sorted('added').slice(0, 2), ['Untitled', 'In the Flesh?']);
assert.equal(sorted('quality')[0], 'So What');
assert.equal(sorted('length-long')[0], 'So What');
assert.deepEqual(sorted('library'), base.map((t) => t.title));

// helpers
assert.equal(parseTime('3:30'), 210); assert.equal(parseTime('1:02:03'), 3723); assert.equal(parseTime('2'), 120); assert.equal(parseTime('90s'), 90);
assert.equal(parseTime('x'), null); assert.equal(parseTime(''), null);
assert.ok(isLossless(tracks[4]) && !isLossless(tracks[3]) && isHiRes(tracks[2]) && !isHiRes(tracks[0]));
assert.deepEqual(formatsIn(tracks), ['FLAC', 'M4A', 'MP3']);
assert.equal(detailLine(tracks[2]), 'FLAC · 24-bit / 96 kHz'); assert.equal(detailLine(tracks[1]), 'MP3 · 320 kbps'); assert.equal(detailLine(tracks[0]), 'FLAC · 16-bit / 44.1 kHz');
// every preset and every field work, and every field offers sensible operators
for (const p of PRESETS) { const f = F('', p.rules.map((r) => ({ ...r })), 'all'); assert.doesNotThrow(() => applyFilter(tracks, f, ctx), p.id); }
for (const f of FIELDS) { assert.ok(opsFor(f).length > 0, f.id); const r = newRule(f.id); assert.equal(r.op, opsFor(f)[0][0]); }
assert.ok(!opsFor(fieldById('plays')).some(([o]) => o === 'empty') && !opsFor(fieldById('added')).some(([o]) => o === 'never') && opsFor(fieldById('lastPlayed')).some(([o]) => o === 'never'));
assert.deepEqual(applyFilter(tracks, F('', [{ field: 'unknown-field', op: 'x', a: 'x' }]), ctx).length, 5, 'a rule for a field that does not exist is ignored');
console.log('ok');
