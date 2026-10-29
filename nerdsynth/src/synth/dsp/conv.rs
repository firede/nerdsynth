//! Unit conversions, after FluidSynth's `utils/fluid_conv.c` and the tables
//! of `gentables/`, computed exactly instead of from tables where the tables
//! only trade precision for speed.

/// The peak attenuation of the default attenuation modulators and of the
/// volume envelope's dB scale, in centibels (FluidSynth's
/// `FLUID_PEAK_ATTENUATION`).
pub(crate) const PEAK_ATTENUATION: f64 = 960.0;

/// The size of the concave and convex tables (`FLUID_VEL_CB_SIZE`).
const CURVE_SIZE: usize = 128;

/// Absolute cents to Hz: 6900 cents is 440 Hz.
///
/// FluidSynth truncates to whole cents (`fluid_ct2hz_real`); this does not.
pub(crate) fn ct2hz(cents: f64) -> f64 {
    440.0 * ((cents - 6900.0) / 1200.0).exp2()
}

/// Hz to absolute cents (`fluid_hz2ct`).
pub(crate) fn hz2ct(hz: f64) -> f64 {
    6900.0 + 1200.0 * (hz / 440.0).log2()
}

/// Centibels of attenuation to an amplitude (`fluid_cb2amp`): negative
/// attenuations amplify, 1441 cB and more are silence.
///
/// FluidSynth looks positive values up in a table of whole centibels; this
/// does not.
pub(crate) fn cb2amp(cb: f64) -> f64 {
    if cb >= 1441.0 { 0.0 } else { 10f64.powf(cb / -200.0) }
}

/// Timecents to seconds, without range checks (`fluid_tc2sec`).
pub(crate) fn tc2sec(tc: f64) -> f64 {
    (tc / 1200.0).exp2()
}

/// A delay in timecents, -12000 to 5000, to seconds (`fluid_tc2sec_delay`).
pub(crate) fn tc2sec_delay(tc: f64) -> f64 {
    if tc <= -32768.0 { 0.0 } else { tc2sec(tc.clamp(-12000.0, 5000.0)) }
}

/// An attack or release in timecents, -12000 to 8000, to seconds
/// (`fluid_tc2sec_attack`, `fluid_tc2sec_release`).
pub(crate) fn tc2sec_attack(tc: f64) -> f64 {
    if tc <= -32768.0 { 0.0 } else { tc2sec(tc.clamp(-12000.0, 8000.0)) }
}

/// The concave curve at `x`, 0 to 127, interpolated between the points of
/// FluidSynth's table (`fluid_concave`, `gentables/fluid_concave.cpp`).
pub(crate) fn concave(x: f64) -> f64 {
    curve(&CURVES.0, x)
}

/// The convex curve at `x`, 0 to 127 (`fluid_convex`).
pub(crate) fn convex(x: f64) -> f64 {
    curve(&CURVES.1, x)
}

fn curve(table: &[f64; CURVE_SIZE], x: f64) -> f64 {
    if x.is_nan() || x < 0.0 {
        return 0.0;
    }
    let i = x as usize;
    if i >= CURVE_SIZE - 1 {
        return table[CURVE_SIZE - 1];
    }
    table[i] + (table[i + 1] - table[i]) * (x - i as f64)
}

struct Curves([f64; CURVE_SIZE], [f64; CURVE_SIZE]);

static CURVES: std::sync::LazyLock<Curves> = std::sync::LazyLock::new(|| {
    let mut concave = [0.0; CURVE_SIZE];
    let mut convex = [0.0; CURVE_SIZE];
    let top = (CURVE_SIZE - 1) as f64;
    for i in 1..CURVE_SIZE - 1 {
        let k = -200.0 * 2.0 / PEAK_ATTENUATION;
        concave[i] = k * ((top - i as f64) / top).log10();
        convex[i] = 1.0 - k * (i as f64 / top).log10();
    }
    concave[CURVE_SIZE - 1] = 1.0;
    convex[CURVE_SIZE - 1] = 1.0;
    Curves(concave, convex)
});

/// The gain of one side for a pan of `c`, -500 (left) to 500 (right), in
/// tenths of a percent (`fluid_pan`): a sine law over FluidSynth's table of
/// 1002 points, the pan truncated to whole tenths.
pub(crate) fn pan(c: f64, left: bool) -> f64 {
    let c = if left { -c } else { c };
    if c.is_nan() {
        return std::f64::consts::FRAC_1_SQRT_2;
    }
    if c <= -500.0 {
        0.0
    } else if c >= 500.0 {
        1.0
    } else {
        let i = c.trunc() + 500.0;
        (i * std::f64::consts::FRAC_PI_2 / 1001.0).sin()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversions() {
        assert!((ct2hz(6900.0) - 440.0).abs() < 1e-9);
        assert!((ct2hz(0.0) - 8.175_798_915_643_707).abs() < 1e-9);
        assert!((hz2ct(440.0) - 6900.0).abs() < 1e-9);
        assert_eq!(cb2amp(0.0), 1.0);
        assert!((cb2amp(200.0) - 0.1).abs() < 1e-12);
        assert!((cb2amp(-200.0) - 10.0).abs() < 1e-9);
        assert_eq!(cb2amp(1441.0), 0.0);
        assert_eq!(tc2sec_delay(-32768.0), 0.0);
        assert!((tc2sec(1200.0) - 2.0).abs() < 1e-12);
        assert_eq!(concave(0.0), 0.0);
        assert_eq!(concave(127.0), 1.0);
        assert_eq!(convex(127.0), 1.0);
        assert!(concave(64.0) < 0.5 && convex(64.0) > 0.5);
        assert_eq!(pan(-500.0, true), 1.0);
        assert_eq!(pan(-500.0, false), 0.0);
        assert!((pan(0.0, true) - pan(0.0, false)).abs() < 1e-12);
    }
}
