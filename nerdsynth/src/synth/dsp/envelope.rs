//! The volume and modulation envelopes (§8.1.2, generators 25–40), after
//! FluidSynth's `fluid_adsr_env`: a value from 0 to 1 that rises linearly
//! in the attack, holds, falls linearly in the decay until the sustain
//! level, and falls linearly in the release. How the value becomes a gain
//! or a modulation is up to the voice: for the volume envelope, the attack
//! is linear in amplitude and the other stages linear in decibels, 1 being
//! 0 dB and 0 being -96 dB.
//!
//! The lengths of the stages are given in frames. The voice makes them
//! FluidSynth's, which counts stages in blocks of 64 frames: each a whole
//! number of blocks (`delay_frames`, `ramp_frames` and `hold_frames` in
//! `voice.rs`), so that notes start and end as there. Within them the
//! envelope moves on frame by frame, not block by block.

/// A stage of an envelope.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Stage {
    Delay,
    Attack,
    Hold,
    Decay,
    Sustain,
    Release,
    Finished,
}

/// The lengths of the stages in frames, and the sustain level.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Params {
    pub(crate) delay: u64,
    /// At least 1.
    pub(crate) attack: u64,
    pub(crate) hold: u64,
    /// The frames the decay takes from 1 to 0; it stops at the sustain
    /// level. 0 jumps to the sustain level.
    pub(crate) decay: u64,
    /// 0 to 1.
    pub(crate) sustain: f64,
    /// The frames the release takes from 1 to 0; at least 1.
    pub(crate) release: u64,
}

impl Default for Params {
    fn default() -> Self {
        Params { delay: 0, attack: 1, hold: 0, decay: 0, sustain: 1.0, release: 1 }
    }
}

/// Where an envelope is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Envelope {
    pub(crate) stage: Stage,
    /// The frames spent in the delay or the hold.
    pub(crate) count: u64,
    pub(crate) value: f64,
}

impl Envelope {
    pub(crate) fn new() -> Envelope {
        Envelope { stage: Stage::Delay, count: 0, value: 0.0 }
    }

    /// Goes on `frames` frames.
    pub(crate) fn advance(&mut self, p: &Params, mut frames: u64) {
        while frames > 0 {
            match self.stage {
                Stage::Delay | Stage::Hold => {
                    let length = if self.stage == Stage::Delay { p.delay } else { p.hold };
                    let left = length.saturating_sub(self.count);
                    if left > frames {
                        self.count += frames;
                        return;
                    }
                    frames -= left;
                    self.count = 0;
                    if self.stage == Stage::Delay {
                        self.stage = Stage::Attack;
                        self.value = 0.0;
                    } else {
                        self.stage = Stage::Decay;
                        self.value = 1.0;
                    }
                }
                Stage::Attack => {
                    let step = 1.0 / p.attack.max(1) as f64;
                    let need = steps(1.0 - self.value, step);
                    if need > frames {
                        self.value += frames as f64 * step;
                        return;
                    }
                    frames -= need;
                    self.value = 1.0;
                    self.stage = Stage::Hold;
                    self.count = 0;
                }
                Stage::Decay => {
                    let sustain = p.sustain.clamp(0.0, 1.0);
                    if p.decay == 0 || self.value <= sustain {
                        self.value = self.value.min(sustain);
                        self.stage = Stage::Sustain;
                        continue;
                    }
                    let step = 1.0 / p.decay as f64;
                    let need = steps(self.value - sustain, step);
                    if need > frames {
                        self.value -= frames as f64 * step;
                        return;
                    }
                    frames -= need;
                    self.value = sustain;
                    self.stage = Stage::Sustain;
                }
                Stage::Sustain | Stage::Finished => return,
                Stage::Release => {
                    let step = 1.0 / p.release.max(1) as f64;
                    let need = steps(self.value, step);
                    if need > frames {
                        self.value -= frames as f64 * step;
                        return;
                    }
                    self.value = 0.0;
                    self.stage = Stage::Finished;
                    return;
                }
            }
        }
    }

    /// Goes to the release, from the value it has.
    pub(crate) fn release(&mut self) {
        if self.stage < Stage::Release {
            self.stage = Stage::Release;
            self.count = 0;
            self.value = self.value.clamp(0.0, 1.0);
        }
    }
}

/// The steps of `step` it takes to cover `distance`, at least 0.
fn steps(distance: f64, step: f64) -> u64 {
    if distance.is_nan() || distance <= 0.0 {
        return 0;
    }
    // Allow for rounding: a distance of 3.0000000001 steps is 3 steps.
    let n = distance / step;
    let r = n.round();
    if (n - r).abs() < 1e-9 { r as u64 } else { n.ceil() as u64 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> Params {
        Params { delay: 10, attack: 100, hold: 50, decay: 200, sustain: 0.5, release: 400 }
    }

    #[test]
    fn stages_take_their_frames() {
        let p = params();
        let mut e = Envelope::new();
        e.advance(&p, 10);
        assert_eq!((e.stage, e.value), (Stage::Attack, 0.0));
        e.advance(&p, 50);
        assert_eq!(e.stage, Stage::Attack);
        assert!((e.value - 0.5).abs() < 1e-12);
        e.advance(&p, 50);
        assert_eq!((e.stage, e.value), (Stage::Hold, 1.0));
        e.advance(&p, 50);
        assert_eq!((e.stage, e.value), (Stage::Decay, 1.0));
        e.advance(&p, 50);
        assert!((e.value - 0.75).abs() < 1e-12);
        e.advance(&p, 50);
        assert_eq!((e.stage, e.value), (Stage::Sustain, 0.5));
        e.advance(&p, 1_000_000);
        assert_eq!((e.stage, e.value), (Stage::Sustain, 0.5));
        e.release();
        e.advance(&p, 100);
        assert!((e.value - 0.25).abs() < 1e-12);
        e.advance(&p, 100);
        assert_eq!((e.stage, e.value), (Stage::Finished, 0.0));
    }

    #[test]
    fn splitting_does_not_matter() {
        let p = params();
        let mut a = Envelope::new();
        let mut b = Envelope::new();
        a.advance(&p, 333);
        for _ in 0..333 {
            b.advance(&p, 1);
        }
        assert_eq!(a.stage, b.stage);
        assert!((a.value - b.value).abs() < 1e-9);
    }
}
