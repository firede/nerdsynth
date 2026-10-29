//! What each metric is, and how it is judged.

/// How a metric is judged against FluidSynth's value.
#[derive(Clone, Copy, Debug)]
pub enum Judge {
    /// Quality, lower is better: ours ≤ FS + tolerance.
    Lower(f64),
    /// Quality, lower is better, the tolerance a fraction of FS's value
    /// with a floor: ours ≤ FS + max(abs, rel·FS).
    LowerRel { abs: f64, rel: f64 },
    /// Quality, higher is better: ours ≥ FS − tolerance.
    Higher(f64),
    /// Must hold for ours, whatever FS does: ours ≤ limit.
    AtMost(f64),
    /// Must hold for ours: ours ≥ limit.
    AtLeast(f64),
    /// Balance: |ours| ≤ max(limit, |FS| + 0.5).
    Balance(f64),
    /// Character: |ours − FS| ≤ tolerance.
    Near(f64),
    /// Character: ours / FS within 1 ± tolerance.
    NearRel(f64),
    /// Character: |log2(ours / FS)| ≤ octaves.
    NearOct(f64),
    /// Reported only.
    Report,
}

impl Judge {
    pub fn pass(self, fs: f64, ours: f64) -> Option<bool> {
        if !ours.is_finite() {
            return match self {
                Judge::Report => None,
                _ => Some(false),
            };
        }
        let fs_ok = fs.is_finite();
        Some(match self {
            Judge::Lower(t) => !fs_ok || ours <= fs + t,
            Judge::LowerRel { abs, rel } => !fs_ok || ours <= fs + abs.max(rel * fs.abs()),
            Judge::Higher(t) => !fs_ok || ours >= fs - t,
            Judge::AtMost(l) => ours <= l,
            Judge::AtLeast(l) => ours >= l,
            Judge::Balance(l) => ours.abs() <= l.max(if fs_ok { fs.abs() + 0.5 } else { 0.0 }),
            Judge::Near(t) => fs_ok && (ours - fs).abs() <= t,
            Judge::NearRel(t) => fs_ok && (ours / fs - 1.0).abs() <= t,
            Judge::NearOct(t) => fs_ok && (ours / fs).log2().abs() <= t,
            Judge::Report => return None,
        })
    }

    pub fn rule(self) -> String {
        match self {
            Judge::Lower(t) => format!("≤ FS + {t}"),
            Judge::LowerRel { abs, rel } => format!("≤ FS + max({abs}, {:.0} %)", rel * 100.0),
            Judge::Higher(t) => format!("≥ FS − {t}"),
            Judge::AtMost(l) => format!("≤ {l}"),
            Judge::AtLeast(l) => format!("≥ {l}"),
            Judge::Balance(l) => format!("|·| ≤ max({l}, |FS| + 0.5)"),
            Judge::Near(t) => format!("FS ± {t}"),
            Judge::NearRel(t) => format!("FS ± {:.0} %", t * 100.0),
            Judge::NearOct(t) => format!("FS ± {t} oct"),
            Judge::Report => "—".into(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Quality,
    Character,
    Must,
    Info,
}

pub struct Spec {
    pub label: &'static str,
    pub unit: &'static str,
    pub kind: Kind,
    pub judge: Judge,
}

/// The spec of a metric key; band metrics (`name.<Hz>`) share their
/// name's.
pub fn spec(key: &str) -> Spec {
    use Judge::*;
    use Kind::*;
    let band = key.rsplit('.').next().is_some_and(|s| s.parse::<f64>().is_ok());
    let base = if band { &key[..key.rfind('.').unwrap()] } else { key };
    let s = |label, unit, kind, judge| Spec { label, unit, kind, judge };
    match base {
        "rev.mixing_time" => {
            s("echo density: mixing time (NED ≥ 0.95, from onset)", "ms", Quality, LowerRel { abs: 5.0, rel: 0.1 })
        }
        "rev.ned_late" => s("echo density: mean NED 150–500 ms", "", Quality, Higher(0.03)),
        "rev.roughness" => s("coloration: tail spectrum roughness", "dB", Quality, Lower(0.3)),
        "rev.tone_spread" => s("coloration: tone tail level spread (96 tones)", "dB", Quality, Lower(0.5)),
        "rev.tone_peak" => s("coloration: tone tail level, 95th percentile over median", "dB", Quality, Lower(0.5)),
        "rev.tone_beat" => s("decay smoothness: tone tail beating (10 ms)", "dB", Quality, Lower(0.3)),
        "rev.edc_dev" if band => s("decay smoothness: EDC deviation", "dB", Info, Report),
        "rev.edc_dev" => s("decay smoothness: EDC deviation, mean of bands", "dB", Quality, Lower(0.2)),
        "rev.fluct" if band => s("decay smoothness: 20 ms fluctuation", "dB", Info, Report),
        "rev.fluct" => s("decay smoothness: 20 ms fluctuation, mean of bands", "dB", Quality, Lower(0.3)),
        "rev.iacc" => s("stereo: late IACC", "", Quality, Lower(0.05)),
        "rev.rho0" => s("stereo: late L/R correlation at lag 0", "", Info, Report),
        "rev.balance" => s("stereo: L/R energy", "dB", Must, Balance(1.0)),
        "rev.cut" => s("clean ending: level where the tail stops", "dBFS", Must, AtMost(-120.0)),
        "rev.t30" if band => s("RT60 (T30)", "s", Character, NearRel(0.25)),
        "rev.t30" => s("RT60 (T30), broadband", "s", Character, NearRel(0.2)),
        "rev.tail_width" => {
            s("tone tails: spectral spread about the tone (≥ 600 Hz)", "cents", Character, NearRel(0.5))
        }
        "rev.edt" => s("early decay time", "s", Character, NearRel(0.35)),
        "rev.predelay" => s("onset (first arrival at −26 dB)", "ms", Character, Near(15.0)),
        "rev.level" => s("wet level (energy re dry)", "dB", Character, Near(2.0)),
        "rev.centroid" => s("tail spectral centroid", "Hz", Character, NearOct(0.3)),
        "cho.snr" => s("cleanliness: SNR outside the modulation band", "dB", Quality, Higher(3.0)),
        "cho.iacc" => s("stereo: IACC (noise)", "", Quality, Lower(0.05)),
        "cho.rho0" => s("stereo: L/R correlation at lag 0 (not phasey)", "", Must, AtLeast(-0.3)),
        "cho.balance" => s("stereo: L/R energy", "dB", Must, Balance(1.0)),
        "cho.mono" => s("stereo: mono sum re stereo", "dB", Info, Report),
        "cho.cut" => s("output end after the click train (follows its input)", "dBFS", Info, Report),
        "cho.rate" => s("modulation rate", "Hz", Character, NearRel(0.35)),
        "cho.depth" => s("delay swing, peak to peak", "ms", Character, NearRel(0.3)),
        "cho.delay_min" => s("shortest delay", "ms", Info, Report),
        "cho.pitch_dev" => s("pitch deviation of a sine (peak)", "cents", Character, NearRel(0.4)),
        "cho.level" => s("wet level (energy re dry, noise)", "dB", Character, Near(2.0)),
        "cho.voices_seen" => s("voices seen in the click train", "", Info, Report),
        _ => s("?", "", Info, Report),
    }
}
