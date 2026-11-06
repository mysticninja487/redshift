import * as hero from './hero.js';
import * as info from './info.js';
import * as lyrics from './lyrics.js';
import * as meters from './meters.js';
import * as queue from './queue.js';
import * as seek from './seek.js';
import * as transport from './transport.js';
import * as viz from './vizpanel.js';

// The equaliser is not a layout panel: it lives in the drawer (ui/eqdrawer.js), so there is only ever one.
export const MODULES = { hero, info, transport, seek, queue, lyrics, viz, meters };
