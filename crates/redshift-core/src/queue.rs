//! The play queue: what is playing, what comes next, and how shuffle and repeat bend that.
//!
//! Entries have a stable id (`EntryId`) that survives reordering, removal of other entries and
//! shuffling, so the UI and the player can refer to "that row" without caring where it sits.
//! The queue keeps a separate *play order*: with shuffle off it is the list order, with shuffle on
//! it is a random permutation with the current entry first.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

pub type EntryId = u64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueEntry {
    pub id: EntryId,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RepeatMode {
    #[default]
    Off,
    /// Loop the whole queue.
    All,
    /// Loop the current entry.
    One,
}

/// SplitMix64: tiny, fast, and good enough to shuffle a playlist.
struct Rng(u64);

impl Rng {
    fn from_clock() -> Self {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64);
        Self(nanos ^ 0x9E37_79B9_7F4A_7C15)
    }
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform in `0..n`.
    fn below(&mut self, n: usize) -> usize {
        ((self.next() as u128 * n as u128) >> 64) as usize
    }
    /// Fisher–Yates.
    fn shuffle<T>(&mut self, v: &mut [T]) {
        for i in (1..v.len()).rev() {
            v.swap(i, self.below(i + 1));
        }
    }
}

pub struct Queue {
    entries: Vec<QueueEntry>,
    /// Indices into `entries`, in the order they will play.
    order: Vec<usize>,
    /// Position within `order` of the current entry; `None` when nothing is current.
    pos: Option<usize>,
    shuffle: bool,
    repeat: RepeatMode,
    next_id: EntryId,
    rng: Rng,
}

impl Default for Queue {
    fn default() -> Self {
        Self::new()
    }
}

impl Queue {
    pub fn new() -> Self {
        Self::with_seed(None)
    }

    /// A fixed seed makes shuffling reproducible, for tests.
    pub fn with_seed(seed: Option<u64>) -> Self {
        Self {
            entries: Vec::new(),
            order: Vec::new(),
            pos: None,
            shuffle: false,
            repeat: RepeatMode::Off,
            next_id: 1,
            rng: seed.map_or_else(Rng::from_clock, Rng),
        }
    }

    pub fn entries(&self) -> &[QueueEntry] {
        &self.entries
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn shuffle(&self) -> bool {
        self.shuffle
    }
    pub fn repeat(&self) -> RepeatMode {
        self.repeat
    }

    pub fn current(&self) -> Option<&QueueEntry> {
        self.pos.map(|p| &self.entries[self.order[p]])
    }
    /// Index of the current entry in the displayed list (not the play order).
    pub fn current_index(&self) -> Option<usize> {
        self.pos.map(|p| self.order[p])
    }

    /// What `advance` would move to, without moving. Used to preload the next track.
    pub fn peek_next(&self) -> Option<&QueueEntry> {
        self.next_pos().map(|p| &self.entries[self.order[p]])
    }

    fn next_pos(&self) -> Option<usize> {
        let pos = self.pos?;
        match self.repeat {
            RepeatMode::One => Some(pos),
            _ if pos + 1 < self.order.len() => Some(pos + 1),
            RepeatMode::All => Some(0),
            RepeatMode::Off => None,
        }
    }

    /// Moves to the next entry as playback does when a track ends. Returns the new current entry,
    /// or `None` (leaving the queue where it was) at the end of an un-repeated queue.
    pub fn advance(&mut self) -> Option<&QueueEntry> {
        let p = self.next_pos()?;
        self.set_pos(p);
        self.current()
    }

    /// Like [`advance`](Self::advance) but for the user pressing "next": repeat-one doesn't pin the
    /// queue to the current entry. Returns `None` at the end of a queue that isn't repeating.
    pub fn skip_next(&mut self) -> Option<&QueueEntry> {
        if self.repeat != RepeatMode::One {
            return self.advance();
        }
        self.repeat = RepeatMode::All;
        let moved = self.advance().is_some();
        self.repeat = RepeatMode::One;
        if moved { self.current() } else { None }
    }

    /// Moves to the previous entry (the user pressed "back"). Wraps only with repeat-all.
    pub fn previous(&mut self) -> Option<&QueueEntry> {
        let pos = self.pos?;
        let p = if pos > 0 {
            pos - 1
        } else if self.repeat == RepeatMode::All {
            self.order.len() - 1
        } else {
            0
        };
        self.set_pos(p);
        self.current()
    }

    /// Every move goes through here. In a shuffled, repeating queue, landing on the last entry of a
    /// pass starts the next pass right away: a fresh shuffle with this entry first. That way
    /// `peek_next` is always exactly what will play, and the same song never plays twice in a row
    /// at the seam.
    fn set_pos(&mut self, p: usize) {
        self.pos = Some(p);
        if self.shuffle && self.repeat == RepeatMode::All && p + 1 == self.order.len() {
            let cur = self.order[p];
            let mut rest: Vec<usize> = (0..self.entries.len()).filter(|&i| i != cur).collect();
            self.rng.shuffle(&mut rest);
            self.order.clear();
            self.order.push(cur);
            self.order.extend(rest);
            self.pos = Some(0);
        }
    }

    /// Replaces the whole queue and makes `start` (an index into `paths`) current.
    pub fn set(&mut self, paths: Vec<PathBuf>, start: usize) -> Vec<EntryId> {
        self.entries.clear();
        let ids = paths.into_iter().map(|p| self.push_entry(p)).collect();
        self.pos = None;
        self.rebuild_order(if self.entries.is_empty() { None } else { Some(start.min(self.entries.len() - 1)) });
        ids
    }

    fn push_entry(&mut self, path: PathBuf) -> EntryId {
        let id = self.next_id;
        self.next_id += 1;
        self.entries.push(QueueEntry { id, path });
        id
    }

    /// Rebuilds the play order, putting entry index `current` (if any) first when shuffled.
    fn rebuild_order(&mut self, current: Option<usize>) {
        let n = self.entries.len();
        self.order = (0..n).collect();
        match current {
            None => self.pos = None,
            Some(cur) => {
                if self.shuffle {
                    self.rng.shuffle(&mut self.order);
                    let at = self.order.iter().position(|&i| i == cur).unwrap();
                    self.order.swap(0, at);
                    self.pos = Some(0);
                } else {
                    self.pos = Some(cur);
                }
            }
        }
    }

    /// Makes the entry with this id current. Returns false if there is no such entry.
    pub fn jump_to(&mut self, id: EntryId) -> bool {
        let Some(idx) = self.entries.iter().position(|e| e.id == id) else { return false };
        match self.order.iter().position(|&i| i == idx) {
            Some(p) => {
                self.set_pos(p);
                true
            }
            None => false,
        }
    }

    /// Appends to the end of the queue.
    pub fn append(&mut self, paths: Vec<PathBuf>) -> Vec<EntryId> {
        let cur = self.current_index();
        let first_new = self.entries.len();
        let ids: Vec<_> = paths.into_iter().map(|p| self.push_entry(p)).collect();
        let mut new_idx: Vec<usize> = (first_new..self.entries.len()).collect();
        if self.shuffle {
            // New entries land at random places in the part of the order still to be played.
            for i in new_idx.drain(..) {
                let lo = self.pos.map_or(0, |p| p + 1);
                let at = lo + self.rng.below(self.order.len() - lo + 1);
                self.order.insert(at, i);
            }
        } else {
            self.order.extend(new_idx);
        }
        if cur.is_none() && !self.entries.is_empty() {
            self.pos = None; // appending to an idle queue doesn't start anything
        }
        ids
    }

    /// Inserts right after the current entry ("play next"), in the given order.
    pub fn insert_next(&mut self, paths: Vec<PathBuf>) -> Vec<EntryId> {
        let at_pos = self.pos.map_or(0, |p| p + 1);
        let mut ids = Vec::new();
        for (k, path) in paths.into_iter().enumerate() {
            let id = self.next_id;
            self.next_id += 1;
            ids.push(id);
            // In the list, the new entry goes after the current one (or after the previously
            // inserted one); in the play order, at the matching spot.
            let list_at = match self.current_index() {
                Some(c) => c + 1 + k,
                None => k,
            };
            for o in &mut self.order {
                if *o >= list_at {
                    *o += 1;
                }
            }
            self.entries.insert(list_at, QueueEntry { id, path });
            self.order.insert(at_pos + k, list_at);
        }
        ids
    }

    /// Removes an entry. If it was current, the following entry becomes current (the player decides
    /// whether to start it). Returns false if the id is unknown.
    pub fn remove(&mut self, id: EntryId) -> bool {
        let Some(idx) = self.entries.iter().position(|e| e.id == id) else { return false };
        let order_at = self.order.iter().position(|&i| i == idx).unwrap();
        self.entries.remove(idx);
        self.order.remove(order_at);
        for o in &mut self.order {
            if *o > idx {
                *o -= 1;
            }
        }
        self.pos = match self.pos {
            _ if self.entries.is_empty() => None,
            Some(p) if order_at < p => Some(p - 1),
            // The current entry was removed: the one after it slides into its place.
            Some(p) if order_at == p => Some(p.min(self.order.len() - 1)),
            other => other,
        };
        true
    }

    /// Moves the entry at list index `from` to list index `to`. The current entry stays current.
    pub fn move_entry(&mut self, from: usize, to: usize) -> bool {
        let n = self.entries.len();
        if from >= n || to >= n {
            return false;
        }
        if from == to {
            return true;
        }
        let cur_id = self.current().map(|e| e.id);
        let order_ids: Vec<EntryId> = self.order.iter().map(|&i| self.entries[i].id).collect();
        let entry = self.entries.remove(from);
        self.entries.insert(to, entry);
        // Rebuild the order from ids. With shuffle off the order is just the list again.
        self.order = if self.shuffle {
            order_ids
                .iter()
                .map(|id| self.entries.iter().position(|e| e.id == *id).unwrap())
                .collect()
        } else {
            (0..n).collect()
        };
        self.pos = cur_id.map(|id| {
            let idx = self.entries.iter().position(|e| e.id == id).unwrap();
            self.order.iter().position(|&i| i == idx).unwrap()
        });
        true
    }

    /// Moves the entry at play position `from` to play position `to`; what the queue shows is the
    /// play order, so that is what a drag in it means. With shuffle off the play order is the list.
    /// The current entry stays current.
    pub fn move_in_play_order(&mut self, from: usize, to: usize) -> bool {
        if !self.shuffle {
            return self.move_entry(from, to);
        }
        let n = self.order.len();
        if from >= n || to >= n {
            return false;
        }
        if from != to {
            let cur = self.current_index();
            let moved = self.order.remove(from);
            self.order.insert(to, moved);
            self.pos = cur.map(|c| self.order.iter().position(|&i| i == c).unwrap());
        }
        true
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
        self.pos = None;
    }

    pub fn set_repeat(&mut self, mode: RepeatMode) {
        self.repeat = mode;
    }

    /// Turns shuffle on or off. The current entry keeps playing; on, everything else is shuffled
    /// after it; off, play continues in list order from the current entry.
    pub fn set_shuffle(&mut self, on: bool) {
        if self.shuffle == on {
            return;
        }
        self.shuffle = on;
        let cur = self.current_index();
        self.rebuild_order(cur);
    }

    /// Entries in the order they will play (for showing "up next").
    pub fn play_order(&self) -> impl Iterator<Item = &QueueEntry> {
        self.order.iter().map(|&i| &self.entries[i])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(names: &[&str]) -> Vec<PathBuf> {
        names.iter().map(PathBuf::from).collect()
    }
    fn name(e: Option<&QueueEntry>) -> Option<String> {
        e.map(|e| e.path.to_string_lossy().into_owned())
    }
    fn cur(q: &Queue) -> Option<String> {
        name(q.current())
    }
    fn upcoming(q: &Queue) -> Vec<String> {
        let skip = q.pos.map_or(0, |p| p + 1);
        q.play_order().skip(skip).map(|e| e.path.to_string_lossy().into_owned()).collect()
    }
    fn queue(names: &[&str], start: usize) -> Queue {
        let mut q = Queue::with_seed(Some(7));
        q.set(paths(names), start);
        q
    }

    #[test]
    fn plays_in_order_and_stops_at_the_end() {
        let mut q = queue(&["a", "b", "c"], 0);
        assert_eq!(cur(&q).as_deref(), Some("a"));
        assert_eq!(name(q.peek_next()).as_deref(), Some("b"));
        assert_eq!(name(q.advance()).as_deref(), Some("b"));
        assert_eq!(name(q.advance()).as_deref(), Some("c"));
        assert!(q.peek_next().is_none());
        assert!(q.advance().is_none());
        assert_eq!(cur(&q).as_deref(), Some("c"), "stays on the last entry");
    }

    #[test]
    fn start_index_is_respected_and_clamped() {
        assert_eq!(cur(&queue(&["a", "b", "c"], 2)).as_deref(), Some("c"));
        assert_eq!(cur(&queue(&["a", "b", "c"], 99)).as_deref(), Some("c"));
        let q = queue(&[], 0);
        assert!(q.current().is_none() && q.peek_next().is_none());
    }

    #[test]
    fn repeat_all_wraps() {
        let mut q = queue(&["a", "b"], 1);
        q.set_repeat(RepeatMode::All);
        assert_eq!(name(q.peek_next()).as_deref(), Some("a"));
        assert_eq!(name(q.advance()).as_deref(), Some("a"));
        assert_eq!(name(q.advance()).as_deref(), Some("b"));
    }

    #[test]
    fn repeat_one_stays_put_but_manual_previous_still_moves() {
        let mut q = queue(&["a", "b", "c"], 1);
        q.set_repeat(RepeatMode::One);
        assert_eq!(name(q.peek_next()).as_deref(), Some("b"));
        assert_eq!(name(q.advance()).as_deref(), Some("b"));
        assert_eq!(name(q.previous()).as_deref(), Some("a"));
    }

    #[test]
    fn skip_next_leaves_the_entry_even_under_repeat_one() {
        let mut q = queue(&["a", "b"], 0);
        q.set_repeat(RepeatMode::One);
        assert_eq!(name(q.skip_next()).as_deref(), Some("b"));
        assert_eq!(name(q.skip_next()).as_deref(), Some("a"), "wraps, as repeat-one implies looping");
        assert_eq!(q.repeat(), RepeatMode::One, "mode is restored");
        let mut off = queue(&["a", "b"], 1);
        assert!(off.skip_next().is_none());
    }

    #[test]
    fn previous_stops_at_the_start_unless_repeating_all() {
        let mut q = queue(&["a", "b", "c"], 0);
        assert_eq!(name(q.previous()).as_deref(), Some("a"));
        q.set_repeat(RepeatMode::All);
        assert_eq!(name(q.previous()).as_deref(), Some("c"));
    }

    #[test]
    fn shuffle_is_a_permutation_with_the_current_entry_first() {
        let names: Vec<String> = (0..50).map(|i| format!("t{i}")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let mut q = queue(&refs, 17);
        q.set_shuffle(true);
        assert_eq!(cur(&q).as_deref(), Some("t17"), "the playing track must not change");
        let mut seen = vec![cur(&q).unwrap()];
        while let Some(e) = q.advance() {
            seen.push(e.path.to_string_lossy().into_owned());
        }
        assert_eq!(seen.len(), 50, "every entry exactly once");
        let mut sorted = seen.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), 50);
        assert_ne!(seen, names, "a shuffle that returns the original order is not a shuffle");
        // Not just rotated: neighbours in the shuffled order are rarely neighbours in the list.
        let adjacent = seen.windows(2).filter(|w| {
            let n = |s: &String| s[1..].parse::<i32>().unwrap();
            (n(&w[0]) - n(&w[1])).abs() == 1
        }).count();
        assert!(adjacent < 10, "{adjacent} of 49 neighbours kept");
    }

    #[test]
    fn turning_shuffle_off_continues_in_list_order_from_the_current_entry() {
        let mut q = queue(&["a", "b", "c", "d", "e"], 0);
        q.set_shuffle(true);
        while q.current().is_some_and(|e| e.path != PathBuf::from("c")) {
            q.advance();
        }
        q.set_shuffle(false);
        assert_eq!(cur(&q).as_deref(), Some("c"));
        assert_eq!(upcoming(&q), ["d", "e"]);
    }

    #[test]
    fn shuffled_repeat_all_keeps_going_with_fresh_passes_and_no_immediate_repeats() {
        for seed in 0..10 {
            let mut q = Queue::with_seed(Some(seed));
            q.set(paths(&["a", "b", "c", "d", "e", "f"]), 0);
            q.set_shuffle(true);
            q.set_repeat(RepeatMode::All);
            let mut plays = vec![cur(&q).unwrap()];
            for _ in 0..100 {
                let promised = name(q.peek_next());
                let got = name(q.advance());
                assert_eq!(got, promised, "peek_next must match what advance does, even at a pass boundary");
                plays.push(got.unwrap());
            }
            assert!(plays.windows(2).all(|w| w[0] != w[1]), "same track twice in a row: {plays:?}");
            // Structure: the first pass is a full permutation (6 plays). From then on, each cycle of
            // 5 plays covers every track except the one that just ended, which seeds the cycle.
            let distinct = |w: &[String]| {
                let mut v = w.to_vec();
                v.sort();
                v.dedup();
                v.len() == w.len()
            };
            assert!(distinct(&plays[..6]), "first pass repeats a track: {:?}", &plays[..6]);
            let mut cycles_differ = false;
            for k in 0..19 {
                let seed_track = &plays[5 + 5 * k];
                let cycle = &plays[6 + 5 * k..11 + 5 * k];
                assert!(distinct(cycle), "cycle {k} repeats a track: {cycle:?}");
                assert!(!cycle.contains(seed_track), "cycle {k} replays {seed_track} before the others");
                cycles_differ |= cycle != &plays[6..11];
            }
            assert!(cycles_differ, "every pass had the same order");
        }
    }

    #[test]
    fn jump_to_by_id_survives_reordering() {
        let mut q = queue(&["a", "b", "c", "d"], 0);
        let id_d = q.entries()[3].id;
        q.move_entry(3, 0);
        assert!(q.jump_to(id_d));
        assert_eq!(cur(&q).as_deref(), Some("d"));
        assert_eq!(q.current_index(), Some(0));
        assert!(!q.jump_to(9999));
    }

    #[test]
    fn append_does_not_disturb_current_or_start_playback() {
        let mut q = queue(&["a", "b"], 0);
        q.append(paths(&["x", "y"]));
        assert_eq!(cur(&q).as_deref(), Some("a"));
        assert_eq!(upcoming(&q), ["b", "x", "y"]);

        let mut idle = Queue::with_seed(Some(1));
        idle.append(paths(&["x"]));
        assert!(idle.current().is_none());
        assert_eq!(idle.len(), 1);
    }

    #[test]
    fn append_while_shuffled_only_lands_in_the_unplayed_part() {
        for seed in 0..20 {
            let mut q = Queue::with_seed(Some(seed));
            q.set(paths(&["a", "b", "c", "d", "e", "f"]), 0);
            q.set_shuffle(true);
            q.advance();
            q.advance();
            let played: Vec<_> = q.play_order().take(3).map(|e| e.id).collect();
            let now = cur(&q);
            q.append(paths(&["x", "y"]));
            assert_eq!(cur(&q), now);
            assert_eq!(q.play_order().take(3).map(|e| e.id).collect::<Vec<_>>(), played, "history is untouched");
            assert_eq!(q.play_order().count(), 8);
        }
    }

    #[test]
    fn insert_next_plays_in_the_given_order_right_after_current() {
        let mut q = queue(&["a", "b", "c"], 0);
        q.insert_next(paths(&["x", "y"]));
        assert_eq!(upcoming(&q), ["x", "y", "b", "c"]);
        assert_eq!(name(q.advance()).as_deref(), Some("x"));
        q.insert_next(paths(&["z"]));
        assert_eq!(upcoming(&q), ["z", "y", "b", "c"]);
        // The list view agrees with the play order when shuffle is off.
        let listed: Vec<_> = q.entries().iter().map(|e| e.path.to_string_lossy().into_owned()).collect();
        assert_eq!(listed, ["a", "x", "z", "y", "b", "c"]);
    }

    #[test]
    fn insert_next_works_while_shuffled_and_on_an_idle_queue() {
        let mut q = queue(&["a", "b", "c", "d"], 0);
        q.set_shuffle(true);
        q.insert_next(paths(&["x"]));
        assert_eq!(name(q.peek_next()).as_deref(), Some("x"));
        assert_eq!(q.play_order().count(), 5);

        let mut idle = Queue::with_seed(Some(1));
        idle.insert_next(paths(&["x", "y"]));
        assert_eq!(idle.len(), 2);
        assert!(idle.current().is_none());
    }

    #[test]
    fn removing_entries_keeps_the_current_one_current() {
        let mut q = queue(&["a", "b", "c", "d"], 2);
        let (id_a, id_d) = (q.entries()[0].id, q.entries()[3].id);
        assert!(q.remove(id_a));
        assert_eq!(cur(&q).as_deref(), Some("c"));
        assert!(q.remove(id_d));
        assert_eq!(cur(&q).as_deref(), Some("c"));
        assert_eq!(upcoming(&q), Vec::<String>::new());
        assert!(!q.remove(id_a), "already gone");
    }

    #[test]
    fn removing_the_current_entry_moves_to_the_one_after_it() {
        let mut q = queue(&["a", "b", "c"], 1);
        let id_b = q.current().unwrap().id;
        q.remove(id_b);
        assert_eq!(cur(&q).as_deref(), Some("c"));
        let id_c = q.current().unwrap().id;
        q.remove(id_c);
        assert_eq!(cur(&q).as_deref(), Some("a"), "falls back to the last remaining entry");
        let id_a = q.current().unwrap().id;
        q.remove(id_a);
        assert!(q.current().is_none() && q.is_empty());
    }

    #[test]
    fn removal_keeps_shuffled_order_consistent() {
        let mut q = Queue::with_seed(Some(3));
        q.set(paths(&["a", "b", "c", "d", "e", "f", "g"]), 3);
        q.set_shuffle(true);
        let order_before: Vec<_> = q.play_order().map(|e| e.id).collect();
        let victim = order_before[4];
        q.remove(victim);
        let expected: Vec<_> = order_before.into_iter().filter(|&i| i != victim).collect();
        assert_eq!(q.play_order().map(|e| e.id).collect::<Vec<_>>(), expected);
        assert_eq!(cur(&q).as_deref(), Some("d"));
    }

    #[test]
    fn move_entry_reorders_the_list_and_keeps_the_current_entry() {
        let mut q = queue(&["a", "b", "c", "d"], 1);
        assert!(q.move_entry(0, 3));
        let listed: Vec<_> = q.entries().iter().map(|e| e.path.to_string_lossy().into_owned()).collect();
        assert_eq!(listed, ["b", "c", "d", "a"]);
        assert_eq!(cur(&q).as_deref(), Some("b"));
        assert_eq!(upcoming(&q), ["c", "d", "a"]);
        assert!(!q.move_entry(0, 9));
        assert!(q.move_entry(2, 2));
    }

    #[test]
    fn moving_in_play_order_reorders_what_plays_next() {
        let mut q = queue(&["a", "b", "c", "d", "e", "f"], 2);
        q.set_shuffle(true);
        let before: Vec<_> = q.play_order().map(|e| e.id).collect();
        let now = cur(&q);
        assert!(q.move_in_play_order(5, 1));
        let after: Vec<_> = q.play_order().map(|e| e.id).collect();
        assert_eq!(after[1], before[5], "dropped where it was put");
        assert_eq!(after.len(), before.len());
        assert_eq!(cur(&q), now, "the current entry stays current");
        assert_eq!(q.entries().len(), 6, "the list itself is untouched");
        assert!(!q.move_in_play_order(0, 9));
        // Shuffle off: the play order is the list, so it is the same move.
        let mut q = queue(&["a", "b", "c"], 0);
        assert!(q.move_in_play_order(0, 2));
        assert_eq!(q.entries().iter().map(|e| e.path.to_string_lossy().into_owned()).collect::<Vec<_>>(), ["b", "c", "a"]);
    }

    #[test]
    fn moving_the_current_entry_while_playing() {
        let mut q = queue(&["a", "b", "c"], 0);
        q.move_entry(0, 2);
        assert_eq!(cur(&q).as_deref(), Some("a"));
        assert_eq!(upcoming(&q), Vec::<String>::new(), "a is now last in the list");
        assert_eq!(q.current_index(), Some(2));
    }

    #[test]
    fn set_replaces_everything_and_ids_are_never_reused() {
        let mut q = queue(&["a", "b"], 0);
        let old: Vec<_> = q.entries().iter().map(|e| e.id).collect();
        let new_ids = q.set(paths(&["x", "y", "z"]), 1);
        assert_eq!(new_ids.len(), 3);
        assert!(new_ids.iter().all(|i| !old.contains(i)));
        assert_eq!(cur(&q).as_deref(), Some("y"));
        q.clear();
        assert!(q.is_empty() && q.current().is_none());
    }

    #[test]
    fn rng_is_roughly_uniform() {
        let mut rng = Rng(42);
        let mut counts = [0usize; 10];
        for _ in 0..100_000 {
            counts[rng.below(10)] += 1;
        }
        assert!(counts.iter().all(|&c| (9_000..11_000).contains(&c)), "{counts:?}");
    }
}
