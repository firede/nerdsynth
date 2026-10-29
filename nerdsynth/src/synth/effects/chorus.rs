//! The chorus: three copies of the input read from one delay line, each at
//! a delay swung by its own sine, the three sines a third of a cycle
//! apart, and spread across the stereo field: the first to the right, the
//! second to the centre, the third to the left. No feedback. The delays
//! fall between frames, which are read by four-point cubic (Catmull-Rom)
//! interpolation: flatter than linear, whose loss of highs would move with
//! the delay and be heard as a flutter.
//!
//! The sines are one phase accumulator, an integer that wraps once a cycle,
//! so that it moves on by any number of frames in one step, to the bit; each
//! voice adds its offset to it. The delay over a cycle is tabulated and read
//! by linear interpolation: cheaper than a sine a voice a frame, and within
//! 5e-6 of the swing of a true sine.

use std::f64::consts::TAU;

use super::{Sample, sanitize};

/// The number of voices (FluidSynth's default `synth.chorus.nr`).
const VOICES: usize = 3;

/// The rate of the sines, in Hz (FluidSynth's default `synth.chorus.speed`).
const SPEED: f64 = 0.2;

/// How far the delays swing, peak to peak, in milliseconds (FluidSynth's
/// default `synth.chorus.depth`).
const DEPTH_MS: f64 = 4.25;

/// The delay the voices swing around, in milliseconds: they swing from 1.4
/// to 5.6 ms, the short delays of the classic synthesizer choruses, which
/// thicken without being heard as a slap. FluidSynth's output shows delays
/// of this order too.
const CENTRE_MS: f64 = 3.5;

/// The output level of all the voices together, matched to FluidSynth's
/// chorus: its wet energy on noise (`tools/fxmetrics`).
const LEVEL: f64 = 2.24;

/// How far the voices are spread: their pans are +WIDTH, 0 and -WIDTH.
/// Which side is which is arbitrary; this way round, a note at the very
/// start, before the sines have moved, leans the same way as FluidSynth's
/// (`fx/gu/quiet-send`).
const WIDTH: f64 = 0.8;

/// A cycle of the phase accumulator, 2^64, as a float.
const CYCLE: f64 = 18_446_744_073_709_551_616.0;

/// The delay table has 2^TABLE_BITS points a cycle, and one more to close it.
const TABLE_BITS: u32 = 10;

/// The chorus. See the module documentation.
pub(crate) struct Chorus {
    /// The delay line, a power of two long, longer than the longest delay
    /// and the interpolation's reach.
    line: Box<[f64]>,
    mask: usize,
    /// Where the next frame is written.
    pos: usize,
    /// The phase of the first voice's sine, a cycle being 2^64.
    phase: u64,
    /// The phase step of a frame.
    step: u64,
    /// The delay at each point of a cycle, in frames.
    delays: Box<[f64]>,
    /// Each voice's phase offset.
    offsets: [u64; VOICES],
    /// Each voice's left and right gains.
    gains: [(f64, f64); VOICES],
    /// Consecutive frames whose input was zero, up to the line's length:
    /// there, the line holds only zeros.
    zeros: usize,
}

impl Chorus {
    pub(crate) fn new(sample_rate: f64) -> Chorus {
        let centre = CENTRE_MS * 0.001 * sample_rate;
        let swing = DEPTH_MS * 0.001 * sample_rate / 2.0;
        // The cubic reads a frame beyond the longest delay, and one nearer
        // than the shortest.
        let len = ((centre + swing).ceil() as usize + 3).next_power_of_two();
        let points = 1usize << TABLE_BITS;
        let delays = (0..=points).map(|i| centre + swing * (TAU * i as f64 / points as f64).sin()).collect();
        let offsets = std::array::from_fn(|i| u64::MAX / VOICES as u64 * i as u64);
        let gains = std::array::from_fn(|i| {
            let pan = WIDTH * (1.0 - 2.0 * i as f64 / (VOICES - 1) as f64);
            let gain = LEVEL / VOICES as f64;
            (gain * (1.0 - pan), gain * (1.0 + pan))
        });
        Chorus {
            line: vec![0.0; len].into_boxed_slice(),
            mask: len - 1,
            pos: 0,
            phase: 0,
            step: (SPEED / sample_rate * CYCLE) as u64,
            delays,
            offsets,
            gains,
            zeros: len,
        }
    }

    /// The bytes it holds, itself, its line and its table.
    #[cfg(test)]
    pub(crate) fn memory(&self) -> usize {
        std::mem::size_of::<Self>() + 8 * (self.line.len() + self.delays.len())
    }

    /// Back to the state `new` made.
    pub(crate) fn reset(&mut self) {
        self.clear_lines();
        self.pos = 0;
        self.phase = 0;
    }

    /// Clears the delay line, keeping the sines going.
    pub(crate) fn clear_lines(&mut self) {
        self.line.fill(0.0);
        self.zeros = self.line.len();
    }

    /// Whether the line holds only zeros: no input for longer than its
    /// length.
    pub(crate) fn is_silent(&self) -> bool {
        self.zeros >= self.line.len()
    }

    /// Reads the mono `input` and adds the wet stereo output into `left`
    /// and `right`, over the shortest of the three.
    pub(crate) fn process<S: Sample>(&mut self, input: &[S], left: &mut [S], right: &mut [S]) {
        for ((x, l), r) in input.iter().zip(left.iter_mut()).zip(right.iter_mut()) {
            if let Some((out_l, out_r)) = self.tick(sanitize(x.to_f64())) {
                l.add_f64(out_l);
                r.add_f64(out_r);
            }
        }
    }

    /// Moves on `frames` frames of silent input: once the line holds only
    /// zeros, only the sines move, all at once.
    pub(crate) fn skip(&mut self, frames: usize) {
        let mut left = frames;
        while left > 0 && !self.is_silent() {
            self.tick(0.0);
            left -= 1;
        }
        self.phase = self.phase.wrapping_add(self.step.wrapping_mul(left as u64));
    }

    /// One frame: the output, or `None` when the line holds only zeros.
    #[inline]
    fn tick(&mut self, x: f64) -> Option<(f64, f64)> {
        if x != 0.0 {
            self.zeros = 0;
        } else if self.is_silent() {
            self.phase = self.phase.wrapping_add(self.step);
            return None;
        } else {
            self.zeros += 1;
        }
        if let Some(cell) = self.line.get_mut(self.pos) {
            *cell = x;
        }
        let (mut out_l, mut out_r) = (0.0, 0.0);
        for (&offset, &(gain_l, gain_r)) in self.offsets.iter().zip(&self.gains) {
            let y = self.read(self.delay(self.phase.wrapping_add(offset)));
            out_l += y * gain_l;
            out_r += y * gain_r;
        }
        self.phase = self.phase.wrapping_add(self.step);
        self.pos = (self.pos + 1) & self.mask;
        if !(out_l + out_r).is_finite() {
            // Cannot happen with clamped input; should it, drop the line.
            self.clear_lines();
            return None;
        }
        Some((out_l, out_r))
    }

    /// The delay at a phase, in frames.
    #[inline]
    fn delay(&self, phase: u64) -> f64 {
        let i = (phase >> (64 - TABLE_BITS)) as usize;
        // The bits below the index, as a fraction.
        let t = ((phase << TABLE_BITS) >> 11) as f64 * (1.0 / (1u64 << 53) as f64);
        let a = self.delays.get(i).copied().unwrap_or(0.0);
        let b = self.delays.get(i + 1).copied().unwrap_or(0.0);
        a + (b - a) * t
    }

    /// The line `delay` frames back from the frame just written,
    /// interpolated; the delay is kept within the line.
    #[inline]
    fn read(&self, delay: f64) -> f64 {
        let delay = delay.clamp(1.0, (self.line.len() - 3) as f64);
        let whole = delay as usize;
        let t = delay - whole as f64;
        let at = |back: usize| self.line.get(self.pos.wrapping_sub(back) & self.mask).copied().unwrap_or(0.0);
        // Frames one nearer, the two either side, and one farther.
        let (y0, y1, y2, y3) = (at(whole - 1), at(whole), at(whole + 1), at(whole + 2));
        let c1 = 0.5 * (y2 - y0);
        let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
        let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
        ((c3 * t + c2) * t + c1) * t + y1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATES: [f64; 6] = [8000.0, 22050.0, 44100.0, 48000.0, 96000.0, 192000.0];

    /// Deterministic noise in -1..1.
    fn noise(n: usize, seed: u64) -> Vec<f64> {
        let mut s = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        (0..n)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                (s >> 11) as f64 / (1u64 << 52) as f64 - 1.0
            })
            .collect()
    }

    fn sine(rate: f64, hz: f64, n: usize) -> Vec<f64> {
        (0..n).map(|i| (TAU * hz * i as f64 / rate).sin()).collect()
    }

    fn process(c: &mut Chorus, input: &[f64]) -> (Vec<f64>, Vec<f64>) {
        let (mut l, mut r) = (vec![0.0; input.len()], vec![0.0; input.len()]);
        c.process(input, &mut l, &mut r);
        (l, r)
    }

    /// The output of a fresh chorus for `input`, in calls of the lengths
    /// `split` gives, in turn.
    fn run<S: Sample + Default + From<f32>>(rate: f64, input: &[f64], split: &[usize]) -> (Vec<S>, Vec<S>) {
        let input: Vec<S> = input.iter().map(|&x| S::from(x as f32)).collect();
        let mut c = Chorus::new(rate);
        let (mut l, mut r) = (vec![S::default(); input.len()], vec![S::default(); input.len()]);
        let mut at = 0;
        for &len in split.iter().cycle() {
            if at >= input.len() {
                break;
            }
            let end = (at + len).min(input.len());
            c.process(&input[at..end], &mut l[at..end], &mut r[at..end]);
            at = end;
        }
        (l, r)
    }

    fn energy(x: &[f64]) -> f64 {
        x.iter().map(|v| v * v).sum()
    }

    #[test]
    fn impulse_response_is_three_taps_and_ends() {
        for rate in RATES {
            let mut c = Chorus::new(rate);
            assert!(c.is_silent());
            let n = (rate * 0.02) as usize;
            let mut input = vec![0.0; n];
            input[0] = 1.0;
            let (l, r) = process(&mut c, &input);
            assert!(l.iter().chain(&r).all(|x| x.is_finite()));
            // Nothing before the shortest delay or after the longest.
            let ms = |t: f64| (t * 0.001 * rate) as usize;
            assert!(l[..ms(1.3)].iter().chain(&l[ms(5.7)..]).all(|x| *x == 0.0), "{rate}");
            assert!(energy(&l) > 0.1 && energy(&r) > 0.1, "{rate}");
            // Silent once the line has been written through.
            let (mut a, mut b) = ([0.0f64; 1], [0.0f64; 1]);
            let mut frames = 0;
            while !c.is_silent() {
                c.process(&[0.0], &mut a, &mut b);
                frames += 1;
            }
            assert!(frames <= c.line.len(), "{rate}");
            assert!(c.line.iter().all(|x| *x == 0.0));
        }
    }

    #[test]
    fn a_sine_is_modulated() {
        let rate = 44100.0;
        let n = 44100 * 5;
        let input = sine(rate, 1000.0, n);
        let (l, r) = process(&mut Chorus::new(rate), &input);
        // The voices drift in and out of phase with each other: the level of
        // the wet signal swings.
        let block = 441;
        let levels: Vec<f64> = l[block..].chunks(block).map(|b| 10.0 * (energy(b) / block as f64).log10()).collect();
        let (lo, hi) = levels.iter().fold((f64::MAX, f64::MIN), |(lo, hi), &v| (lo.min(v), hi.max(v)));
        assert!(hi - lo > 6.0, "{lo} {hi}");
        // And it is not the input, delayed: the zero crossings move.
        let crossings = |x: &[f64]| x.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        assert_ne!(crossings(&l[..n / 2]), crossings(&input[..n / 2]));
        assert!(l != r);
        assert!(l.iter().chain(&r).all(|x| x.abs() < 3.0));
    }

    #[test]
    fn channels_differ() {
        let input = noise(44100, 2);
        let (l, r) = process(&mut Chorus::new(44100.0), &input);
        let (el, er) = (energy(&l), energy(&r));
        let cross: f64 = l.iter().zip(&r).map(|(a, b)| a * b).sum();
        assert!((el / er).log10().abs() < 0.1, "{el} {er}");
        assert!(cross / (el * er).sqrt() < 0.9, "{}", cross / (el * er).sqrt());
    }

    #[test]
    fn split_calls_are_identical() {
        let mut input = noise(30000, 3);
        input.extend(std::iter::repeat_n(0.0, 20000));
        input.extend(sine(22050.0, 300.0, 30000));
        let whole: (Vec<f64>, Vec<f64>) = run(22050.0, &input, &[usize::MAX]);
        for split in [&[1usize][..], &[7], &[64, 1, 333], &[4096, 5]] {
            assert!(run::<f64>(22050.0, &input, split) == whole, "{split:?}");
        }
        let whole: (Vec<f32>, Vec<f32>) = run(22050.0, &input, &[usize::MAX]);
        for split in [&[1usize][..], &[13, 100], &[64]] {
            assert!(run::<f32>(22050.0, &input, split) == whole, "f32 {split:?}");
        }
    }

    #[test]
    fn reset_is_as_new() {
        let input = noise(5000, 4);
        let mut c = Chorus::new(44100.0);
        process(&mut c, &input);
        c.reset();
        assert!(c.is_silent());
        assert!(process(&mut c, &input) == process(&mut Chorus::new(44100.0), &input));
    }

    #[test]
    fn clear_lines_keeps_the_sines() {
        let input = noise(5000, 5);
        let mut c = Chorus::new(48000.0);
        process(&mut c, &input);
        c.clear_lines();
        assert!(c.is_silent());
        // As a fresh one whose sines have moved on as far.
        let mut fresh = Chorus::new(48000.0);
        fresh.skip(5000);
        assert!(process(&mut c, &input) == process(&mut fresh, &input));
    }

    #[test]
    fn hostile_input_stays_finite() {
        let mut hostile: Vec<f32> =
            [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 1e30, -1e30, f32::MAX, f32::MIN, 1e-40, -0.0]
                .iter()
                .copied()
                .cycle()
                .take(9000)
                .collect();
        hostile.extend((0..20000).map(|i| if i % 2 == 0 { 1e30 } else { -1e30 }));
        for rate in RATES {
            let mut c = Chorus::new(rate);
            let (mut l, mut r) = (vec![0.0f32; hostile.len()], vec![0.0f32; hostile.len()]);
            c.process(&hostile, &mut l, &mut r);
            assert!(l.iter().chain(&r).all(|x| x.is_finite()), "{rate}");
            c.skip(1 << 16);
            assert!(c.is_silent());
        }
        // Tiny values are flushed: no state ever goes subnormal.
        let mut c = Chorus::new(44100.0);
        let tiny = vec![1e-35f64; 1000];
        process(&mut c, &tiny);
        assert!(c.is_silent());
    }

    #[test]
    fn skip_is_processing_silence() {
        let input = noise(2000, 6);
        let probe = noise(3000, 7);
        for frames in [0, 1, 100, 5000, 1 << 20] {
            let mut a = Chorus::new(96000.0);
            let mut b = Chorus::new(96000.0);
            process(&mut a, &input);
            process(&mut b, &input);
            // Not yet silent: skip processes the zeros up to silence itself.
            a.skip(frames);
            process(&mut b, &vec![0.0; frames]);
            assert_eq!(a.is_silent(), b.is_silent());
            assert!(process(&mut a, &probe) == process(&mut b, &probe), "{frames}");
        }
    }
}
