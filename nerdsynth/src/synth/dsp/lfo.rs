//! The LFOs (§8.1.2, generators 21–24): a triangle from 0 rising to 1,
//! falling to -1 and back, after a delay, as FluidSynth's `fluid_lfo`.

/// Where an LFO is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Lfo {
    /// The frames since the note started, up to the delay.
    elapsed: u64,
    /// The phase, 0 to 1, a cycle being 1.
    phase: f64,
}

impl Lfo {
    pub(crate) fn new() -> Lfo {
        Lfo { elapsed: 0, phase: 0.0 }
    }

    /// Goes on `frames` frames at `increment` cycles a frame, after `delay`
    /// frames from the start.
    pub(crate) fn advance(&mut self, frames: u64, delay: u64, increment: f64) {
        let waiting = delay.saturating_sub(self.elapsed).min(frames);
        self.elapsed = self.elapsed.saturating_add(waiting);
        let moving = frames - waiting;
        if moving > 0 && increment.is_finite() {
            self.phase = (self.phase + moving as f64 * increment).fract();
            if self.phase.is_nan() || self.phase < 0.0 {
                self.phase = 0.0;
            }
        }
    }

    /// The value, -1 to 1.
    pub(crate) fn value(&self) -> f64 {
        let p = self.phase;
        if p < 0.25 {
            4.0 * p
        } else if p < 0.75 {
            2.0 - 4.0 * p
        } else {
            4.0 * p - 4.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_triangle_after_the_delay() {
        let mut l = Lfo::new();
        l.advance(100, 100, 0.01);
        assert_eq!(l.value(), 0.0);
        l.advance(25, 100, 0.01);
        assert!((l.value() - 1.0).abs() < 1e-12);
        l.advance(50, 100, 0.01);
        assert!((l.value() + 1.0).abs() < 1e-12);
        l.advance(25, 100, 0.01);
        assert!(l.value().abs() < 1e-12);
        // Hours later the phase is still exact enough.
        l.advance(44100 * 3600 * 50, 100, 0.01);
        assert!(l.value().abs() < 1e-3, "{}", l.value());
    }
}
