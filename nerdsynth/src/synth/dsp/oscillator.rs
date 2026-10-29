//! Reading a sample at a pitch: the position in 32.32 fixed point, four-point
//! cubic (FluidSynth's `fluid_rvoice_dsp_interpolate_4th_order`) or linear
//! interpolation, and the loop.
//!
//! Every point read is within the sample: positions are clamped when the
//! voice starts (see [`Bounds::new`]); the points around the loop's ends
//! wrap into the loop while looping (the point before the start is the
//! last of the loop once it has looped), and repeat the end otherwise.
//! A step longer than the loop wraps by the remainder, so a loop of two
//! points at any pitch never reads outside it (E-10).

use crate::synth::Interpolation;

const FRAC_BITS: u32 = 32;
const ONE: u64 = 1 << FRAC_BITS;

/// The points a voice may read, all within the sample data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Bounds {
    pub(crate) start: u32,
    /// The last point played, inclusive (FluidSynth's `end`).
    pub(crate) end: u32,
    pub(crate) loop_start: u32,
    /// The point after the loop.
    pub(crate) loop_end: u32,
    /// Whether the loop is valid: at least two points, within the sample.
    pub(crate) can_loop: bool,
}

/// A sample header's positions and a voice's offsets (generators 0–4, 12,
/// 45, 50), as absolute positions.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Positions {
    pub(crate) start: i64,
    /// The point after the last.
    pub(crate) end: i64,
    pub(crate) loop_start: i64,
    pub(crate) loop_end: i64,
    pub(crate) start_offset: i64,
    pub(crate) end_offset: i64,
    pub(crate) loop_start_offset: i64,
    pub(crate) loop_end_offset: i64,
}

impl Bounds {
    /// The bounds of a sample header within data of `len` points, with a
    /// voice's offsets, clamped as FluidSynth's
    /// `fluid_rvoice_check_sample_sanity` does; `None` when nothing is left
    /// to play (P-SF-11).
    ///
    /// The header itself is clamped into the data first. A loop the header
    /// puts outside its sample, or of fewer than two points, is no loop.
    pub(crate) fn new(p: Positions, len: usize) -> Option<Bounds> {
        let len = len.min(u32::MAX as usize) as i64;
        let s_start = p.start.clamp(0, len);
        let s_end = p.end.clamp(s_start, len);
        if s_end - s_start < 1 {
            return None;
        }
        // FluidSynth's sample end is the last point, inclusive.
        let (min, max) = (s_start, s_end - 1);
        let mut start = (p.start + p.start_offset).clamp(min, max);
        let mut end = (p.end - 1 + p.end_offset).clamp(min, max);
        if start > end {
            std::mem::swap(&mut start, &mut end);
        }
        if start == end {
            return None;
        }
        let header_loop_ok = p.loop_start >= s_start && p.loop_end <= s_end && p.loop_end >= p.loop_start + 2;
        let (lmin, lmax) = (s_start, s_end);
        let mut ls = (p.loop_start + p.loop_start_offset).clamp(lmin, lmax);
        let mut le = (p.loop_end + p.loop_end_offset).clamp(lmin, lmax);
        if ls > le {
            std::mem::swap(&mut ls, &mut le);
        }
        let can_loop = header_loop_ok && le >= ls + 2;
        Some(Bounds { start: start as u32, end: end as u32, loop_start: ls as u32, loop_end: le as u32, can_loop })
    }
}

/// The sample data a voice reads: 16-bit points, and the low bytes of 24-bit
/// ones.
#[derive(Clone, Copy)]
pub(crate) struct Data<'a> {
    pub(crate) points: &'a [i16],
    pub(crate) low: Option<&'a [u8]>,
}

impl Data<'_> {
    #[inline]
    fn at(&self, i: u32) -> f32 {
        let i = i as usize;
        let hi = self.points.get(i).copied().unwrap_or(0);
        match self.low {
            Some(low) => {
                let lo = low.get(i).copied().unwrap_or(0);
                (i32::from(hi) << 8 | i32::from(lo)) as f32 * (1.0 / 8_388_608.0)
            }
            None => f32::from(hi) * (1.0 / 32768.0),
        }
    }
}

/// Where a voice is in its sample.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Oscillator {
    pub(crate) bounds: Bounds,
    /// 32.32 fixed point, absolute in the data.
    pos: u64,
    has_looped: bool,
    /// Past the end of the sample.
    pub(crate) finished: bool,
}

impl Oscillator {
    pub(crate) fn new(bounds: Bounds) -> Oscillator {
        Oscillator { bounds, pos: u64::from(bounds.start) << FRAC_BITS, has_looped: false, finished: false }
    }

    /// The point played, for tests.
    #[cfg(test)]
    pub(crate) fn index(&self) -> u32 {
        (self.pos >> FRAC_BITS) as u32
    }

    /// The step a frame for a ratio of the sample's speed to the output's.
    pub(crate) fn step(ratio: f64) -> u64 {
        // Up to 2^20 points a frame; at least the smallest fraction, so that
        // the voice never stops.
        let r = if ratio.is_finite() { ratio.clamp(0.0, 1_048_576.0) } else { 0.0 };
        ((r * ONE as f64) as u64).max(1)
    }

    /// Writes `out.len()` points, stepping `step` (32.32) a point, looping
    /// if `looping`. Past the end it writes zeros and is finished.
    pub(crate) fn render(&mut self, data: Data<'_>, out: &mut [f32], step: u64, looping: bool, interp: Interpolation) {
        let b = self.bounds;
        let looping = looping && b.can_loop;
        if looping && (self.pos >> FRAC_BITS) as u32 >= b.loop_end {
            // The loop moved before the position; FluidSynth goes to its
            // start.
            self.pos = u64::from(b.loop_start) << FRAC_BITS;
        }
        let loop_len = u64::from(b.loop_end - b.loop_start);
        // The points the fast path may read: i - 1 to i + 2 within these.
        let upper = if looping { b.loop_end } else { b.end.saturating_add(1) };
        for (n, x) in out.iter_mut().enumerate() {
            if self.finished {
                for y in &mut out[n..] {
                    *y = 0.0;
                }
                return;
            }
            let i = (self.pos >> FRAC_BITS) as u32;
            let frac = (self.pos & (ONE - 1)) as f32 * (1.0 / ONE as f32);
            *x = match interp {
                Interpolation::Linear => {
                    let s0 = data.at(i);
                    let s1 = data.at(self.next(i, 1, looping));
                    s0 + frac * (s1 - s0)
                }
                Interpolation::Cubic => {
                    let lower_ok = i > b.start && !(looping && self.has_looped && i <= b.loop_start);
                    let (sm, s0, s1, s2) = if lower_ok && i.saturating_add(2) < upper {
                        (data.at(i - 1), data.at(i), data.at(i + 1), data.at(i + 2))
                    } else {
                        (
                            data.at(self.previous(i, looping)),
                            data.at(i),
                            data.at(self.next(i, 1, looping)),
                            data.at(self.next(i, 2, looping)),
                        )
                    };
                    let f = frac;
                    let c0 = f * (-0.5 + f * (1.0 - 0.5 * f));
                    let c1 = 1.0 + f * f * (1.5 * f - 2.5);
                    let c2 = f * (0.5 + f * (2.0 - 1.5 * f));
                    let c3 = 0.5 * f * f * (f - 1.0);
                    c0 * sm + c1 * s0 + c2 * s1 + c3 * s2
                }
            };
            self.pos = self.pos.wrapping_add(step);
            let i = self.pos >> FRAC_BITS;
            if looping {
                if i >= u64::from(b.loop_end) {
                    let wrapped = u64::from(b.loop_start) + (i - u64::from(b.loop_start)) % loop_len;
                    self.pos = wrapped << FRAC_BITS | (self.pos & (ONE - 1));
                    self.has_looped = true;
                }
            } else if i > u64::from(b.end) {
                self.finished = true;
            }
        }
    }

    /// The point `k` after `i`, wrapped into the loop or held at the end.
    #[inline]
    fn next(&self, i: u32, k: u32, looping: bool) -> u32 {
        let b = &self.bounds;
        let j = u64::from(i) + u64::from(k);
        if looping {
            if j >= u64::from(b.loop_end) {
                let len = u64::from(b.loop_end - b.loop_start);
                return (u64::from(b.loop_start) + (j - u64::from(b.loop_start)) % len) as u32;
            }
            j as u32
        } else {
            j.min(u64::from(b.end)) as u32
        }
    }

    /// The point before `i`: the last of the loop at its start once looped,
    /// else the first point repeated.
    #[inline]
    fn previous(&self, i: u32, looping: bool) -> u32 {
        let b = &self.bounds;
        if looping && self.has_looped && i <= b.loop_start {
            b.loop_end - 1
        } else if i <= b.start {
            b.start
        } else {
            i - 1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn positions(start: i64, end: i64, ls: i64, le: i64) -> Positions {
        Positions {
            start,
            end,
            loop_start: ls,
            loop_end: le,
            start_offset: 0,
            end_offset: 0,
            loop_start_offset: 0,
            loop_end_offset: 0,
        }
    }

    #[test]
    fn headers_are_clamped_into_the_data() {
        let b = Bounds::new(positions(-5, 1_000_000, 10, 20), 100).unwrap();
        assert_eq!((b.start, b.end), (0, 99));
        let b = Bounds::new(positions(50, 40, 0, 0), 100);
        assert!(b.is_none());
        let b = Bounds::new(positions(0, 100, 90, 200), 100).unwrap();
        assert!(!b.can_loop, "a loop outside the sample is no loop");
        let mut p = positions(0, 100, 10, 20);
        p.start_offset = 50;
        p.loop_end_offset = -1_000_000;
        let b = Bounds::new(p, 100).unwrap();
        assert_eq!(b.start, 50);
        // The loop end clamps to the sample start and the points swap, as in
        // FluidSynth.
        assert_eq!((b.loop_start, b.loop_end, b.can_loop), (0, 10, true));
        // Offsets past the end leave nothing to play.
        p.start_offset = 1_000_000;
        assert!(Bounds::new(p, 100).is_none());
    }

    #[test]
    fn short_loops_wrap_by_the_remainder() {
        let points: Vec<i16> = (0..100).map(|i| if (40..42).contains(&i) { 1000 } else { -30000 }).collect();
        let data = Data { points: &points, low: None };
        let b = Bounds::new(positions(0, 100, 40, 42), 100).unwrap();
        let mut o = Oscillator::new(b);
        o.pos = 40 << FRAC_BITS;
        o.has_looped = true;
        let mut out = vec![0.0; 1000];
        // 37.25 points a frame over a loop of 2.
        o.render(data, &mut out, Oscillator::step(37.25), true, Interpolation::Cubic);
        assert!(out.iter().all(|x| (x - 1000.0 / 32768.0).abs() < 1e-6), "read outside the loop");
        assert!((40..42).contains(&o.index()));
    }

    #[test]
    fn unlooped_samples_end() {
        let points = vec![100i16; 10];
        let data = Data { points: &points, low: None };
        let b = Bounds::new(positions(0, 10, 0, 0), 10).unwrap();
        let mut o = Oscillator::new(b);
        let mut out = vec![1.0; 20];
        o.render(data, &mut out, Oscillator::step(1.0), false, Interpolation::Cubic);
        assert!(o.finished);
        assert!(out[..10].iter().all(|x| (x - 100.0 / 32768.0).abs() < 1e-7));
        assert!(out[10..].iter().all(|x| *x == 0.0));
    }

    #[test]
    fn low_bytes_make_24_bits() {
        let points = vec![1i16; 4];
        let low = vec![0x80u8; 4];
        let data = Data { points: &points, low: Some(&low) };
        assert_eq!(data.at(0), (256.0 + 128.0) / 8_388_608.0);
    }
}
