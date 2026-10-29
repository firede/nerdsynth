//! The reverb: a feedback delay network (Jot and Chaigne, 1991), fed through
//! a pre-delay and a chain of Schroeder all-passes.
//!
//! Eight delay lines of mutually prime lengths, 46 to 188 ms, feed back
//! through an 8×8 Hadamard matrix, which is orthogonal (lossless) and costs
//! 24 additions by the fast Walsh–Hadamard transform. Each line's output
//! goes through a first-order shelving filter whose gains at DC, at 8 kHz
//! and at the top of the band are set from its length, so that every line,
//! and so the whole tail, falls 60 dB in the same time at each frequency:
//! no line rings on after the others, and the decay time can be set per
//! frequency (Jot's absorption filters). A shelf rather than a low-pass
//! keeps the top octave decaying at a set rate instead of dying at once,
//! which would dull the tail. The all-passes before the network spread the
//! input into many echoes at once, so that the tail is dense from its
//! start. The two outputs take the lines with two orthogonal sign
//! patterns: a diffuse tail comes out of them uncorrelated.
//!
//! Delays are set in milliseconds and scaled to the sample rate, so that
//! the sound does not depend on it.
//!
//! The lines' lengths swing slowly, each by its own sine (0.3 ms either
//! way, at most 3 cents of pitch, different in every line): the network's
//! modes wander a little and blur into each other instead of ringing at
//! fixed frequencies. More swing would even out the tail further but be
//! heard as a detuned, chorused tail; FluidSynth's tail is about as
//! narrow about a tone as this one. The lines are read between frames
//! through first-order all-passes, which lose no highs however many times
//! the sound goes round (a polynomial interpolation would, by up to 2 dB a
//! pass at 16 kHz). The sines are integer phase accumulators, which move
//! on by any number of frames in one step; the delays follow them along
//! straight segments.
//!
//! The design and its constants were tuned against FluidSynth's rendered
//! reverb by the metrics in `tools/fxmetrics`: echo density, coloration,
//! smoothness of the decay and stereo decorrelation no worse than
//! FluidSynth's, and decay times, onset, level and brightness near it.
//!
//! Silence is detected inside the network: once the input and every line's
//! output have stayed below [`SILENT_LEVEL`] (at the output's scale) for
//! longer than the longest way through, what is left is below it too, and
//! is dropped.

use super::{SILENT_LEVEL, Sample, sanitize};

/// The lines, in milliseconds (prime numbers of frames at 44.1 kHz). They
/// are long, a second in all, for many modes close together: the fewer
/// and wider apart a network's modes, the more some notes' tails ring out
/// over others'. The first is short, for an early first echo.
const LINES_MS: [f64; 8] = [46.01, 96.58, 113.58, 130.36, 144.51, 158.34, 170.91, 187.6];

/// The all-passes before the network: delays in milliseconds and gains.
const DIFFUSERS_MS: [f64; 4] = [3.13, 4.31, 7.91, 11.29];
const DIFFUSER_GAINS: [f64; 4] = [0.6, 0.6, 0.5, 0.5];

/// How far each line's length swings either way, in milliseconds, and the
/// rates of the swings, in Hz (none a simple ratio of another).
const SWING_MS: f64 = 0.3;
const SWING_HZ: [f64; 8] = [0.31, 0.43, 0.53, 0.61, 0.71, 0.79, 0.89, 0.97];

/// A cycle of a phase accumulator, 2^64, as a float.
const CYCLE: f64 = 18_446_744_073_709_551_616.0;

/// The sine table has 2^TABLE_BITS points a cycle, and one more to close it.
const TABLE_BITS: u32 = 9;

/// The delays move along straight segments of this many frames between
/// points of their sines: the sine is looked up once a segment, and the
/// chords miss it by under 0.001 frames.
const SEGMENT: usize = 32;

/// The pre-delay, in milliseconds: the first echo comes this much later
/// than the shortest line (about 59 ms after the dry sound, as
/// FluidSynth's at 44.1 kHz).
const PREDELAY_MS: f64 = 13.0;

/// The time the tail takes to fall by 60 dB at low frequencies, at
/// [`HIGH_HZ`], and at the top of the band, in seconds: after FluidSynth's,
/// measured (`tools/fxmetrics`).
const DECAY_LOW: f64 = 1.72;
const DECAY_HIGH: f64 = 1.29;
const HIGH_HZ: f64 = 8000.0;
const DECAY_TOP: f64 = 1.0;

/// The output gain, matched to FluidSynth's wet level.
const LEVEL: f64 = 0.46;

const N: usize = 8;

/// The output sign patterns: rows of the Hadamard matrix, orthogonal to
/// each other.
const LEFT: [f64; N] = [1.0, 1.0, 1.0, 1.0, -1.0, -1.0, -1.0, -1.0];
const RIGHT: [f64; N] = [1.0, -1.0, 1.0, -1.0, 1.0, -1.0, 1.0, -1.0];
/// The input's signs into the lines.
const INPUT: [f64; N] = [1.0, 1.0, -1.0, -1.0, 1.0, 1.0, -1.0, -1.0];

/// A circular delay line of a fixed length (at least one frame).
struct Line {
    buf: Box<[f64]>,
    /// The next frame to read, then write.
    pos: usize,
}

impl Line {
    fn new(len: usize) -> Line {
        Line { buf: vec![0.0; len.max(1)].into_boxed_slice(), pos: 0 }
    }

    /// The frame written a line's length ago.
    #[inline]
    fn read(&self) -> f64 {
        self.buf.get(self.pos).copied().unwrap_or(0.0)
    }

    /// Writes over the frame [`Line::read`] gave, and moves on.
    #[inline]
    fn write(&mut self, x: f64) {
        if let Some(cell) = self.buf.get_mut(self.pos) {
            *cell = x;
        }
        self.pos += 1;
        if self.pos >= self.buf.len() {
            self.pos = 0;
        }
    }

    fn clear(&mut self) {
        self.buf.fill(0.0);
        self.pos = 0;
    }
}

/// A delay line read at a swinging delay.
struct Swung {
    buf: Box<[f64]>,
    /// Where the next frame is written.
    pos: usize,
    /// The delay's centre, in frames.
    centre: f64,
    /// The phase of its sine, a cycle being 2^64, and its step a frame.
    phase: u64,
    step: u64,
    /// The interpolating all-pass's last output.
    last: f64,
    /// The delay at the start of the current segment, and its change a
    /// frame.
    start: f64,
    slope: f64,
}

impl Swung {
    fn new(centre: f64, swing: f64, rate: f64, hz: f64, phase: u64) -> Swung {
        // The all-pass reads a frame farther than the delay.
        let len = ((centre + swing).ceil() as usize + 3).max(SEGMENT + 4);
        Swung {
            buf: vec![0.0; len].into_boxed_slice(),
            pos: 0,
            centre,
            phase,
            step: (hz / rate * CYCLE) as u64,
            last: 0.0,
            start: centre,
            slope: 0.0,
        }
    }

    /// The frame `delay` frames back from `ahead` frames on (the delay at
    /// least `SEGMENT` + 1.5, so that it reads no frame written since,
    /// and at most the length less 2), through a first-order all-pass for
    /// the fraction, whose gain is 1 at every frequency: no highs are lost
    /// in the loop, unlike with any polynomial interpolation. The fraction
    /// is kept within 0.5 to 1.5, where the all-pass's delay is flattest;
    /// at high frequencies its delay still strays from the one asked as the
    /// fraction moves (a slight extra swing of their phase, within a tail
    /// already swinging). As the delay passes a half frame, the taps move
    /// by one and the coefficient jumps; the state carries over.
    #[inline]
    fn read(&mut self, ahead: usize, delay: f64) -> f64 {
        let len = self.buf.len();
        let delay = delay.max(SEGMENT as f64 + 1.5).min(len as f64 - 2.0);
        let whole = (delay - 0.5) as usize;
        // The coefficient for the fraction 1 + d is -d / (2 + d); its series
        // to d⁴ is within 0.001 of it, and any coefficient below 1 makes an
        // all-pass, so the error only moves the delay, by a hair.
        let d = delay - whole as f64 - 1.0;
        let h = -0.5 * d;
        let eta = h * (1.0 + h * (1.0 + h * (1.0 + h)));
        // Ahead is under the delay, so one wrap at most.
        let at = |back: usize| {
            let i = self.pos + ahead + len - back;
            self.buf.get(if i >= len { i - len } else { i }).copied().unwrap_or(0.0)
        };
        let y = eta * (at(whole) - self.last) + at(whole + 1);
        self.last = y;
        y
    }

    #[inline]
    fn write(&mut self, x: f64) {
        if let Some(cell) = self.buf.get_mut(self.pos) {
            *cell = x;
        }
        self.pos += 1;
        if self.pos >= self.buf.len() {
            self.pos = 0;
        }
    }
}

/// A Schroeder all-pass.
struct Allpass {
    line: Line,
    gain: f64,
}

impl Allpass {
    #[inline]
    fn tick(&mut self, x: f64) -> f64 {
        let delayed = self.line.read();
        let u = x + self.gain * delayed;
        self.line.write(u);
        delayed - self.gain * u
    }
}

/// The absorption filter of a line, a first-order shelf:
/// `y = b0·x + b1·x' − a1·y'`.
#[derive(Clone, Copy, Default)]
struct Absorb {
    b0: f64,
    b1: f64,
    a1: f64,
}

impl Absorb {
    /// The filter of a line of `frames`: falling 60 dB in [`DECAY_LOW`] at
    /// DC, in [`DECAY_HIGH`] at [`HIGH_HZ`] (or the highest frequency the
    /// rate allows, its decay time read off a line from 2 kHz), and in
    /// [`DECAY_TOP`] at the top of the band.
    fn new(frames: usize, rate: f64) -> Absorb {
        let gain = |t: f64| 10f64.powf(-3.0 * frames as f64 / (t * rate));
        let hz = HIGH_HZ.min(0.35 * rate);
        let t = DECAY_LOW + (DECAY_HIGH - DECAY_LOW) * ((hz - 2000.0) / (HIGH_HZ - 2000.0)).clamp(0.0, 1.0);
        let (dc, mid, top) = (gain(DECAY_LOW), gain(t), gain(DECAY_TOP));
        // An analog shelf g·(s + z)/(s + p), its frequencies prewarped for
        // the bilinear transform (which maps infinity to the top of the
        // band): |H|² = g²·(w² + z²)/(w² + p²), with g the top's gain and
        // g·z/p DC's; p is solved for the middle gain at w.
        let w = (std::f64::consts::PI * hz / rate).tan();
        let p = w * ((top * top - mid * mid) / (mid * mid - dc * dc)).sqrt();
        let z = p * dc / top;
        Absorb { b0: top * (1.0 + z) / (1.0 + p), b1: top * (z - 1.0) / (1.0 + p), a1: (p - 1.0) / (1.0 + p) }
    }
}

/// The fast Walsh–Hadamard transform of 8, scaled to be orthogonal.
#[inline]
fn hadamard(v: &mut [f64; N]) {
    let mut h = 1;
    while h < N {
        let mut i = 0;
        while i < N {
            for j in i..i + h {
                let (a, b) = (v[j], v[j + h]);
                v[j] = a + b;
                v[j + h] = a - b;
            }
            i += 2 * h;
        }
        h *= 2;
    }
    const SCALE: f64 = 0.353_553_390_593_273_8; // 1/√8
    for x in v.iter_mut() {
        *x *= SCALE;
    }
}

/// The reverb. See the module documentation.
pub(crate) struct Reverb {
    predelay: Line,
    diffusers: [Allpass; 4],
    lines: [Swung; N],
    /// A cycle of a sine, in frames of swing.
    sine: Box<[f64]>,
    absorb: [Absorb; N],
    /// The absorption filters' states: the lines' last outputs, before and
    /// after them.
    outs: [f64; N],
    stores: [f64; N],
    /// Consecutive frames with the input and the lines below the level.
    quiet: usize,
    /// After this many quiet frames, all of it is below the level, and the
    /// lines are cleared.
    window: usize,
    /// Frames into the current segment of the delays.
    offset: usize,
    /// The lines are all zeros.
    silent: bool,
}

/// The swing at a phase, in frames, off the table of a cycle.
#[inline]
fn swing(sine: &[f64], phase: u64) -> f64 {
    let i = (phase >> (64 - TABLE_BITS)) as usize;
    let t = ((phase << TABLE_BITS) >> 11) as f64 * (1.0 / (1u64 << 53) as f64);
    let a = sine.get(i).copied().unwrap_or(0.0);
    let b = sine.get(i + 1).copied().unwrap_or(0.0);
    a + (b - a) * t
}

impl Reverb {
    pub(crate) fn new(sample_rate: f64) -> Reverb {
        let frames = |ms: f64| ((ms * 0.001 * sample_rate).round() as usize).max(1);
        let swing = SWING_MS * 0.001 * sample_rate;
        let lines: [Swung; N] = std::array::from_fn(|i| {
            let phase = u64::MAX / N as u64 * i as u64;
            Swung::new(frames(LINES_MS[i]) as f64, swing, sample_rate, SWING_HZ[i], phase)
        });
        let points = 1usize << TABLE_BITS;
        let sine = (0..=points).map(|i| swing * (std::f64::consts::TAU * i as f64 / points as f64).sin()).collect();
        let absorb = std::array::from_fn(|i| Absorb::new(lines[i].centre as usize, sample_rate));
        let mut i = 0;
        let diffusers = DIFFUSERS_MS.map(|ms| {
            let gain = DIFFUSER_GAINS[i];
            i += 1;
            Allpass { line: Line::new(frames(ms)), gain }
        });
        let predelay = Line::new(frames(PREDELAY_MS));
        let longest = lines.iter().map(|l| l.buf.len()).max().unwrap_or(0);
        let window = predelay.buf.len() + diffusers.iter().map(|d| d.line.buf.len()).sum::<usize>() + longest + 1;
        Reverb {
            predelay,
            diffusers,
            lines,
            sine,
            absorb,
            outs: [0.0; N],
            stores: [0.0; N],
            quiet: 0,
            window,
            offset: 0,
            silent: true,
        }
    }

    /// The bytes it holds, itself and its lines.
    #[cfg(test)]
    pub(crate) fn memory(&self) -> usize {
        let frames = self.predelay.buf.len()
            + self.diffusers.iter().map(|d| d.line.buf.len()).sum::<usize>()
            + self.lines.iter().map(|l| l.buf.len()).sum::<usize>()
            + self.sine.len();
        std::mem::size_of::<Self>() + 8 * frames
    }

    /// Back to the state `new` made.
    pub(crate) fn reset(&mut self) {
        self.clear_lines();
        for (i, l) in self.lines.iter_mut().enumerate() {
            l.pos = 0;
            l.phase = u64::MAX / N as u64 * i as u64;
        }
        self.offset = 0;
    }

    /// Clears the tail, keeping the swings going.
    pub(crate) fn clear_lines(&mut self) {
        self.predelay.clear();
        for d in &mut self.diffusers {
            d.line.clear();
        }
        for l in &mut self.lines {
            l.buf.fill(0.0);
        }
        for l in &mut self.lines {
            l.last = 0.0;
        }
        self.outs = [0.0; N];
        self.stores = [0.0; N];
        self.quiet = 0;
        self.silent = true;
    }

    /// Whether the lines are all zeros: the tail has died away below
    /// [`SILENT_LEVEL`] and no input has come since.
    pub(crate) fn is_silent(&self) -> bool {
        self.silent
    }

    /// Reads the mono `input` and adds the wet stereo output into `left`
    /// and `right`, over the shortest of the three.
    pub(crate) fn process<S: Sample>(&mut self, input: &[S], left: &mut [S], right: &mut [S]) {
        let n = input.len().min(left.len()).min(right.len());
        let mut x = [0.0; SEGMENT];
        let mut out = [(0.0, 0.0); SEGMENT];
        let mut at = 0;
        while at < n {
            if self.silent && sanitize(input[at].to_f64()) == 0.0 {
                self.advance();
                at += 1;
                continue;
            }
            let len = (n - at).min(SEGMENT - self.offset);
            for (x, s) in x.iter_mut().zip(&input[at..at + len]) {
                *x = sanitize(s.to_f64());
            }
            let (done, last) = self.block(&x[..len], &mut out[..len]);
            let outputs = if last { done } else { done - 1 };
            for (k, &(l, r)) in out[..outputs].iter().enumerate() {
                left[at + k].add_f64(l);
                right[at + k].add_f64(r);
            }
            at += done;
        }
    }

    /// Moves on `frames` frames of silent input: once the lines hold only
    /// zeros, only the swings move, all at once.
    pub(crate) fn skip(&mut self, frames: usize) {
        let zeros = [0.0; SEGMENT];
        let mut out = [(0.0, 0.0); SEGMENT];
        let mut left = frames;
        while left > 0 && !self.silent {
            let len = left.min(SEGMENT - self.offset);
            left -= self.block(&zeros[..len], &mut out[..len]).0;
        }
        for l in &mut self.lines {
            l.phase = l.phase.wrapping_add(l.step.wrapping_mul(left as u64));
        }
        self.offset = (self.offset + left % SEGMENT) % SEGMENT;
    }

    /// Sets the delays' current segments from their phases (which have
    /// moved `offset` frames into them): the same at the segment's start
    /// as on any frame after, so that waking from silence finds them as
    /// processing would have.
    fn segments(&mut self) {
        for l in &mut self.lines {
            let first = l.phase.wrapping_sub(l.step.wrapping_mul(self.offset as u64));
            let a = l.centre + swing(&self.sine, first);
            let b = l.centre + swing(&self.sine, first.wrapping_add(l.step.wrapping_mul(SEGMENT as u64)));
            l.start = a;
            l.slope = (b - a) * (1.0 / SEGMENT as f64);
        }
    }

    /// One frame on for the swings.
    #[inline]
    fn advance(&mut self) {
        for l in &mut self.lines {
            l.phase = l.phase.wrapping_add(l.step);
        }
        self.offset += 1;
        if self.offset == SEGMENT {
            self.offset = 0;
            if !self.silent {
                self.segments();
            }
        }
    }

    /// Processes the frames `x` (sanitized, the first not zero if silent,
    /// within one segment), its output into `out`. Every line is read
    /// further back than the segment is long, so each line's reads for the
    /// whole of it are made at once, in a tight loop; then the frames go
    /// through the matrix one by one. The arithmetic of each frame is the
    /// same however the frames are grouped.
    ///
    /// Returns how many frames it took (fewer when the lines were cleared,
    /// whose reads ahead are then stale), and whether the last of them has
    /// an output.
    fn block(&mut self, x: &[f64], out: &mut [(f64, f64)]) -> (usize, bool) {
        let len = x.len().min(out.len()).min(SEGMENT - self.offset);
        if self.silent {
            self.silent = false;
            self.segments();
        }
        // The way in: the pre-delay and the all-passes.
        let mut y = [0.0; SEGMENT];
        for (y, &x) in y.iter_mut().zip(&x[..len]) {
            let mut v = self.predelay.read();
            self.predelay.write(x);
            for d in &mut self.diffusers {
                v = d.tick(v);
            }
            *y = v;
        }
        // The lines' outputs, through their absorption filters.
        let mut s = [[0.0; SEGMENT]; N];
        for (i, s) in s.iter_mut().enumerate() {
            let line = &mut self.lines[i];
            let f = self.absorb[i];
            let (mut o, mut st) = (self.outs[i], self.stores[i]);
            for (k, s) in s[..len].iter_mut().enumerate() {
                let out = line.read(k, line.start + line.slope * (self.offset + k) as f64);
                st = f.b0 * out + f.b1 * o - f.a1 * st;
                o = out;
                *s = st;
            }
            self.outs[i] = o;
            self.stores[i] = st;
        }
        for k in 0..len {
            let mut v: [f64; N] = std::array::from_fn(|i| s[i][k]);
            let mut energy = 0.0;
            let (mut out_l, mut out_r) = (0.0, 0.0);
            for i in 0..N {
                energy += v[i] * v[i];
                out_l += LEFT[i] * v[i];
                out_r += RIGHT[i] * v[i];
            }
            hadamard(&mut v);
            for (i, line) in self.lines.iter_mut().enumerate() {
                line.write(v[i] + INPUT[i] * y[k]);
            }
            self.advance();
            let (out_l, out_r) = (out_l * LEVEL, out_r * LEVEL);
            // Cannot happen with clamped input and a stable loop; should it,
            // the state is lost anyway.
            if !(out_l + out_r).is_finite() {
                self.clear_lines();
                return (k + 1, false);
            }
            out[k] = (out_l, out_r);
            // Each output is at most √8·LEVEL times the lines' magnitude.
            if x[k].abs() > SILENT_LEVEL || energy * (N as f64 * LEVEL * LEVEL) > SILENT_LEVEL * SILENT_LEVEL {
                self.quiet = 0;
            } else {
                self.quiet += 1;
                if self.quiet >= self.window {
                    // The input has passed through, and every line has been
                    // read through, below the level: drop what is left,
                    // before it can go subnormal.
                    self.clear_lines();
                    return (k + 1, true);
                }
            }
        }
        (len, true)
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

    /// Noise bursts with silence between, `seconds` long.
    fn bursts(rate: f64, seconds: f64) -> Vec<f64> {
        let n = (rate * seconds) as usize;
        let mut x = noise(n, 7);
        for (i, v) in x.iter_mut().enumerate() {
            if (i as f64 / rate) % 2.0 > 0.1 {
                *v = 0.0;
            }
        }
        x
    }

    /// The output of a fresh reverb for `input`, processed in calls of the
    /// lengths `split` gives, in turn.
    fn run<S: Sample + Default + From<f32>>(rate: f64, input: &[f64], split: &[usize]) -> (Vec<S>, Vec<S>) {
        let input: Vec<S> = input.iter().map(|&x| S::from(x as f32)).collect();
        let mut rv = Reverb::new(rate);
        let (mut l, mut r) = (vec![S::default(); input.len()], vec![S::default(); input.len()]);
        let mut at = 0;
        for &len in split.iter().cycle() {
            if at >= input.len() {
                break;
            }
            let end = (at + len).min(input.len());
            rv.process(&input[at..end], &mut l[at..end], &mut r[at..end]);
            at = end;
        }
        (l, r)
    }

    fn energy(x: &[f64]) -> f64 {
        x.iter().map(|v| v * v).sum()
    }

    /// Frames of zero input until silent, at most `limit`.
    fn frames_to_silence(rv: &mut Reverb, limit: usize) -> Option<usize> {
        let zeros = [0.0f64; 64];
        let (mut l, mut r) = ([0.0f64; 64], [0.0f64; 64]);
        let mut n = 0;
        while !rv.is_silent() {
            if n >= limit {
                return None;
            }
            rv.process(&zeros, &mut l, &mut r);
            n += 64;
        }
        Some(n)
    }

    #[test]
    fn impulse_response_decays_and_ends() {
        for rate in RATES {
            let n = (rate * 4.0) as usize;
            let mut input = vec![0.0; n];
            input[0] = 1.0;
            let mut rv = Reverb::new(rate);
            assert!(rv.is_silent());
            let (mut l, mut r) = (vec![0.0f64; n], vec![0.0f64; n]);
            let half = n / 8;
            rv.process(&input[..half], &mut l[..half], &mut r[..half]);
            assert!(!rv.is_silent());
            rv.process(&input[half..], &mut l[half..], &mut r[half..]);
            assert!(l.iter().chain(&r).all(|x| x.is_finite()), "{rate}");
            // Nothing before the pre-delay and the shortest line (less its
            // swing), then a tail falling by about 60 dB in DECAY_LOW.
            let first = ((PREDELAY_MS + LINES_MS[0] - SWING_MS) * 0.001 * rate) as usize;
            assert!(l[..first - 1].iter().all(|x| *x == 0.0), "{rate}");
            let second = rate as usize;
            let e = |k: usize| energy(&l[k * second / 2..(k + 1) * second / 2]);
            assert!(e(0) > 1e-4, "{rate}: {}", e(0));
            for k in 1..5 {
                let drop = 10.0 * (e(k - 1) / e(k)).log10();
                assert!((8.0..30.0).contains(&drop), "{rate}: {drop} dB in half second {k}");
            }
            // Silent within 6 s of the impulse.
            let frames = frames_to_silence(&mut rv, (rate * 10.0) as usize).expect("never silent");
            assert!(frames < (rate * 2.0) as usize, "{rate}: {frames}");
        }
    }

    /// The gain of `absorb` at `hz`.
    fn response(f: Absorb, hz: f64, rate: f64) -> f64 {
        let w = std::f64::consts::TAU * hz / rate;
        let (re_n, im_n) = (f.b0 + f.b1 * w.cos(), -f.b1 * w.sin());
        let (re_d, im_d) = (1.0 + f.a1 * w.cos(), -f.a1 * w.sin());
        ((re_n * re_n + im_n * im_n) / (re_d * re_d + im_d * im_d)).sqrt()
    }

    #[test]
    fn every_line_decays_alike() {
        // Per line, the loss a pass is that of the set decay time over the
        // line's length: at DC, at 8 kHz and at the top of the band.
        for rate in [44100.0, 48000.0, 96000.0] {
            let rv = Reverb::new(rate);
            for (line, &f) in rv.lines.iter().zip(&rv.absorb) {
                let seconds = |gain: f64| -3.0 * line.centre / rate / gain.log10();
                for (hz, t) in [(0.0, DECAY_LOW), (HIGH_HZ, DECAY_HIGH), (rate / 2.0, DECAY_TOP)] {
                    let got = seconds(response(f, hz, rate));
                    assert!((got - t).abs() < 1e-6, "{rate} Hz, {} frames, at {hz} Hz: {got} s", line.centre);
                }
                // Monotonic between: the decay time falls with the frequency.
                let mut last = f64::INFINITY;
                for k in 0..100 {
                    let t = seconds(response(f, k as f64 * rate / 200.0, rate));
                    assert!(t <= last + 1e-9);
                    last = t;
                }
            }
        }
        // Low rates keep the curve, up to what they can hold.
        let rv = Reverb::new(8000.0);
        assert!(rv.absorb.iter().all(|f| response(*f, 0.0, 8000.0) < 1.0 && response(*f, 4000.0, 8000.0) > 0.0));
    }

    #[test]
    fn swung_reads_lose_no_highs() {
        // A 16 kHz sine through a line held at a fractional delay comes out
        // at its level, and on time.
        let rate = 44100.0;
        for delay in [40.0, 40.25, 40.5, 40.75, 41.4] {
            let mut line = Swung::new(60.0, 0.0, rate, 1.0, 0);
            let w = std::f64::consts::TAU * 16000.0 / rate;
            let (mut ein, mut eout) = (0.0, 0.0);
            for n in 0..20000 {
                let y = line.read(0, delay);
                line.write((w * n as f64).sin());
                if n > 1000 {
                    ein += (w * n as f64).sin().powi(2);
                    eout += y * y;
                }
            }
            let gain = 10.0 * (eout / ein).log10();
            assert!(gain.abs() < 0.01, "{delay}: {gain} dB");
            // At low frequencies the delay is the one asked for.
            let mut line = Swung::new(60.0, 0.0, rate, 1.0, 0);
            let w = std::f64::consts::TAU * 50.0 / rate;
            let mut err = 0.0f64;
            for n in 0..5000 {
                let y = line.read(0, delay);
                line.write((w * n as f64).sin());
                if n > 1000 {
                    err = err.max((y - (w * (n as f64 - delay)).sin()).abs());
                }
            }
            assert!(err < 1e-3, "{delay}: {err}");
        }
    }

    #[test]
    fn silent_means_zeros() {
        let mut rv = Reverb::new(44100.0);
        let input = noise(4410, 1);
        let (mut l, mut r) = (vec![0.0; 4410], vec![0.0; 4410]);
        rv.process(&input, &mut l, &mut r);
        frames_to_silence(&mut rv, 44100 * 10).expect("never silent");
        assert!(rv.stores.iter().chain(&rv.outs).all(|x| *x == 0.0));
        assert!(rv.lines.iter().all(|l| l.last == 0.0 && l.buf.iter().all(|x| *x == 0.0)));
        assert!(rv.diffusers.iter().map(|d| &d.line).chain([&rv.predelay]).all(|l| l.buf.iter().all(|x| *x == 0.0)));
        let (mut l, mut r) = (vec![0.0f64; 1000], vec![0.0f64; 1000]);
        rv.process(&[0.0; 1000], &mut l, &mut r);
        assert!(l.iter().chain(&r).all(|x| *x == 0.0));
        assert!(rv.is_silent());
    }

    #[test]
    fn delays_follow_the_sines() {
        // Frame by frame, each line's delay is within a thousandth of a
        // frame of its centre plus its sine, and swings the whole way.
        let rate = 44100.0;
        let mut rv = Reverb::new(rate);
        let swing = SWING_MS * 0.001 * rate;
        let (mut lo, mut hi) = ([f64::MAX; N], [f64::MIN; N]);
        let (mut l, mut r) = ([0.0f64; 1], [0.0f64; 1]);
        for n in 0..(rate * 3.3) as usize {
            // A little input now and then keeps it awake.
            rv.process(&[if n % 1000 == 0 { 0.5 } else { 0.0 }], &mut l, &mut r);
            if rv.offset == 0 {
                continue;
            }
            for (i, line) in rv.lines.iter().enumerate() {
                let delay = line.start + line.slope * rv.offset as f64;
                let exact = line.centre + swing * (line.phase as f64 / CYCLE * std::f64::consts::TAU).sin();
                assert!((delay - exact).abs() < 1e-3, "line {i} at {n}: {delay} for {exact}");
                lo[i] = lo[i].min(delay - line.centre);
                hi[i] = hi[i].max(delay - line.centre);
            }
        }
        for i in 0..N {
            assert!(lo[i] < -0.99 * swing && hi[i] > 0.99 * swing, "line {i}: {} to {}", lo[i], hi[i]);
        }
    }

    #[test]
    fn split_calls_are_identical() {
        let input = bursts(8000.0, 9.0);
        let whole: (Vec<f64>, Vec<f64>) = run(8000.0, &input, &[usize::MAX]);
        assert!(whole.0.iter().any(|x| *x != 0.0));
        for split in [&[1usize][..], &[7], &[64, 1, 333], &[4096, 5]] {
            assert!(run::<f64>(8000.0, &input, split) == whole, "{split:?}");
        }
        let whole: (Vec<f32>, Vec<f32>) = run(8000.0, &input, &[usize::MAX]);
        for split in [&[1usize][..], &[13, 100], &[64]] {
            assert!(run::<f32>(8000.0, &input, split) == whole, "f32 {split:?}");
        }
    }

    #[test]
    fn reset_is_as_new() {
        let input = noise(5000, 3);
        let mut rv = Reverb::new(44100.0);
        let (mut l, mut r) = (vec![0.0; 5000], vec![0.0; 5000]);
        rv.process(&input, &mut l, &mut r);
        rv.reset();
        assert!(rv.is_silent());
        let (mut l, mut r) = (vec![0.0f64; 5000], vec![0.0f64; 5000]);
        rv.process(&input, &mut l, &mut r);
        let mut fresh = Reverb::new(44100.0);
        let (mut a, mut b) = (vec![0.0f64; 5000], vec![0.0f64; 5000]);
        fresh.process(&input, &mut a, &mut b);
        assert!(l == a && r == b);
    }

    #[test]
    fn clear_lines_keeps_the_swings() {
        let input = noise(5000, 4);
        let mut rv = Reverb::new(48000.0);
        let (mut l, mut r) = (vec![0.0f64; 5000], vec![0.0f64; 5000]);
        rv.process(&input, &mut l, &mut r);
        rv.clear_lines();
        assert!(rv.is_silent());
        let (mut a, mut b) = (vec![0.0f64; 5000], vec![0.0f64; 5000]);
        rv.process(&input, &mut a, &mut b);
        // As a fresh one whose swings have moved on as far.
        let mut fresh = Reverb::new(48000.0);
        fresh.skip(5000);
        let (mut c, mut d) = (vec![0.0f64; 5000], vec![0.0f64; 5000]);
        fresh.process(&input, &mut c, &mut d);
        assert!(a == c && b == d);
    }

    #[test]
    fn channels_differ() {
        let input = noise(44100, 5);
        let (l, r): (Vec<f64>, Vec<f64>) = run(44100.0, &input, &[usize::MAX]);
        let (el, er) = (energy(&l), energy(&r));
        let cross: f64 = l.iter().zip(&r).map(|(a, b)| a * b).sum();
        // About as loud, but far from the same signal.
        assert!((el / er).log10().abs() < 0.1, "{el} {er}");
        assert!(cross / (el * er).sqrt() < 0.9, "{}", cross / (el * er).sqrt());
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
        hostile.extend(std::iter::repeat_n(f32::MAX, 20000));
        for rate in RATES {
            let mut rv = Reverb::new(rate);
            let (mut l, mut r) = (vec![0.0f32; hostile.len()], vec![0.0f32; hostile.len()]);
            rv.process(&hostile, &mut l, &mut r);
            assert!(l.iter().chain(&r).all(|x| x.is_finite()), "{rate}");
            // And it recovers: the clamped tail dies away.
            assert!(frames_to_silence(&mut rv, (rate * 12.0) as usize).is_some(), "{rate}");
            let input = noise(1000, 6);
            let (mut l, mut r) = (vec![0.0f64; 1000], vec![0.0f64; 1000]);
            rv.process(&input, &mut l, &mut r);
            assert!(l.iter().chain(&r).all(|x| x.is_finite()));
        }
    }

    #[test]
    fn skip_is_processing_silence() {
        let input = noise(2000, 8);
        let probe = noise(3000, 9);
        for frames in [0, 1, 100, 5000, 1 << 20] {
            let mut a = Reverb::new(22050.0);
            let mut b = Reverb::new(22050.0);
            let (mut l, mut r) = (vec![0.0f64; 2000], vec![0.0f64; 2000]);
            a.process(&input, &mut l, &mut r);
            b.process(&input, &mut l, &mut r);
            // Not yet silent too: skip then processes the zeros itself.
            if frames == 100 {
                assert!(!a.is_silent());
            } else {
                frames_to_silence(&mut a, 22050 * 10).unwrap();
                frames_to_silence(&mut b, 22050 * 10).unwrap();
            }
            a.skip(frames);
            let zeros = vec![0.0f64; frames];
            let (mut l, mut r) = (vec![0.0f64; frames], vec![0.0f64; frames]);
            b.process(&zeros, &mut l, &mut r);
            assert_eq!(a.is_silent(), b.is_silent());
            let (mut la, mut ra) = (vec![0.0f64; 3000], vec![0.0f64; 3000]);
            let (mut lb, mut rb) = (vec![0.0f64; 3000], vec![0.0f64; 3000]);
            a.process(&probe, &mut la, &mut ra);
            b.process(&probe, &mut lb, &mut rb);
            assert!(la == lb && ra == rb, "{frames}");
        }
    }

    #[test]
    fn adds_into_the_buffers() {
        let input = noise(3000, 10);
        let (mut l, mut r) = (vec![0.0f64; 3000], vec![0.0f64; 3000]);
        Reverb::new(44100.0).process(&input, &mut l, &mut r);
        let mut rv = Reverb::new(44100.0);
        let (mut l2, mut r2) = (vec![0.5f64; 3000], vec![-0.25f64; 3000]);
        rv.process(&input, &mut l2, &mut r2);
        assert!(l.iter().zip(&l2).all(|(a, b)| a + 0.5 == *b));
        assert!(r.iter().zip(&r2).all(|(a, b)| a - 0.25 == *b));
        // Shortest length only.
        let mut rv = Reverb::new(44100.0);
        let (mut l3, mut r3) = (vec![0.0f64; 10], vec![0.0f64; 3000]);
        rv.process(&input, &mut l3, &mut r3);
        assert!(r3[10..].iter().all(|x| *x == 0.0));
    }
}
