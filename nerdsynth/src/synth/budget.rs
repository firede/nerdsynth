//! The work budget: how much work events may cost, so that no stream of
//! events makes rendering fall behind.
//!
//! Work is counted in units of about a nanosecond on a recent desktop
//! machine. The budget earns [`RATE`] units a second of output, a quarter
//! of the time, and holds at most [`BURST`]. Each event spends what it costs;
//! while the budget is spent, events due wait in the queue, in order, and
//! take effect at the first multiple of 64 frames where the budget is back
//! above 0 (ending a chunk for them would cost more than the frames earn). An event
//! always takes effect whole, so the budget can go below 0 by one event's
//! cost. Everything is counted in whole units per frame, so when events
//! take effect does not depend on the lengths `render` is called with.

/// Units earned a second of output.
const RATE: u64 = 250_000_000;
/// The most units held: about two milliseconds of work at once, enough
/// for a chord of 256 notes.
const BURST: i64 = 2_000_000;

/// Any event.
pub(crate) const EVENT: i64 = 64;
/// Each voice slot, for an event that goes through them (a note off, a
/// pedal released, All Notes Off and the like).
pub(crate) const SLOT: i64 = 1;
/// Each voice slot, for a note on: it goes through them several times, to
/// release the same key, to find slots and to stop an exclusive class.
pub(crate) const NOTE_ON_SLOT: i64 = 4;
/// Each preset zone and instrument zone a note on goes through.
pub(crate) const PAIR: i64 = 4;
/// Each voice a note on starts, with its first segment.
pub(crate) const VOICE: i64 = 2048;
/// Each modulator of a voice started.
pub(crate) const VOICE_MODULATOR: i64 = 32;
/// Each voice whose segment an event cuts.
pub(crate) const CUT: i64 = 256;
/// Each modulator of a voice whose segment an event cuts.
pub(crate) const CUT_MODULATOR: i64 = 8;
/// Each chunk an event ends before the block does.
pub(crate) const CHUNK: i64 = 64;
/// Each voice slot, for such a chunk: rendering goes through the slots,
/// each in its own cache lines.
pub(crate) const CHUNK_SLOT: i64 = 4;
/// Setting every channel to its initial state.
pub(crate) const RESET: i64 = 4096;
/// Clearing the effects' delay lines.
pub(crate) const CLEAR_EFFECTS: i64 = 16384;

pub(crate) struct Budget {
    units: i64,
    per_frame: i64,
}

impl Budget {
    /// A full budget at `sample_rate`.
    pub(crate) fn new(sample_rate: u32) -> Budget {
        Budget { units: BURST, per_frame: (RATE / u64::from(sample_rate.max(1))) as i64 }
    }

    /// Fills the budget.
    pub(crate) fn fill(&mut self) {
        self.units = BURST;
    }

    /// Whether an event may take effect now.
    pub(crate) fn ready(&self) -> bool {
        self.units > 0
    }

    pub(crate) fn spend(&mut self, units: i64) {
        self.units = self.units.saturating_sub(units);
    }

    /// Earns the units of `frames` frames rendered.
    pub(crate) fn earn(&mut self, frames: usize) {
        self.units = self.units.saturating_add(self.per_frame.saturating_mul(frames as i64)).min(BURST);
    }

    /// The frames to render before an event may take effect: 0 if one may
    /// now.
    pub(crate) fn frames_until_ready(&self) -> u64 {
        if self.ready() { 0 } else { self.units.unsigned_abs() / self.per_frame as u64 + 1 }
    }
}
