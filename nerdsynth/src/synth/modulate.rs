//! Modulator evaluation (SoundFont 2.04 §8.2, §9.5), with FluidSynth 2.6's
//! mapping of source values (`fluid_mod_get_source_value`,
//! `fluid_mod_transform_source_value` and `fluid_mod_get_value` in
//! `src/synth/fluid_mod.c`).
//!
//! FluidSynth maps a 7-bit controller over a range of 128, not 127, so that
//! the center value 64 is exactly the middle of a bipolar source; the
//! unipolar maximum is then 127/128, and the bipolar maximum is kept at
//! 127/128 as well instead of 126/128.

use super::dsp::conv::{concave, convex};
use crate::soundfont::{Modulator, Source};

/// General controller indices (§8.2.1).
const NONE: u8 = 0;
const VELOCITY: u8 = 2;
const KEY: u8 = 3;
const POLY_PRESSURE: u8 = 10;
const CHANNEL_PRESSURE: u8 = 13;
const PITCH_WHEEL: u8 = 14;
const PITCH_WHEEL_SENSITIVITY: u16 = 16;

const VIB_LFO_TO_PITCH: u16 = 6;
const MOD_LFO_TO_PITCH: u16 = 5;
pub(crate) const INITIAL_FILTER_FC: u16 = 8;

/// What a voice's modulators read: the state of its channel, and its key
/// and velocity.
#[derive(Clone, Copy)]
pub(crate) struct ModInput<'a> {
    pub(crate) cc: &'a [u8; 128],
    /// The key and velocity after the keynum and velocity generators.
    pub(crate) key: u8,
    pub(crate) velocity: u8,
    pub(crate) poly_pressure: u8,
    pub(crate) channel_pressure: u8,
    pub(crate) pitch_bend: u16,
    /// Semitones.
    pub(crate) bend_range: f64,
    /// RPN 5, cents; 50 by default.
    pub(crate) mod_depth_range: f64,
    /// Whether a modulator identical to FluidSynth's velocity-to-filter
    /// default modulator gives 0, as in FluidSynth.
    pub(crate) mute_fluid_vel_to_filter: bool,
}

/// FluidSynth's form of the velocity-to-filter-cutoff default modulator:
/// velocity, negative unipolar linear, times velocity as a positive
/// unipolar switch, -2400 cents.
pub(crate) const FLUID_VEL_TO_FILTER: Modulator = Modulator {
    source: Source(0x0102),
    destination: INITIAL_FILTER_FC,
    amount: -2400,
    amount_source: Source(0x0C02),
    transform: 0,
};

/// The specification's form (2.01, §8.4.2): velocity, negative unipolar
/// linear, -2400 cents.
pub(crate) const LINEAR_VEL_TO_FILTER: Modulator = Modulator {
    source: Source(0x0102),
    destination: INITIAL_FILTER_FC,
    amount: -2400,
    amount_source: Source::NONE,
    transform: 0,
};

/// SpessaSynth's CC74 (brightness) modulator: positive bipolar linear,
/// 9600 cents to the filter cutoff.
pub(crate) const CC74_TO_FILTER: Modulator = Modulator {
    source: Source(0x02CA),
    destination: INITIAL_FILTER_FC,
    amount: 9600,
    amount_source: Source::NONE,
    transform: 0,
};

/// Whether the modulator is FluidSynth's velocity-to-filter modulator by
/// FluidSynth's identity test, which ignores the amount and the transform.
fn is_fluid_vel_to_filter(m: &Modulator) -> bool {
    m.source == FLUID_VEL_TO_FILTER.source
        && m.amount_source == FLUID_VEL_TO_FILTER.amount_source
        && m.destination == FLUID_VEL_TO_FILTER.destination
}

/// Whether the output of a modulator can change after the note started:
/// it reads a controller of the channel rather than only the note's key and
/// velocity.
pub(crate) fn is_dynamic(m: &Modulator) -> bool {
    let dynamic = |s: Source| s.is_cc() || matches!(s.index(), POLY_PRESSURE | CHANNEL_PRESSURE | PITCH_WHEEL | 16);
    dynamic(m.source) || dynamic(m.amount_source)
}

/// The raw value of a source and the range it is mapped over; `None` for
/// "no controller".
fn raw(source: Source, input: &ModInput<'_>) -> Option<(f64, f64)> {
    let index = source.index();
    if source.is_cc() {
        // FluidSynth: an index of 0 with the CC flag is also "none".
        if index == NONE {
            return None;
        }
        return Some((f64::from(input.cc[index as usize]), 128.0));
    }
    let value = match index {
        NONE => return None,
        VELOCITY => input.velocity.into(),
        KEY => input.key.into(),
        POLY_PRESSURE => input.poly_pressure.into(),
        CHANNEL_PRESSURE => input.channel_pressure.into(),
        PITCH_WHEEL => return Some((f64::from(input.pitch_bend), 16384.0)),
        i if u16::from(i) == PITCH_WHEEL_SENSITIVITY => input.bend_range,
        // A linked modulator's output, or an unknown controller: FluidSynth
        // disables the modulator.
        _ => 0.0,
    };
    Some((value, 128.0))
}

/// A source mapped through its polarity, direction and curve (§8.2.2).
fn mapped(source: Source, input: &ModInput<'_>) -> f64 {
    let Some((value, range)) = raw(source, input) else {
        return 1.0;
    };
    let norm = value / range;
    let inverse = 1.0 - 1.0 / range - norm;
    let top = (range - 1.0) / range;
    let negative = source.is_negative();
    match (source.is_bipolar(), source.curve()) {
        (false, curve) => {
            let v = if negative { inverse } else { norm };
            match curve {
                3 => {
                    if v >= 0.5 {
                        1.0
                    } else {
                        0.0
                    }
                }
                1 => concave(128.0 * v).min(top),
                2 => convex(128.0 * v).min(top),
                _ => v,
            }
        }
        (true, curve) => {
            let n = if negative { inverse } else { norm };
            let v = if n == top { n } else { -1.0 + 2.0 * n };
            match curve {
                3 => {
                    if v >= 0.0 {
                        1.0
                    } else {
                        -1.0
                    }
                }
                1 => {
                    if v >= 0.0 {
                        concave(128.0 * v).min(top)
                    } else {
                        -concave(-128.0 * v)
                    }
                }
                2 => {
                    if v >= 0.0 {
                        convex(128.0 * v).min(top)
                    } else {
                        -convex(-128.0 * v)
                    }
                }
                _ => v,
            }
        }
    }
}

/// The output of a modulator: amount × source × amount source, made
/// absolute by transform 2.
pub(crate) fn value(m: &Modulator, input: &ModInput<'_>) -> f64 {
    if input.mute_fluid_vel_to_filter && is_fluid_vel_to_filter(m) {
        return 0.0;
    }
    let mut v1 = mapped(m.source, input);
    // FluidSynth 2.6: the modulation wheel's depth to the LFOs' pitch follows
    // RPN 5 (modulation depth range), 50 cents being the default.
    if m.source.is_cc()
        && m.source.index() == 1
        && (m.destination == VIB_LFO_TO_PITCH || m.destination == MOD_LFO_TO_PITCH)
    {
        v1 *= input.mod_depth_range / 50.0;
    }
    let v2 = mapped(m.amount_source, input);
    let out = f64::from(m.amount) * v1 * v2;
    if m.transform == 2 { out.abs() } else { out }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::soundfont::DEFAULT_MODULATORS;

    fn input(cc: &[u8; 128]) -> ModInput<'_> {
        ModInput {
            cc,
            key: 60,
            velocity: 127,
            poly_pressure: 0,
            channel_pressure: 0,
            pitch_bend: 8192,
            bend_range: 2.0,
            mod_depth_range: 50.0,
            mute_fluid_vel_to_filter: true,
        }
    }

    #[test]
    fn default_modulators_at_rest() {
        let mut cc = [0u8; 128];
        cc[7] = 127;
        cc[11] = 127;
        cc[10] = 64;
        let i = input(&cc);
        for m in DEFAULT_MODULATORS {
            let v = value(m, &i);
            assert!(v.abs() < 1e-9, "{m:?} gives {v}");
        }
    }

    #[test]
    fn velocity_to_attenuation_follows_the_concave_curve() {
        let cc = [0u8; 128];
        let mut i = input(&cc);
        i.velocity = 1;
        let v = value(&DEFAULT_MODULATORS[0], &i);
        // concave(126) times 960.
        assert!((v - 960.0 * concave(126.0)).abs() < 1e-9);
        assert!(v > 800.0 && v < 900.0, "{v}");
    }

    #[test]
    fn pitch_wheel_to_fine_tune() {
        let cc = [0u8; 128];
        let mut i = input(&cc);
        i.pitch_bend = 16383;
        let up = value(&DEFAULT_MODULATORS[8], &i);
        // FluidSynth: 12700 × 16383/16384 × 2/128.
        assert!((up - 12700.0 * 16383.0 / 16384.0 * 2.0 / 128.0).abs() < 1e-9, "{up}");
        i.pitch_bend = 0;
        let down = value(&DEFAULT_MODULATORS[8], &i);
        assert!((down + 12700.0 * 2.0 / 128.0).abs() < 1e-9, "{down}");
    }

    #[test]
    fn pan_maps_the_whole_range() {
        let mut cc = [0u8; 128];
        let m = &DEFAULT_MODULATORS[4];
        cc[10] = 0;
        assert_eq!(value(m, &input(&cc)), -500.0);
        cc[10] = 127;
        assert!((value(m, &input(&cc)) - 500.0 * 127.0 / 128.0).abs() < 1e-9);
    }

    #[test]
    fn fluid_vel_to_filter_is_muted_when_asked() {
        let cc = [0u8; 128];
        let mut i = input(&cc);
        i.velocity = 100;
        assert_eq!(value(&FLUID_VEL_TO_FILTER, &i), 0.0);
        i.mute_fluid_vel_to_filter = false;
        assert!((value(&FLUID_VEL_TO_FILTER, &i) + 2400.0 * (1.0 - 1.0 / 128.0 - 100.0 / 128.0)).abs() < 1e-9);
        i.velocity = 63;
        assert_eq!(value(&FLUID_VEL_TO_FILTER, &i), 0.0);
    }
}
