// Inline SVG icons (24x24, drawn with currentColor) so they follow the theme.

const P = {
  play: '<path d="M7 4.5v15l12-7.5z" fill="currentColor" stroke="none"/>',
  pause: '<rect x="6" y="4.5" width="4.2" height="15" rx="1" fill="currentColor" stroke="none"/><rect x="13.8" y="4.5" width="4.2" height="15" rx="1" fill="currentColor" stroke="none"/>',
  next: '<path d="M6 5v14l9-7z" fill="currentColor" stroke="none"/><path d="M18 5v14"/>',
  prev: '<path d="M18 5v14l-9-7z" fill="currentColor" stroke="none"/><path d="M6 5v14"/>',
  shuffle: '<path d="M3 7h3.5c3 0 4.5 3 6.5 5.5s3.5 4.5 6 4.5H21"/><path d="M3 17h3.5c1.6 0 2.8-.9 3.8-2.1"/><path d="M13 8.2C14.2 7.5 15.5 7 17 7h4"/><path d="m18.5 4.5 2.5 2.5-2.5 2.5"/><path d="m18.5 14.5 2.5 2.5-2.5 2.5"/>',
  repeat: '<path d="M4 11V9a3 3 0 0 1 3-3h13"/><path d="m17 3 3 3-3 3"/><path d="M20 13v2a3 3 0 0 1-3 3H4"/><path d="m7 21-3-3 3-3"/>',
  repeatOne: '<path d="M4 11V9a3 3 0 0 1 3-3h13"/><path d="m17 3 3 3-3 3"/><path d="M20 13v2a3 3 0 0 1-3 3H4"/><path d="m7 21-3-3 3-3"/><path d="M11 10.5l1.5-1v5.5" stroke-width="1.6"/>',
  volume: '<path d="M4 9.5v5h3.5L12 18.5v-13L7.5 9.5z" fill="currentColor"/><path d="M15.5 9a4 4 0 0 1 0 6"/><path d="M18 6.5a7.5 7.5 0 0 1 0 11"/>',
  volumeLow: '<path d="M4 9.5v5h3.5L12 18.5v-13L7.5 9.5z" fill="currentColor"/><path d="M15.5 9a4 4 0 0 1 0 6"/>',
  mute: '<path d="M4 9.5v5h3.5L12 18.5v-13L7.5 9.5z" fill="currentColor"/><path d="m16 9.5 5 5"/><path d="m21 9.5-5 5"/>',
  queue: '<path d="M4 6h12M4 11h12M4 16h7"/><path d="M17 14.5v5.2a2 2 0 1 0 1.5 1.9V14.5l3-1V11l-4.5 1.5z" fill="currentColor" stroke="none"/>',
  lyrics: '<path d="M5 5h14a1 1 0 0 1 1 1v9a1 1 0 0 1-1 1h-7l-4.5 3.5V16H5a1 1 0 0 1-1-1V6a1 1 0 0 1 1-1z"/><path d="M8 9h8M8 12h5"/>',
  eq: '<path d="M6 4v16M12 4v16M18 4v16"/><circle cx="6" cy="14" r="2" fill="currentColor"/><circle cx="12" cy="8" r="2" fill="currentColor"/><circle cx="18" cy="15" r="2" fill="currentColor"/>',
  settings: '<circle cx="12" cy="12" r="3.2"/><path d="M19.4 13.5a7.7 7.7 0 0 0 0-3l1.9-1.5-1.9-3.3-2.3.9a7.6 7.6 0 0 0-2.6-1.5l-.4-2.6H9.9l-.4 2.6a7.6 7.6 0 0 0-2.6 1.5l-2.3-.9L2.7 9l1.9 1.5a7.7 7.7 0 0 0 0 3L2.7 15l1.9 3.3 2.3-.9a7.6 7.6 0 0 0 2.6 1.5l.4 2.6h4.2l.4-2.6a7.6 7.6 0 0 0 2.6-1.5l2.3.9 1.9-3.3z"/>',
  library: '<rect x="4" y="4" width="4" height="16" rx=".8"/><rect x="10" y="4" width="4" height="16" rx=".8"/><path d="m16.2 5.6 3.6-1 3 14.6-3.6 1z"/>',
  search: '<circle cx="10.5" cy="10.5" r="6"/><path d="m15 15 5 5"/>',
  plus: '<path d="M12 5v14M5 12h14"/>',
  folder: '<path d="M3.5 7a1.5 1.5 0 0 1 1.5-1.5h4l2 2.5H19a1.5 1.5 0 0 1 1.5 1.5v8A1.5 1.5 0 0 1 19 19H5a1.5 1.5 0 0 1-1.5-1.5z"/>',
  close: '<path d="m6 6 12 12M18 6 6 18"/>',
  grip: '<circle cx="9" cy="6" r="1.4" fill="currentColor" stroke="none"/><circle cx="15" cy="6" r="1.4" fill="currentColor" stroke="none"/><circle cx="9" cy="12" r="1.4" fill="currentColor" stroke="none"/><circle cx="15" cy="12" r="1.4" fill="currentColor" stroke="none"/><circle cx="9" cy="18" r="1.4" fill="currentColor" stroke="none"/><circle cx="15" cy="18" r="1.4" fill="currentColor" stroke="none"/>',
  trash: '<path d="M5 7h14M10 7V4.5h4V7M7 7l1 12.5h8L17 7"/>',
  pip: '<rect x="3" y="5" width="18" height="14" rx="2"/><rect x="12" y="12" width="7" height="5" rx="1" fill="currentColor" stroke="none"/>',
  fullscreen: '<path d="M4 9V4h5M20 9V4h-5M4 15v5h5M20 15v5h-5"/>',
  ambient: '<circle cx="12" cy="12" r="2.6"/><path d="M7.4 7.4a6.5 6.5 0 0 0 0 9.2M16.6 7.4a6.5 6.5 0 0 1 0 9.2M4.3 4.3a10.9 10.9 0 0 0 0 15.4M19.7 4.3a10.9 10.9 0 0 1 0 15.4"/>',
  changes: '<path d="M4 6h11M4 12h7M4 18h5"/><circle cx="18" cy="15" r="3.2"/><path d="m16.6 15 1 1 1.9-2"/>',
  eye: '<path d="M2 12s3.6-7 10-7 10 7 10 7-3.6 7-10 7S2 12 2 12z"/><circle cx="12" cy="12" r="3"/>',
  edit: '<path d="M4 20h4L19.5 8.5l-4-4L4 16v4z"/><path d="m13.5 6.5 4 4"/>',
  pin: '<path d="M9 3h6l-1 6 3 3v2H7v-2l3-3-1-6zM12 14v7"/>',
  expand: '<path d="M14 4h6v6M10 20H4v-6M20 4l-7 7M4 20l7-7"/>', collapse: '<path d="M20 10h-6V4M4 14h6v6M14 10l7-7M10 14l-7 7"/>',
  chevronLeft: '<path d="m14.5 5-7 7 7 7"/>', chevronRight: '<path d="m9.5 5 7 7-7 7"/>',
  chevronDown: '<path d="m5 9.5 7 7 7-7"/>', arrowLeft: '<path d="M20 12H5M11 6l-6 6 6 6"/>',
  refresh: '<path d="M20 11a8 8 0 0 0-14.5-4M4 4v4h4"/><path d="M4 13a8 8 0 0 0 14.5 4M20 20v-4h-4"/>',
  more: '<circle cx="5.5" cy="12" r="1.6" fill="currentColor" stroke="none"/><circle cx="12" cy="12" r="1.6" fill="currentColor" stroke="none"/><circle cx="18.5" cy="12" r="1.6" fill="currentColor" stroke="none"/>',
  note: '<path d="M9 18V6l10-2v12"/><circle cx="6.5" cy="18" r="2.5" fill="currentColor"/><circle cx="16.5" cy="16" r="2.5" fill="currentColor"/>',
  record: '<circle cx="12" cy="12" r="8"/><circle cx="12" cy="12" r="2.6" fill="currentColor"/>',
  check: '<path d="m5 12.5 4.5 4.5L19 7.5"/>',
  playlist: '<path d="M4 6h11M4 11h11M4 16h6"/><path d="M18 9v8.2a2.5 2.5 0 1 0 1.8 2.4V9z" fill="currentColor" stroke="none"/>',
  heart: '<path d="M12 20s-7.5-4.6-7.5-10A4.3 4.3 0 0 1 12 7.6 4.3 4.3 0 0 1 19.5 10c0 5.4-7.5 10-7.5 10z"/>',
  sun: '<circle cx="12" cy="12" r="4"/><path d="M12 2.5v2.2M12 19.3v2.2M2.5 12h2.2M19.3 12h2.2M5.3 5.3l1.6 1.6M17.1 17.1l1.6 1.6M5.3 18.7l1.6-1.6M17.1 6.9l1.6-1.6"/>',
  moon: '<path d="M20 14.5A8.5 8.5 0 0 1 9.5 4a8.5 8.5 0 1 0 10.5 10.5z"/>',
  eyeOff: '<path d="M3 12s3.5-6.5 9-6.5S21 12 21 12s-3.5 6.5-9 6.5S3 12 3 12z"/><circle cx="12" cy="12" r="2.6"/><path d="m4 4 16 16"/>',
  viz: '<path d="M5 19V11M9.5 19V5M14 19v-9M18.5 19V8"/>',
  palette: '<path d="M12 3.5a8.5 8.5 0 1 0 0 17c1.2 0 1.8-.8 1.8-1.7 0-1.4-1.3-1.6-1.3-2.8 0-.9.7-1.5 1.6-1.5H17a3.5 3.5 0 0 0 3.5-3.5C20.5 6.8 16.7 3.5 12 3.5z"/><circle cx="7.8" cy="11.5" r="1.1" fill="currentColor" stroke="none"/><circle cx="10.2" cy="7.6" r="1.1" fill="currentColor" stroke="none"/><circle cx="14.6" cy="7.6" r="1.1" fill="currentColor" stroke="none"/>',
  keyboard: '<rect x="3" y="6" width="18" height="12" rx="2"/><path d="M7 10h.01M10.5 10h.01M14 10h.01M17.5 10h.01M7 14h10"/>',
  database: '<ellipse cx="12" cy="6" rx="7" ry="2.8"/><path d="M5 6v6c0 1.5 3.1 2.8 7 2.8s7-1.3 7-2.8V6"/><path d="M5 12v6c0 1.5 3.1 2.8 7 2.8s7-1.3 7-2.8v-6"/>',
  layout: '<rect x="3.5" y="4.5" width="17" height="15" rx="1.8"/><path d="M9.5 4.5v15M9.5 10h11"/>',
  home: '<path d="M4 11 12 4l8 7v8.5a.5.5 0 0 1-.5.5H15v-5.5H9V20H4.5a.5.5 0 0 1-.5-.5z"/>',
  sliders: '<path d="M4 7h9M17 7h3M4 17h3M11 17h9"/><circle cx="15" cy="7" r="2"/><circle cx="9" cy="17" r="2"/>',
};

/** An <svg> element for the named icon. */
export function icon(name, size = 20) {
  const t = document.createElement('template');
  t.innerHTML = `<svg class="icon icon-${name}" viewBox="0 0 24 24" width="${size}" height="${size}" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false">${P[name] ?? ''}</svg>`;
  return t.content.firstChild;
}
export const hasIcon = (n) => n in P;
