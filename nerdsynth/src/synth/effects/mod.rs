//! The send effects: a reverb and a chorus, written for this crate (not
//! ported): the reverb is an eight-line feedback delay network with swung
//! line lengths ([`reverb`]), the chorus three sine-swung taps of one delay
//! line ([`chorus`]). Their settings are fixed, chosen to sound close to
//! FluidSynth's defaults; `tools/fxmetrics` measures both against
//! FluidSynth's rendered output.
//!
//! Each effect is a type with:
//!
//! - `new(sample_rate: f64) -> Self`: allocates everything it will need,
//!   its delays scaled to the sample rate;
//! - `reset(&mut self)`: back to exactly the state `new` made;
//! - `clear_lines(&mut self)`: on a GM/GS system reset, zeroes what it holds
//!   of the signal (delay lines, filter states), keeping their sines going;
//! - `process<S: Sample>(&mut self, input: &[S], left: &mut [S], right: &mut [S])`:
//!   reads the mono send bus `input` and adds its wet stereo output into
//!   `left`/`right` (shortest length processed);
//! - `is_silent(&self) -> bool`: constant time; true once its lines hold
//!   only zeros;
//! - `skip(&mut self, frames: usize)`: called instead of `process` while
//!   `is_silent` and the input is all zeros; leaves the effect exactly as
//!   processing that silence would.
//!
//! # What makes them safe and repeatable
//!
//! They compute in `f64`, each frame's arithmetic the same whatever came
//! before it in the call, so the output depends only on the input, not on
//! how it is cut into calls.
//! Input goes through [`sanitize`]: non-finite samples count as 0, huge ones
//! are clamped, tiny ones flushed, so the states stay finite and never go
//! subnormal; should a state still turn non-finite, it is cleared. Every
//! buffer is allocated in `new`; nothing indexes out of a buffer whatever
//! the state.
//!
//! Silence is exact: the chorus, having no feedback, holds only zeros once
//! a line's length of zero input has passed; the reverb counts the frames
//! its input and output stay below [`SILENT_LEVEL`], and once that is longer
//! than the way through its filters, zeroes them. A line of zeros stays so
//! and gives zeros whatever its position, so `skip` has nothing to do but
//! move the sines, whose phases are integers, on by `frames` steps in one
//! multiplication.
//!
//! # How the synth uses them
//!
//! A voice is mixed into each effect's mono bus like into the dry buses,
//! with the pan factor replaced by the send (`reverb_send`, `chorus_send`:
//! the generators / 1000 clipped to 0..1). After all voices of a chunk are
//! mixed, the reverb adds its output to the dry left/right buses, then the
//! chorus does: `out = (dry + reverb) + chorus`.

mod chorus;
mod reverb;

pub(crate) use chorus::Chorus;
pub(crate) use reverb::Reverb;

/// A sample type the effects read from and add into: `f32` or `f64`.
///
/// The effects compute in `f64`; with `f32` buffers each output is added in
/// `f64` and rounded once.
pub(crate) trait Sample: Copy {
    fn to_f64(self) -> f64;
    /// `self += value`.
    fn add_f64(&mut self, value: f64);
}

impl Sample for f32 {
    #[inline]
    fn to_f64(self) -> f64 {
        f64::from(self)
    }

    #[inline]
    fn add_f64(&mut self, value: f64) {
        *self = (f64::from(*self) + value) as f32;
    }
}

impl Sample for f64 {
    #[inline]
    fn to_f64(self) -> f64 {
        self
    }

    #[inline]
    fn add_f64(&mut self, value: f64) {
        *self += value;
    }
}

/// The largest input magnitude passed on to an effect; larger ones are clamped.
///
/// Far above any real signal (1.0 is full scale), it only keeps an absurd input
/// from producing outputs that overflow `f32`.
const MAX_INPUT: f64 = 1.0e6;

/// Below this magnitude a signal or state value counts as silent (-140 dB).
const SILENT_LEVEL: f64 = 1.0e-7;

/// Below this magnitude an input sample is taken as 0 (-600 dB): the
/// effects' states then never go subnormal, which is slow on some CPUs.
const TINY_INPUT: f64 = 1.0e-30;

/// Makes an input sample safe: non-finite and tiny values become 0, huge
/// ones are clamped.
#[inline]
fn sanitize(x: f64) -> f64 {
    if x.is_finite() && x.abs() >= TINY_INPUT { x.clamp(-MAX_INPUT, MAX_INPUT) } else { 0.0 }
}

/// The effects' cost, for the metrics tool (`tools/fxmetrics`), which reads
/// the `COST key value` lines. Ignored: it times; run it in release.
#[cfg(test)]
mod cost {
    use super::{Chorus, Reverb};
    use std::time::Instant;

    /// The best of fifteen, in nanoseconds a frame, of `f` over `frames`
    /// (the best, against other loads and slower cores).
    fn time(frames: usize, mut f: impl FnMut()) -> f64 {
        (0..15)
            .map(|_| {
                let t = Instant::now();
                f();
                t.elapsed().as_secs_f64() * 1e9 / frames as f64
            })
            .fold(f64::MAX, f64::min)
    }

    #[test]
    #[ignore]
    fn report() {
        const BLOCK: usize = 64;
        let n = 44100 * 2;
        let mut s = 0x2545_F491_4F6C_DD1Du64;
        let input: Vec<f32> = (0..n)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                (s >> 40) as f32 / (1u64 << 24) as f32 * 0.2 - 0.1
            })
            .collect();
        let (mut l, mut r) = ([0.0f32; BLOCK], [0.0f32; BLOCK]);
        let mut reverb = Reverb::new(44100.0);
        let ns = time(n, || {
            for c in input.chunks(BLOCK) {
                reverb.process(c, &mut l[..c.len()], &mut r[..c.len()]);
            }
        });
        println!("COST reverb.ns_per_frame {ns:.2}");
        let mut chorus = Chorus::new(44100.0);
        let ns = time(n, || {
            for c in input.chunks(BLOCK) {
                chorus.process(c, &mut l[..c.len()], &mut r[..c.len()]);
            }
        });
        println!("COST chorus.ns_per_frame {ns:.2}");
        // Idle: silent, skipped a block at a time, as the synth does.
        reverb.reset();
        chorus.reset();
        let ns = time(n, || {
            for _ in 0..n / BLOCK {
                reverb.skip(std::hint::black_box(BLOCK));
            }
        });
        println!("COST reverb.idle_ns_per_frame {ns:.3}");
        let ns = time(n, || {
            for _ in 0..n / BLOCK {
                chorus.skip(std::hint::black_box(BLOCK));
            }
        });
        println!("COST chorus.idle_ns_per_frame {ns:.3}");
        std::hint::black_box((&l, &r, &reverb, &chorus));
        for rate in [44100.0, 192000.0] {
            println!("COST reverb.kib_at_{rate} {:.1}", Reverb::new(rate).memory() as f64 / 1024.0);
            println!("COST chorus.kib_at_{rate} {:.1}", Chorus::new(rate).memory() as f64 / 1024.0);
        }
    }
}
