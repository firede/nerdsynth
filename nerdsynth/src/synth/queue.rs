//! The event queue: a binary heap of fixed capacity, ordered by frame and,
//! within a frame, by the order the events were queued. It never
//! allocates after it is made.

use std::cmp::{Ordering, Reverse};
use std::collections::BinaryHeap;

use super::channel::BankStyle;
use crate::Event;

/// What waits in the queue: a MIDI event, or, inside the crate, an action
/// no MIDI event can say.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Queued {
    Midi(Event),
    /// The channels of the mask (bit `n` for channel `n`) to their initial
    /// state, as [`Synth::reset`](super::Synth::reset) sets them, and bank
    /// selects to follow `BankStyle` on every channel; the voices and the
    /// effects go on. A sequencer sends it, for the channels its file
    /// uses, when it jumps.
    ResetChannels(BankStyle, u16),
}

#[derive(Clone, Copy, Debug)]
struct Entry {
    frame: u64,
    seq: u64,
    item: Queued,
}

impl PartialEq for Entry {
    fn eq(&self, other: &Self) -> bool {
        (self.frame, self.seq) == (other.frame, other.seq)
    }
}

impl Eq for Entry {}

impl PartialOrd for Entry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Entry {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.frame, self.seq).cmp(&(other.frame, other.seq))
    }
}

pub(crate) struct Queue {
    heap: BinaryHeap<Reverse<Entry>>,
    capacity: usize,
    seq: u64,
}

impl Queue {
    pub(crate) fn new(capacity: usize) -> Queue {
        Queue { heap: BinaryHeap::with_capacity(capacity), capacity, seq: 0 }
    }

    /// Queues `item` for `frame`; false when full.
    pub(crate) fn push(&mut self, frame: u64, item: Queued) -> bool {
        if self.heap.len() >= self.capacity {
            return false;
        }
        self.heap.push(Reverse(Entry { frame, seq: self.seq, item }));
        self.seq = self.seq.wrapping_add(1);
        true
    }

    /// The frame of the next event.
    pub(crate) fn next_frame(&self) -> Option<u64> {
        self.heap.peek().map(|e| e.0.frame)
    }

    /// The next item if it is for `frame` or before.
    pub(crate) fn pop_due(&mut self, frame: u64) -> Option<Queued> {
        if self.next_frame()? <= frame { self.heap.pop().map(|e| e.0.item) } else { None }
    }

    pub(crate) fn clear(&mut self) {
        self.heap.clear();
    }

    pub(crate) fn len(&self) -> usize {
        self.heap.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(key: u8) -> Queued {
        Queued::Midi(Event::NoteOn { channel: 0, key, velocity: 1 })
    }

    #[test]
    fn frames_then_order_of_queueing() {
        let mut q = Queue::new(16);
        for (frame, key) in [(10, 1), (5, 2), (10, 3), (5, 4), (0, 5), (10, 6)] {
            assert!(q.push(frame, ev(key)));
        }
        // A reset of the channels keeps its place like any event.
        assert!(q.push(5, Queued::ResetChannels(BankStyle::Gs, 0xFFFF)));
        let mut out = vec![];
        while let Some(e) = q.pop_due(u64::MAX) {
            match e {
                Queued::Midi(Event::NoteOn { key, .. }) => out.push(key),
                Queued::ResetChannels(..) => out.push(0),
                _ => {}
            }
        }
        assert_eq!(out, [5, 2, 4, 0, 1, 3, 6]);
    }

    #[test]
    fn full_is_full() {
        let mut q = Queue::new(2);
        assert!(q.push(0, ev(0)));
        assert!(q.push(0, ev(0)));
        assert!(!q.push(0, ev(0)));
        assert_eq!(q.pop_due(0), Some(ev(0)));
        assert!(q.push(0, ev(0)));
    }
}
