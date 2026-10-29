//! Markdown reports of a comparison.

use std::collections::BTreeMap;
use std::fmt::Write;

use crate::cases::{Case, Group};
use crate::judge::{self, Thresholds};
use crate::metrics::{CaseResult, Metric, median};

pub fn fmt_value(m: Metric, v: f64) -> String {
    match m {
        Metric::EnvCorr => format!("{v:.4}"),
        Metric::EnvDevDb | Metric::LoudnessDb | Metric::BandsDb | Metric::BalanceDb => format!("{v:+.2}"),
        Metric::F0Cents | Metric::VibratoCents => format!("{v:+.1}"),
        Metric::CentroidOct => format!("{v:+.3}"),
        Metric::OnsetFrames | Metric::TailMs => format!("{v:+.0}"),
    }
}

fn fmt_dev(m: Metric, v: f64) -> String {
    fmt_value(m, v).trim_start_matches('+').to_string()
}

/// The header lines: what was compared, and how.
pub struct Header {
    pub title: String,
    pub reference: String,
    pub candidate: String,
    pub settings: Vec<String>,
}

/// A comparison of every case: a summary, the cases beyond the thresholds
/// by category, the typical deviation of each category, and every value.
pub fn write(
    h: &Header,
    cases: &[&Case],
    results: &[CaseResult],
    t: &Thresholds,
    calibrating: bool,
    notes: Option<&str>,
) -> String {
    let mut o = String::new();
    let offset = judge::gain_offset(cases, results);
    let _ = writeln!(o, "# {}\n", h.title);
    let _ = writeln!(o, "- 参照：{}", h.reference);
    let _ = writeln!(o, "- 对象：{}", h.candidate);
    for s in &h.settings {
        let _ = writeln!(o, "- {s}");
    }
    let _ = writeln!(
        o,
        "- 用例：{} 个（无效果 {}，有效果 {}）",
        cases.len(),
        count(cases, Group::Dry),
        count(cases, Group::Fx)
    );
    let gain_ok = offset.abs() <= t.max_gain_offset_db;
    let _ = writeln!(
        o,
        "- 整体增益偏移（无效果用例整体响度差的中位数，对象 − 参照）：{offset:+.2} dB{}",
        if calibrating {
            String::new()
        } else {
            format!("，限值 ±{:.1} dB，{}", t.max_gain_offset_db, if gain_ok { "通过" } else { "**不通过**" })
        }
    );
    let _ = writeln!(o, "- 响度指标（loudness_db）的偏差是减去这个偏移之后的值。\n");

    // Thresholds.
    let _ = writeln!(o, "## 阈值\n");
    let _ = writeln!(o, "规则：{}。\n", t.rule);
    for (group, stats) in &t.groups {
        let _ = writeln!(o, "### {}\n", group_name(*group));
        let _ = writeln!(o, "| 指标 | 用例数 | 中位偏差 | p75 | p90 | 最大 | 下限 | 阈值 |");
        let _ = writeln!(o, "| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |");
        for (m, s) in stats {
            let _ = writeln!(
                o,
                "| {} | {} | {} | {} | {} | {} | {} | {} |",
                m.name(),
                s.n,
                fmt_dev(*m, s.p50),
                fmt_dev(*m, s.p75),
                fmt_dev(*m, s.p90),
                fmt_dev(*m, s.max),
                fmt_dev(*m, judge::floor(*m)),
                fmt_dev(*m, s.threshold)
            );
        }
        let _ = writeln!(o);
    }

    // Beyond the thresholds.
    let mut by_cat: BTreeMap<&str, Vec<(&Case, Vec<judge::Failure>)>> = BTreeMap::new();
    let mut failed = 0;
    for (c, r) in cases.iter().zip(results) {
        let f = judge::failures(c, r, offset, t);
        if !f.is_empty() {
            failed += 1;
            by_cat.entry(c.category).or_default().push((c, f));
        }
    }
    let _ = writeln!(
        o,
        "## {}（{failed} / {} 个用例）\n",
        if calibrating { "差异显著的用例" } else { "超出阈值的用例" },
        cases.len()
    );
    if calibrating {
        let _ = writeln!(
            o,
            "两个成熟实现在这些用例上的差异超过了阈值：规范没写死、需要我们做决定的地方多半在这里。每行是一个指标最差的那一段。\n"
        );
    }
    for (cat, list) in &by_cat {
        let _ = writeln!(o, "### {cat}\n");
        let _ = writeln!(o, "| 用例 | 指标 | 段 | 值 | 阈值 | 细节 |");
        let _ = writeln!(o, "| --- | --- | --- | ---: | ---: | --- |");
        for (c, fs) in list {
            for f in fs {
                let m = f.worst.metric;
                let _ = writeln!(
                    o,
                    "| `{}` | {} | {} | {} | {} | {} |",
                    c.id,
                    m.name(),
                    f.worst.segment,
                    fmt_value(m, f.worst.value),
                    fmt_dev(m, f.threshold),
                    f.worst.detail
                );
            }
        }
        let _ = writeln!(o);
    }

    // Typical deviation by category.
    let _ = writeln!(o, "## 各类用例的中位偏差\n");
    let _ = writeln!(o, "每类用例里，各指标（只算该类判定的指标）最差段偏差的中位数。\n");
    let mut cats: Vec<&str> = cases.iter().map(|c| c.category).collect();
    cats.dedup();
    let _ = write!(o, "| 类别 | 用例数 |");
    for m in Metric::ALL {
        let _ = write!(o, " {} |", m.name());
    }
    let _ = writeln!(o);
    let _ = writeln!(o, "| --- | ---: |{}", " ---: |".repeat(Metric::ALL.len()));
    for cat in cats {
        let idx: Vec<usize> = (0..cases.len()).filter(|&i| cases[i].category == cat).collect();
        let _ = write!(o, "| {cat} | {} |", idx.len());
        for m in Metric::ALL {
            let devs: Vec<f64> = idx
                .iter()
                .filter(|&&i| cases[i].focus.contains(&m))
                .filter_map(|&i| judge::worst(&results[i], offset).get(&m).map(|w| w.deviation))
                .collect();
            if devs.is_empty() {
                let _ = write!(o, " |");
            } else {
                let _ = write!(o, " {} |", fmt_dev(m, median(&devs)));
            }
        }
        let _ = writeln!(o);
    }
    let _ = writeln!(o);

    if let Some(notes) = notes {
        let _ = writeln!(o, "{}\n", notes.trim());
    }

    // Everything.
    let _ = writeln!(o, "## 附录：全部用例\n");
    let _ = writeln!(o, "每个用例判定的指标，取最差的段；超出阈值的加粗。整体响度差是整段渲染的。\n");
    for (c, r) in cases.iter().zip(results) {
        let w = judge::worst(r, offset);
        let _ = writeln!(o, "#### `{}`\n", c.id);
        let _ = writeln!(o, "{}；整体响度差 {:+.2} dB。\n", c.desc, r.loudness_db);
        let _ = writeln!(o, "| 指标 | 值 | 偏差 | 阈值 | 段 | 细节 |");
        let _ = writeln!(o, "| --- | ---: | ---: | ---: | --- | --- |");
        for m in &c.focus {
            let thr = t.threshold(c.group, *m);
            match w.get(m) {
                Some(x) => {
                    let mark = if x.deviation > thr { "**" } else { "" };
                    let _ = writeln!(
                        o,
                        "| {} | {mark}{}{mark} | {} | {} | {} | {} |",
                        m.name(),
                        fmt_value(*m, x.value),
                        fmt_dev(*m, x.deviation),
                        fmt_dev(*m, thr),
                        x.segment,
                        x.detail
                    );
                }
                None => {
                    let _ = writeln!(o, "| {} | – | | {} | | 没有测到 |", m.name(), fmt_dev(*m, thr));
                }
            }
        }
        let notes: Vec<String> =
            r.segments.iter().flat_map(|s| s.notes.iter().map(move |n| format!("{}：{n}", s.label))).collect();
        if !notes.is_empty() {
            let _ = writeln!(o, "\n{}", notes.join("；"));
        }
        let _ = writeln!(o);
    }
    o
}

fn count(cases: &[&Case], g: Group) -> usize {
    cases.iter().filter(|c| c.group == g).count()
}

fn group_name(g: Group) -> &'static str {
    match g {
        Group::Dry => "无效果（dry）",
        Group::Fx => "有效果（fx：混响、合唱开着）",
    }
}
