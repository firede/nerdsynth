//! Thresholds, calibrated from FluidSynth against SpessaSynth, and judging
//! results by them.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::cases::{Case, Group};
use crate::metrics::{CaseResult, Metric, median, percentile};

/// How a threshold comes from the calibration: `FACTOR` times the median
/// deviation of the cases the metric judges, at least the metric's floor.
/// The median is the typical difference of two mature implementations;
/// the cases far beyond it are where they disagree on a behavior.
pub const FACTOR: f64 = 3.0;

/// The gain offset (the median loudness difference over the dry cases) a
/// candidate may have. Not calibrated: NerdSynth means to match
/// FluidSynth's level at gain 1.
pub const MAX_GAIN_OFFSET_DB: f64 = 1.0;

/// The smallest threshold of each metric: below it, differences are
/// beneath what the metric resolves or what anyone hears.
pub fn floor(m: Metric) -> f64 {
    match m {
        Metric::EnvCorr => 0.002,
        Metric::EnvDevDb => 1.5,
        Metric::LoudnessDb => 0.5,
        Metric::F0Cents => 2.0,
        Metric::VibratoCents => 3.0,
        Metric::CentroidOct => 0.05,
        Metric::BandsDb => 1.0,
        Metric::OnsetFrames => 64.0,
        Metric::BalanceDb => 0.5,
        Metric::TailMs => 30.0,
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Stat {
    pub threshold: f64,
    /// The cases the metric judges.
    pub n: usize,
    pub p50: f64,
    pub p75: f64,
    pub p90: f64,
    pub max: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Thresholds {
    pub reference: String,
    pub candidate: String,
    pub rule: String,
    /// The calibration's gain offset (candidate over reference), in dB.
    pub gain_offset_db: f64,
    pub max_gain_offset_db: f64,
    pub groups: BTreeMap<Group, BTreeMap<Metric, Stat>>,
}

impl Thresholds {
    pub fn load(path: &Path) -> Result<Thresholds, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("{}: {e}; run: mise run diff:calibrate", path.display()))?;
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())? + "\n";
        std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn threshold(&self, group: Group, m: Metric) -> f64 {
        self.groups.get(&group).and_then(|g| g.get(&m)).map_or(floor(m), |s| s.threshold)
    }
}

/// The worst segment of a case for one metric.
#[derive(Clone, Debug, Serialize)]
pub struct Worst {
    pub metric: Metric,
    pub segment: String,
    pub value: f64,
    pub deviation: f64,
    pub detail: String,
}

/// The median loudness difference over the dry cases.
pub fn gain_offset(cases: &[&Case], results: &[CaseResult]) -> f64 {
    let v: Vec<f64> =
        cases.iter().zip(results).filter(|(c, _)| c.group == Group::Dry).map(|(_, r)| r.loudness_db).collect();
    median(&v)
}

/// For each metric the case measured, its worst segment.
pub fn worst(result: &CaseResult, gain_offset: f64) -> BTreeMap<Metric, Worst> {
    let mut w: BTreeMap<Metric, Worst> = BTreeMap::new();
    for s in &result.segments {
        for (&m, v) in &s.values {
            let deviation = m.deviation(v.value, gain_offset);
            if w.get(&m).is_none_or(|x| deviation > x.deviation) {
                w.insert(
                    m,
                    Worst { metric: m, segment: s.label.clone(), value: v.value, deviation, detail: v.detail.clone() },
                );
            }
        }
    }
    w
}

/// Thresholds from a calibration run.
pub fn calibrate(cases: &[&Case], results: &[CaseResult], reference: String, candidate: String) -> Thresholds {
    let offset = gain_offset(cases, results);
    let mut groups: BTreeMap<Group, BTreeMap<Metric, Stat>> = BTreeMap::new();
    for group in [Group::Dry, Group::Fx] {
        let mut stats = BTreeMap::new();
        for m in Metric::ALL {
            let devs: Vec<f64> = cases
                .iter()
                .zip(results)
                .filter(|(c, _)| c.group == group && c.focus.contains(&m))
                .filter_map(|(_, r)| worst(r, offset).get(&m).map(|w| w.deviation))
                .collect();
            if devs.is_empty() {
                continue;
            }
            let p50 = median(&devs);
            stats.insert(
                m,
                Stat {
                    threshold: round_up(floor(m).max(FACTOR * p50)),
                    n: devs.len(),
                    p50,
                    p75: percentile(&devs, 0.75),
                    p90: percentile(&devs, 0.9),
                    max: percentile(&devs, 1.0),
                },
            );
        }
        groups.insert(group, stats);
    }
    Thresholds {
        reference,
        candidate,
        rule: format!(
            "threshold = max(floor, {FACTOR} × median deviation of the cases the metric judges), rounded up to 2 significant digits"
        ),
        gain_offset_db: offset,
        max_gain_offset_db: MAX_GAIN_OFFSET_DB,
        groups,
    }
}

/// Rounds up to two significant digits.
fn round_up(x: f64) -> f64 {
    if x <= 0.0 {
        return x;
    }
    let e = 10f64.powi(x.log10().floor() as i32 - 1);
    // The last rounding takes off the binary noise of the multiplication.
    ((x / e - 1e-9).ceil() * e * 1e9).round() / 1e9
}

/// A focus metric beyond its threshold.
#[derive(Clone, Debug, Serialize)]
pub struct Failure {
    pub threshold: f64,
    #[serde(flatten)]
    pub worst: Worst,
}

/// The focus metrics of `case` beyond their thresholds.
pub fn failures(case: &Case, result: &CaseResult, gain_offset: f64, t: &Thresholds) -> Vec<Failure> {
    let w = worst(result, gain_offset);
    case.focus
        .iter()
        .filter_map(|m| w.get(m))
        .filter_map(|x| {
            let threshold = t.threshold(case.group, x.metric);
            (x.deviation > threshold).then(|| Failure { threshold, worst: x.clone() })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounds_up() {
        assert_eq!(round_up(0.123), 0.13);
        assert_eq!(round_up(12.0), 12.0);
        assert_eq!(round_up(1234.0), 1300.0);
    }
}
