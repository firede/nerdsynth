//! Comparing a candidate rendering with a reference one, segment by
//! segment, so that a difference shows where it is and in what.

use std::collections::BTreeMap;
use std::f64::consts::PI;

use serde::{Deserialize, Serialize};

use crate::cases::{Case, Pitch, Segment};
use crate::wav::Audio;

/// What is compared. Each metric has a value (candidate against reference,
/// signed where that means something) and a deviation, the non-negative
/// number thresholds bound.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Metric {
    /// Pearson correlation of the 10 ms RMS envelopes in dB (each clamped
    /// at 60 dB below the louder peak). Deviation: 1 - correlation.
    EnvCorr,
    /// The largest difference of the two dB envelopes, after taking out
    /// their median difference: how far the shapes part, anywhere.
    EnvDevDb,
    /// The energy of the candidate over the reference's, in dB. Deviation:
    /// distance from the run's overall gain offset (the median over all
    /// dry cases), so that a different master gain is judged once, not in
    /// every case.
    LoudnessDb,
    /// The median pitch of the candidate minus the reference's, in cents.
    F0Cents,
    /// Vibrato depth (half the 10th to 90th percentile spread of the pitch
    /// track) of the candidate minus the reference's, in cents.
    VibratoCents,
    /// log2 of the candidate's spectral centroid over the reference's, in
    /// octaves (+0.1 is about 7 % brighter).
    CentroidOct,
    /// The largest difference, in dB, of the share of energy in one of five
    /// bands (below 250 Hz, to 1k, to 4k, to 16k, above).
    BandsDb,
    /// When the candidate first reaches 30 dB below its peak in the
    /// segment, minus when the reference does, in frames.
    OnsetFrames,
    /// The left/right energy ratio of the candidate minus the reference's,
    /// in dB (positive: the candidate is further left).
    BalanceDb,
    /// When the candidate last is within 60 dB of its peak in the segment,
    /// minus when the reference is, in ms.
    TailMs,
}

pub use Metric::*;

impl Metric {
    pub const ALL: [Metric; 10] =
        [EnvCorr, EnvDevDb, LoudnessDb, F0Cents, VibratoCents, CentroidOct, BandsDb, OnsetFrames, BalanceDb, TailMs];

    pub fn name(self) -> &'static str {
        match self {
            EnvCorr => "env_corr",
            EnvDevDb => "env_dev_db",
            LoudnessDb => "loudness_db",
            F0Cents => "f0_cents",
            VibratoCents => "vibrato_cents",
            CentroidOct => "centroid_oct",
            BandsDb => "bands_db",
            OnsetFrames => "onset_frames",
            BalanceDb => "balance_db",
            TailMs => "tail_ms",
        }
    }

    /// The deviation of `value`; `gain_offset` is the run's overall
    /// loudness difference.
    pub fn deviation(self, value: f64, gain_offset: f64) -> f64 {
        match self {
            EnvCorr => 1.0 - value,
            LoudnessDb => (value - gain_offset).abs(),
            _ => value.abs(),
        }
    }
}

// Short names for case definitions.
pub use Metric::{
    BalanceDb as Balance, BandsDb as Bands, CentroidOct as Centroid, EnvDevDb as EnvDev, F0Cents as F0,
    LoudnessDb as Loudness, OnsetFrames as Onset, TailMs as Tail, VibratoCents as Vibrato,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Value {
    pub value: f64,
    /// What the value comes from, to read when it is off.
    pub detail: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SegmentResult {
    pub label: String,
    pub start_ms: u32,
    pub end_ms: u32,
    pub values: BTreeMap<Metric, Value>,
    /// Why some metrics are missing.
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CaseResult {
    pub id: String,
    /// The loudness difference of the whole rendering, in dB.
    pub loudness_db: f64,
    pub segments: Vec<SegmentResult>,
}

const SILENT_DB: f64 = -100.0;

/// The analysis of one rendering, shared by the segments.
struct Analysis<'a> {
    audio: &'a Audio,
    /// Power (mean of the squares of both channels) per 10 ms frame, in dB.
    env: Vec<f64>,
    hop: usize,
}

impl<'a> Analysis<'a> {
    fn new(audio: &'a Audio) -> Analysis<'a> {
        let hop = (audio.rate / 100) as usize;
        let env = (0..audio.frames() / hop)
            .map(|i| {
                let r = i * hop..(i + 1) * hop;
                let p: f64 = r.map(|j| sq(audio.left[j]) + sq(audio.right[j])).sum::<f64>() / (2 * hop) as f64;
                db(p)
            })
            .collect();
        Analysis { audio, env, hop }
    }

    fn frames(&self, s: &Segment) -> std::ops::Range<usize> {
        let a = (s.start_ms / 10) as usize;
        let b = ((s.end_ms / 10) as usize).min(self.env.len());
        a.min(b)..b
    }

    fn samples(&self, from_ms: u32, to_ms: u32) -> std::ops::Range<usize> {
        let n = self.audio.frames();
        let a = (from_ms as u64 * self.audio.rate as u64 / 1000) as usize;
        let b = ((to_ms as u64 * self.audio.rate as u64 / 1000) as usize).min(n);
        a.min(b)..b
    }

    fn peak_db(&self, s: &Segment) -> f64 {
        self.env[self.frames(s)].iter().copied().fold(f64::NEG_INFINITY, f64::max)
    }
}

fn sq(x: f32) -> f64 {
    (x as f64) * (x as f64)
}

fn db(power: f64) -> f64 {
    10.0 * (power + 1e-30).log10()
}

/// Compares `cand` with `reference`, rendered from `case`; both must have
/// the same rate.
pub fn compare(case: &Case, reference: &Audio, cand: &Audio) -> CaseResult {
    let a = Analysis::new(reference);
    let b = Analysis::new(cand);
    let energy = |x: &Audio| x.left.iter().chain(&x.right).map(|&v| sq(v)).sum::<f64>();
    let loudness_db = (db(energy(cand)) - db(energy(reference))).clamp(-120.0, 120.0);
    let segments = case.segments.iter().map(|s| segment(s, &a, &b)).collect();
    CaseResult { id: case.id.clone(), loudness_db, segments }
}

fn segment(s: &Segment, a: &Analysis, b: &Analysis) -> SegmentResult {
    let mut r = SegmentResult {
        label: s.label.clone(),
        start_ms: s.start_ms,
        end_ms: s.end_ms,
        values: BTreeMap::new(),
        notes: Vec::new(),
    };
    let mut put = |m: Metric, value: f64, detail: String| {
        r.values.insert(m, Value { value, detail });
    };
    let (pa, pb) = (a.peak_db(s), b.peak_db(s));
    let frames = a.frames(s);
    if frames.is_empty() {
        r.notes.push("segment beyond the rendering".into());
        return r;
    }
    if pa < SILENT_DB && pb < SILENT_DB {
        r.notes.push("both silent".into());
        return r;
    }
    let ea: f64 = a.env[frames.clone()].iter().map(|&d| 10f64.powf(d / 10.0)).sum();
    let eb: f64 = b.env[frames.clone()].iter().map(|&d| 10f64.powf(d / 10.0)).sum();
    let loud = (db(eb) - db(ea)).clamp(-120.0, 120.0);
    put(LoudnessDb, loud, format!("peak ref {pa:.1} dB, cand {pb:.1} dB"));
    if pa < SILENT_DB || pb < SILENT_DB {
        r.notes.push(if pa < SILENT_DB { "reference silent".into() } else { "candidate silent".into() });
        return r;
    }

    // Envelopes.
    let floor = pa.max(pb) - 60.0;
    let (mut xs, mut ys, mut ts) = (Vec::new(), Vec::new(), Vec::new());
    for i in frames.clone() {
        let (x, y) = (a.env[i].max(floor), b.env[i].max(floor));
        if x > floor || y > floor {
            xs.push(x);
            ys.push(y);
            ts.push(i as u32 * 10);
        }
    }
    if xs.len() >= 3 {
        put(EnvCorr, pearson(&xs, &ys), format!("{} frames above {floor:.0} dB", xs.len()));
        let d: Vec<f64> = xs.iter().zip(&ys).map(|(x, y)| y - x).collect();
        let med = median(&d);
        let (i, dev) =
            d.iter().map(|v| v - med).enumerate().max_by(|p, q| p.1.abs().total_cmp(&q.1.abs())).unwrap_or((0, 0.0));
        put(
            EnvDevDb,
            dev,
            format!("at {} ms (ref {:.1} dB, cand {:.1} dB), median offset {med:+.1} dB", ts[i], xs[i], ys[i]),
        );
    } else {
        r.notes.push("too short for envelopes".into());
    }

    // Onset and tail.
    let samples = a.samples(s.start_ms, s.end_ms);
    let onset = |x: &Analysis| {
        let au = x.audio;
        let amp = |j: usize| au.left[j].abs().max(au.right[j].abs());
        let peak = samples.clone().map(amp).fold(0.0f32, f32::max);
        let thr = peak * 10f32.powf(-30.0 / 20.0);
        samples.clone().find(|&j| amp(j) >= thr).unwrap_or(samples.start)
    };
    let (oa, ob) = (onset(a), onset(b));
    put(OnsetFrames, ob as f64 - oa as f64, format!("ref at {:.1} ms", oa as f64 * 1000.0 / a.audio.rate as f64));
    let tail = |x: &Analysis, peak: f64| {
        let last = frames.clone().rev().find(|&i| x.env[i] > peak - 60.0).unwrap_or(frames.start);
        (last + 1) as f64 * x.hop as f64 * 1000.0 / x.audio.rate as f64
    };
    let (ta, tb) = (tail(a, pa), tail(b, pb));
    put(TailMs, tb - ta, format!("ref ends at {ta:.0} ms, cand at {tb:.0} ms"));

    // Stereo balance.
    let balance = |x: &Analysis| {
        let l: f64 = x.audio.left[samples.clone()].iter().map(|&v| sq(v)).sum();
        let r: f64 = x.audio.right[samples.clone()].iter().map(|&v| sq(v)).sum();
        (db(l) - db(r)).clamp(-60.0, 60.0)
    };
    let (ba, bb) = (balance(a), balance(b));
    put(BalanceDb, bb - ba, format!("L/R ref {ba:+.2} dB, cand {bb:+.2} dB"));

    // Spectrum.
    let (sa, sb) = (spectrum(a.audio, samples.clone()), spectrum(b.audio, samples.clone()));
    if sa.centroid > 0.0 && sb.centroid > 0.0 {
        put(
            CentroidOct,
            (sb.centroid / sa.centroid).log2(),
            format!("ref {:.0} Hz, cand {:.0} Hz (ratio {:.3})", sa.centroid, sb.centroid, sb.centroid / sa.centroid),
        );
        let ta: f64 = sa.bands.iter().sum();
        let tb: f64 = sb.bands.iter().sum();
        let mut worst: Option<(usize, f64, f64, f64)> = None;
        for k in 0..BANDS.len() {
            let (x, y) = (db(sa.bands[k] / ta), db(sb.bands[k] / tb));
            if x.max(y) < -40.0 {
                continue;
            }
            let d = y.max(-60.0) - x.max(-60.0);
            if worst.is_none_or(|w| d.abs() > w.1.abs()) {
                worst = Some((k, d, x, y));
            }
        }
        if let Some((k, d, x, y)) = worst {
            put(BandsDb, d, format!("band {}: ref {x:.1} dB, cand {y:.1} dB of the total", BANDS[k].0));
        }
    }

    // Pitch.
    if let Some(p) = &s.pitch {
        match (pitch_track(a.audio, p), pitch_track(b.audio, p)) {
            (Some(ta), Some(tb)) => {
                let (ma, mb) = (median(&ta), median(&tb));
                put(F0Cents, mb - ma, format!("vs key {:.2}: ref {ma:+.1} c, cand {mb:+.1} c", p.key));
                let (da, dbb) = (depth(&ta), depth(&tb));
                put(VibratoCents, dbb - da, format!("depth ref {da:.1} c, cand {dbb:.1} c"));
            }
            (ta, tb) => r.notes.push(format!(
                "no steady pitch in {}–{} ms (ref {}, cand {})",
                p.from_ms,
                p.to_ms,
                if ta.is_some() { "ok" } else { "none" },
                if tb.is_some() { "ok" } else { "none" }
            )),
        }
    }
    r
}

fn pearson(x: &[f64], y: &[f64]) -> f64 {
    let n = x.len() as f64;
    let (mx, my) = (x.iter().sum::<f64>() / n, y.iter().sum::<f64>() / n);
    let (mut sxy, mut sxx, mut syy) = (0.0, 0.0, 0.0);
    for (a, b) in x.iter().zip(y) {
        sxy += (a - mx) * (b - my);
        sxx += (a - mx) * (a - mx);
        syy += (b - my) * (b - my);
    }
    // Flat envelopes (a held organ note) correlate when both are flat.
    let flat = 1e-6 * n;
    match (sxx < flat, syy < flat) {
        (true, true) => 1.0,
        (true, false) | (false, true) => 0.0,
        _ => sxy / (sxx * syy).sqrt(),
    }
}

pub fn median(v: &[f64]) -> f64 {
    percentile(v, 0.5)
}

/// The `q` quantile (0 to 1) of `v`, interpolated; 0 when empty.
pub fn percentile(v: &[f64], q: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    let mut s = v.to_vec();
    s.sort_by(f64::total_cmp);
    let x = q * (s.len() - 1) as f64;
    let (i, f) = (x.floor() as usize, x.fract());
    if i + 1 < s.len() { s[i] * (1.0 - f) + s[i + 1] * f } else { s[i] }
}

fn depth(track: &[f64]) -> f64 {
    (percentile(track, 0.9) - percentile(track, 0.1)) / 2.0
}

const BANDS: [(&str, f64); 5] =
    [("<250 Hz", 250.0), ("250–1k", 1000.0), ("1k–4k", 4000.0), ("4k–16k", 16000.0), (">16k", f64::INFINITY)];

struct Spectrum {
    /// Energy-weighted mean of the frames' centroids, in Hz; 0 if silent.
    centroid: f64,
    bands: [f64; 5],
}

const FFT: usize = 2048;

fn spectrum(audio: &Audio, range: std::ops::Range<usize>) -> Spectrum {
    let hann: Vec<f64> = (0..FFT).map(|i| 0.5 - 0.5 * (2.0 * PI * i as f64 / FFT as f64).cos()).collect();
    let bin_hz = audio.rate as f64 / FFT as f64;
    let mut frames = Vec::new();
    let mut start = range.start;
    while start + FFT <= range.end {
        let mut re: Vec<f64> =
            (0..FFT).map(|i| hann[i] * 0.5 * (audio.left[start + i] as f64 + audio.right[start + i] as f64)).collect();
        let mut im = vec![0.0; FFT];
        fft(&mut re, &mut im);
        let power: Vec<f64> = (0..FFT / 2).map(|k| re[k] * re[k] + im[k] * im[k]).collect();
        frames.push(power);
        start += FFT / 4;
    }
    let energy: Vec<f64> = frames.iter().map(|p| p[1..].iter().sum()).collect();
    let max = energy.iter().copied().fold(0.0, f64::max);
    let mut s = Spectrum { centroid: 0.0, bands: [0.0; 5] };
    if max <= 1e-20 {
        return s;
    }
    let (mut wsum, mut csum) = (0.0, 0.0);
    for (p, &e) in frames.iter().zip(&energy) {
        if e < max * 1e-4 {
            continue;
        }
        let c: f64 = p.iter().enumerate().skip(1).map(|(k, &v)| k as f64 * bin_hz * v).sum::<f64>() / e;
        wsum += e;
        csum += c * e;
        for (k, &v) in p.iter().enumerate().skip(1) {
            let f = k as f64 * bin_hz;
            let band = BANDS.iter().position(|b| f < b.1).unwrap_or(BANDS.len() - 1);
            s.bands[band] += v;
        }
    }
    s.centroid = csum / wsum;
    s
}

/// In-place radix-2 FFT; the length must be a power of two.
fn fft(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = -2.0 * PI / len as f64;
        let (wr, wi) = (ang.cos(), ang.sin());
        for start in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0, 0.0);
            for k in 0..len / 2 {
                let (a, b) = (start + k, start + k + len / 2);
                let tr = re[b] * cr - im[b] * ci;
                let ti = re[b] * ci + im[b] * cr;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
                let ncr = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = ncr;
            }
        }
        len <<= 1;
    }
}

/// The pitch, in cents from `p.key`, of each 1024-frame window (hop 256)
/// of `p`'s span that is periodic; `None` when fewer than three are.
///
/// The measure is the normalized squared difference of the window and
/// itself shifted by a lag, searched within 15 semitones of the expected
/// period (presets do sound an octave off: GeneralUser's organ plays a 16'
/// stop). The pitch is the shortest lag that is a local minimum within
/// 0.05 of the least one, so that multiples of the period lose to it;
/// parabolic interpolation refines it.
fn pitch_track(audio: &Audio, p: &Pitch) -> Option<Vec<f64>> {
    const W: usize = 1024;
    const HOP: usize = 256;
    let rate = audio.rate as f64;
    let expect = 440.0 * 2f64.powf((p.key - 69.0) / 12.0);
    let period = rate / expect;
    let span = 2f64.powf(15.0 / 12.0);
    let tmin = ((period / span).floor() as usize).max(2);
    let tmax = (period * span).ceil() as usize;
    let a = (p.from_ms as u64 * audio.rate as u64 / 1000) as usize;
    let b = ((p.to_ms as u64 * audio.rate as u64 / 1000) as usize).min(audio.frames());
    let x: Vec<f64> = (a..b).map(|i| 0.5 * (audio.left[i] as f64 + audio.right[i] as f64)).collect();
    let peak = x.iter().fold(0.0f64, |m, v| m.max(v.abs()));
    if peak < 1e-5 {
        return None;
    }
    let mut track = Vec::new();
    let mut start = 0;
    while start + W + tmax + 1 < x.len() {
        let w = &x[start..start + W + tmax + 2];
        let d = |tau: usize| {
            let (mut num, mut den) = (0.0, 0.0);
            for j in 0..W {
                let (u, v) = (w[j], w[j + tau]);
                num += (u - v) * (u - v);
                den += u * u + v * v;
            }
            if den > 0.0 { num / den } else { 1.0 }
        };
        let ds: Vec<f64> = (tmin - 1..=tmax + 1).map(d).collect();
        // ds[i] is the lag tmin - 1 + i; search tmin..=tmax.
        let least = ds[1..ds.len() - 1].iter().copied().fold(f64::INFINITY, f64::min);
        let pick = (1..ds.len() - 1)
            .find(|&i| ds[i] <= ds[i - 1] && ds[i] <= ds[i + 1] && ds[i] <= least + 0.05 && ds[i] < 0.3);
        if let Some(i) = pick {
            let (l, c, r) = (ds[i - 1], ds[i], ds[i + 1]);
            let den = l - 2.0 * c + r;
            let shift = if den > 0.0 { (0.5 * (l - r) / den).clamp(-0.5, 0.5) } else { 0.0 };
            let tau = (tmin - 1 + i) as f64 + shift;
            track.push(1200.0 * (rate / tau / expect).log2());
        }
        start += HOP;
    }
    (track.len() >= 3).then_some(track)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(rate: u32, hz: f64, secs: f64, amp: f32, pan: f32) -> Audio {
        let n = (rate as f64 * secs) as usize;
        let mut a = Audio::silent(rate, n);
        for i in 0..n {
            let v = amp * (2.0 * PI * hz * i as f64 / rate as f64).sin() as f32;
            a.left[i] = v * (1.0 - pan);
            a.right[i] = v * pan;
        }
        a
    }

    #[test]
    fn pitch_of_a_sine() {
        let a = tone(44100, 261.6256 * 2f64.powf(10.0 / 1200.0), 1.0, 0.5, 0.5);
        let t = pitch_track(&a, &Pitch { key: 60.0, from_ms: 100, to_ms: 900 }).unwrap();
        assert!((median(&t) - 10.0).abs() < 0.5, "{}", median(&t));
    }

    #[test]
    fn fft_of_a_bin() {
        let n = 64;
        let mut re: Vec<f64> = (0..n).map(|i| (2.0 * PI * 5.0 * i as f64 / n as f64).cos()).collect();
        let mut im = vec![0.0; n];
        fft(&mut re, &mut im);
        assert!((re[5] - n as f64 / 2.0).abs() < 1e-9);
        assert!(re[6].abs() < 1e-9 && im[5].abs() < 1e-9);
    }

    #[test]
    fn same_audio_compares_equal() {
        use crate::cases::{Group, Sf2};
        let a = tone(44100, 440.0, 1.0, 0.3, 0.3);
        let case = Case {
            id: "t".into(),
            category: "t",
            desc: String::new(),
            sf2: Sf2::GeneralUser,
            group: Group::Dry,
            polyphony: 1,
            dur_ms: 1000,
            events: Vec::new(),
            segments: vec![Segment {
                label: "all".into(),
                start_ms: 0,
                end_ms: 1000,
                pitch: Some(Pitch { key: 69.0, from_ms: 100, to_ms: 900 }),
            }],
            focus: Vec::new(),
        };
        let r = compare(&case, &a, &a);
        let v = &r.segments[0].values;
        assert_eq!(v[&EnvCorr].value, 1.0);
        for m in [EnvDevDb, LoudnessDb, F0Cents, CentroidOct, BandsDb, OnsetFrames, BalanceDb, TailMs] {
            assert!(v[&m].value.abs() < 1e-9, "{m:?}");
        }
        // Louder by 6 dB and to the left.
        let mut b = a.clone();
        b.left.iter_mut().for_each(|x| *x *= 2.0);
        let r = compare(&case, &a, &b);
        assert!(r.segments[0].values[&BalanceDb].value > 5.9);
    }
}
