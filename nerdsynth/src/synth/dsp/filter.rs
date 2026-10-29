//! The voice filter (§8.1.2, generators 8 and 9): a resonant second-order
//! low-pass, as FluidSynth 2.6's `fluid_iir_filter`: Robert Bristow-Johnson's
//! cookbook coefficients, Q in centibels above a non-resonant filter, and
//! the gain reduced by half the height of the resonance peak (SoundFont
//! 2.01 page 59). The voice's gain is applied after the filter, as in
//! FluidSynth 2.6, so that turning a resonant voice down does not click.
//!
//! Also as FluidSynth: when the cutoff moves by more than a cent, it glides
//! there in cents over 64 frames times the Q (1 to 5), and when the Q
//! changes, it glides over 64 frames; while gliding, the coefficients are
//! recomputed every frame from a table of the sine and cosine of every
//! cent. (Gliding the coefficients themselves instead, though every step
//! is a stable filter, diverges under fast deep modulation.) Should the
//! output still grow beyond what a resonance can reach, the voice stops.

use super::conv::{ct2hz, hz2ct};

/// The coefficients, normalized by a0.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub(crate) struct Coeffs {
    pub(crate) a1: f32,
    pub(crate) a2: f32,
    /// b0 and b2, which are equal.
    pub(crate) b02: f32,
    pub(crate) b1: f32,
}

/// The lowest and highest cutoff of the table, in cents: 20 Hz to about 20
/// kHz (FluidSynth's `FRES_MIN`, `FRES_MAX`).
const TABLE_MIN: i32 = 1500;
const TABLE_MAX: i32 = 13500;

/// FluidSynth's lowest Q (`Q_MIN`).
const Q_MIN: f32 = 0.001;

/// Frames of a glide of the Q, and of the cutoff for each unit of Q.
const GLIDE: f32 = 64.0;

/// Beyond this the output is runaway: at most the square root of the Q
/// (about 250 for 96 dB) times the input. The state itself is no measure:
/// its gain at resonance is about Q over sin² of the cutoff's angle, 1e8
/// and more for low cutoffs at high sample rates.
const OUTPUT_LIMIT: f32 = 1e5;

/// The sine and cosine of the angular frequency of every cent of the table,
/// at a sample rate (FluidSynth's `fluid_iir_filter_init_table`).
pub(crate) struct SinCos {
    table: Box<[(f32, f32)]>,
    /// The highest cutoff in cents: 0.45 times the sample rate.
    max_cents: f64,
}

impl SinCos {
    pub(crate) fn new(sample_rate: f64) -> SinCos {
        let table = (TABLE_MIN..=TABLE_MAX)
            .map(|cents| {
                let omega = std::f64::consts::TAU * ct2hz(f64::from(cents)) / sample_rate;
                (omega.sin() as f32, omega.cos() as f32)
            })
            .collect();
        SinCos { table, max_cents: hz2ct(0.45 * sample_rate) }
    }

    /// The low-pass coefficients for a cutoff in cents (whole cents of the
    /// table) and a linear Q (`fluid_iir_filter_calculate_coefficients`).
    pub(crate) fn lowpass(&self, cents: f32, q: f32) -> Coeffs {
        let i = ((cents as i32).saturating_sub(TABLE_MIN)).clamp(0, TABLE_MAX - TABLE_MIN) as usize;
        let (sin, cos) = self.table.get(i).copied().unwrap_or((0.0, 1.0));
        let alpha = sin / (2.0 * q);
        let a0_inv = 1.0 / (1.0 + alpha);
        let b1 = (1.0 - cos) * a0_inv / q.sqrt();
        Coeffs { a1: -2.0 * cos * a0_inv, a2: (1.0 - alpha) * a0_inv, b02: b1 * 0.5, b1 }
    }
}

/// FluidSynth's Q: `q_cb` (generator 9) in decibels less 3.01, so that 0 is
/// no resonance hump, as a linear factor (`fluid_iir_filter_q_from_dB`).
pub(crate) fn q_linear(q_cb: f64) -> f64 {
    let db = (q_cb / 10.0).clamp(0.0, 96.0) - 3.01;
    10f64.powf(db / 20.0)
}

/// A filter's state: its history, its cutoff and Q, gliding towards their
/// targets, and its coefficients.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Filter {
    hist1: f32,
    hist2: f32,
    coeffs: Coeffs,
    started: bool,
    cents: f32,
    cents_step: f32,
    cents_left: u32,
    q: f32,
    q_step: f32,
    q_left: u32,
    q_target: f32,
}

impl Filter {
    /// Sets the cutoff, in cents, and the linear Q to go to: at once when
    /// the filter starts, else gliding (`fluid_iir_filter_calc`,
    /// `fluid_iir_filter_set_q`).
    pub(crate) fn set(&mut self, cents: f64, q: f64, table: &SinCos) {
        let min = hz2ct(5.0);
        let cents = if cents.is_finite() { cents.clamp(min, table.max_cents.max(min)) } else { min } as f32;
        let q = if q.is_finite() { q.max(f64::from(Q_MIN)) } else { 1.0 } as f32;
        if !self.started {
            *self = Filter { started: true, cents, q, q_target: q, ..Filter::default() };
            self.coeffs = table.lowpass(cents, q);
            return;
        }
        if q != self.q_target {
            self.q_target = q;
            self.q_step = (q - self.q) / GLIDE;
            self.q_left = GLIDE as u32;
        }
        let diff = cents - self.cents;
        if diff.abs() > 1.0 {
            let frames = GLIDE * self.q.clamp(1.0, 5.0);
            self.cents_step = diff / frames;
            self.cents_left = (frames + 0.5) as u32;
            self.coeffs = table.lowpass(self.cents, self.q);
        } else {
            let changed = cents != self.cents;
            self.cents = cents;
            self.cents_left = 0;
            if changed {
                self.coeffs = table.lowpass(self.cents, self.q);
            }
        }
    }

    /// Filters `buf` in place, multiplying the output by a gain that starts
    /// at `*gain` and goes up by `gain_step` a frame, never below 0 (Direct
    /// Form II, as FluidSynth), and gliding the cutoff and Q. Gives false if
    /// it ran away (its output beyond any resonance, or not finite), after
    /// clearing its state.
    pub(crate) fn process(&mut self, buf: &mut [f32], gain: &mut f32, gain_step: f32, table: &SinCos) -> bool {
        let (mut h1, mut h2) = (self.hist1, self.hist2);
        let mut c = self.coeffs;
        let mut g = *gain;
        let mut peak = 0.0f32;
        for x in buf.iter_mut() {
            let center = *x - c.a1 * h1 - c.a2 * h2;
            let y = c.b02 * (center + h2) + c.b1 * h1;
            h2 = h1;
            h1 = center;
            peak = peak.max(y.abs());
            *x = y * g;
            g = (g + gain_step).max(0.0);
            if self.cents_left > 0 || self.q_left > 0 {
                if self.cents_left > 0 {
                    self.cents_left -= 1;
                    self.cents += self.cents_step;
                }
                if self.q_left > 0 {
                    self.q_left -= 1;
                    self.q = (self.q + self.q_step).max(Q_MIN);
                }
                c = table.lowpass(self.cents, self.q);
            }
        }
        // Denormals, as FluidSynth.
        if h1.abs() < 1e-20 {
            h1 = 0.0;
        }
        if h2.abs() < 1e-20 {
            h2 = 0.0;
        }
        self.coeffs = c;
        *gain = g;
        // `peak < OUTPUT_LIMIT` is false for NaN too.
        if h1.is_finite() && h2.is_finite() && peak < OUTPUT_LIMIT {
            self.hist1 = h1;
            self.hist2 = h2;
            true
        } else {
            self.hist1 = 0.0;
            self.hist2 = 0.0;
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The gain at `hz` of the filter of `cents` and `q`, measured on a
    /// sine.
    fn gain_at(cents: f64, q: f64, hz: f64, sr: f64) -> f64 {
        let table = SinCos::new(sr);
        let mut f = Filter::default();
        f.set(cents, q, &table);
        let n = 20000;
        let mut buf: Vec<f32> = (0..n).map(|i| (std::f64::consts::TAU * hz * i as f64 / sr).sin() as f32).collect();
        let mut g = 1.0;
        f.process(&mut buf, &mut g, 0.0, &table);
        let tail = &buf[n / 2..];
        let rms = (tail.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>() / tail.len() as f64).sqrt();
        rms * 2f64.sqrt()
    }

    #[test]
    fn no_resonance_is_3_db_below_the_pass_band_at_the_cutoff() {
        // FluidSynth: a Q of 0 cB is a linear Q of 1/sqrt(2), and the gain
        // normalization 1/sqrt(Q) lifts the pass band by 1.5 dB.
        let sr = 44100.0;
        let db = |hz| 20.0 * gain_at(hz2ct(1000.0), q_linear(0.0), hz, sr).log10();
        assert!((db(1000.0) + 1.505).abs() < 0.1, "{}", db(1000.0));
        assert!((db(50.0) - 1.505).abs() < 0.1, "{}", db(50.0));
        assert!(db(10000.0) < -30.0);
    }

    #[test]
    fn resonance_peaks_at_half_its_height() {
        let sr = 44100.0;
        // At the cutoff the gain is Q, less half of it in decibels.
        let q = q_linear(200.0);
        let db = 20.0 * gain_at(hz2ct(1000.0), q, 1000.0, sr).log10();
        assert!((db - 10.0 * q.log10()).abs() < 0.3, "{db}");
    }

    #[test]
    fn the_cutoff_stays_below_nyquist() {
        for sr in [8000.0, 22050.0, 44100.0, 192000.0] {
            let table = SinCos::new(sr);
            let mut f = Filter::default();
            f.set(20000.0, q_linear(960.0), &table);
            let mut buf = vec![1.0f32; 10000];
            let mut g = 1.0;
            assert!(f.process(&mut buf, &mut g, 0.0, &table));
            assert!(buf.iter().all(|x| x.is_finite()));
        }
    }

    /// The cutoff glides in cents over 64 frames times the Q, and the Q
    /// over 64 frames.
    #[test]
    fn cutoff_and_q_glide() {
        let table = SinCos::new(44100.0);
        let mut f = Filter::default();
        f.set(6000.0, 2.0, &table);
        f.set(8000.0, 3.0, &table);
        let mut buf = [0.0f32; 64];
        let mut g = 1.0;
        f.process(&mut buf, &mut g, 0.0, &table);
        assert!((f.cents - 7000.0).abs() < 1.0, "{}", f.cents);
        assert!((f.q - 3.0).abs() < 1e-4, "{}", f.q);
        f.process(&mut buf, &mut g, 0.0, &table);
        assert!((f.cents - 8000.0).abs() < 1.0, "{}", f.cents);
        assert_eq!(f.coeffs, table.lowpass(f.cents, f.q));
    }
}
