//! `nerdsynth-fxmetrics`: objective quality metrics of NerdSynth's reverb
//! and chorus, against FluidSynth's (black box). See the README.

mod ab;
mod dsp;
mod metrics;
mod render;
mod spec;
mod wav;

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;
use std::process::Command;

use render::Synth;
use spec::{Kind, spec};

/// Rates judged in full, and rates spot-checked.
const RATES: [u32; 2] = [44100, 48000];
const SPOT: [u32; 2] = [22050, 96000];

struct Args {
    rates: Vec<u32>,
    save: Option<String>,
    compare: Option<String>,
    cost: bool,
    wav: bool,
}

fn usage() -> ! {
    eprintln!(
        "usage: nerdsynth-fxmetrics [run] [--rates 44100,48000] [--save NAME] [--compare NAME] [--no-cost] [--wav]\n       nerdsynth-fxmetrics ab"
    );
    std::process::exit(2)
}

fn parse() -> (String, Args) {
    let mut cmd = "run".to_string();
    let mut a =
        Args { rates: RATES.iter().chain(&SPOT).copied().collect(), save: None, compare: None, cost: true, wav: false };
    let mut it = std::env::args().skip(1);
    while let Some(s) = it.next() {
        match s.as_str() {
            "run" | "ab" => cmd = s,
            "--rates" => {
                a.rates = it
                    .next()
                    .unwrap_or_else(|| usage())
                    .split(',')
                    .map(|r| r.parse().unwrap_or_else(|_| usage()))
                    .collect()
            }
            "--save" => a.save = Some(it.next().unwrap_or_else(|| usage())),
            "--compare" => a.compare = Some(it.next().unwrap_or_else(|| usage())),
            "--no-cost" => a.cost = false,
            "--wav" => a.wav = true,
            _ => usage(),
        }
    }
    (cmd, a)
}

/// Measured values: (rate, key) → (FS, ours).
type Results = BTreeMap<(u32, String), (f64, f64)>;

fn main() {
    let (cmd, args) = parse();
    let out = render::out_dir();
    if cmd == "ab" {
        if let Err(e) = ab::render_all(&out.join("ab")) {
            eprintln!("{e}");
            std::process::exit(1);
        }
        return;
    }
    let results = match measure(&args, &out) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    let cost = if args.cost { cost() } else { Vec::new() };
    if let Some(name) = &args.save {
        let mut t = String::new();
        for ((rate, key), (fs, ours)) in &results {
            let _ = writeln!(t, "{rate}\t{key}\t{fs}\t{ours}");
        }
        for (k, v) in &cost {
            let _ = writeln!(t, "0\t{k}\tNaN\t{v}");
        }
        std::fs::write(out.join(format!("results-{name}.tsv")), t).expect("write results");
    }
    let before = args.compare.as_ref().map(|n| load(&out.join(format!("results-{n}.tsv"))));
    let (text, failures) = report(&results, &cost, before.as_ref(), args.compare.as_deref());
    let path = out.join("report.md");
    std::fs::write(&path, text).expect("write report");
    for f in &failures {
        eprintln!("  {f}");
    }
    eprintln!("{} failing; report: {}", failures.len(), path.display());
    if !failures.is_empty() {
        std::process::exit(1);
    }
}

fn measure(args: &Args, out: &Path) -> Result<Results, String> {
    let results = std::sync::Mutex::new(Results::new());
    let errors = std::sync::Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for &rate in &args.rates {
            let (results, errors) = (&results, &errors);
            s.spawn(move || {
                let run = || -> Result<(), String> {
                    let dir = out.join("render").join(rate.to_string());
                    for (test, measure) in [
                        (render::reverb_test(), metrics::reverb as fn(&render::Pair) -> metrics::Values),
                        (render::chorus_test(), metrics::chorus),
                    ] {
                        let fs = render::fluidsynth(&dir, &test, rate)?;
                        let ns = render::nerdsynth(&test, rate)?;
                        if args.wav {
                            for (synth, p) in [(Synth::Fluid, &fs), (Synth::Nerd, &ns)] {
                                let path = dir.join(format!("wet-{}-{}.wav", synth.name(), test.name));
                                wav::write(&path, &p.wet()).map_err(|e| e.to_string())?;
                            }
                        }
                        let a = measure(&fs);
                        let b: BTreeMap<String, f64> = measure(&ns).into_iter().collect();
                        let mut r = results.lock().unwrap();
                        for (k, v) in a {
                            let ours = b.get(&k).copied().unwrap_or(f64::NAN);
                            r.insert((rate, k), (v, ours));
                        }
                    }
                    Ok(())
                };
                if let Err(e) = run() {
                    errors.lock().unwrap().push(format!("{rate} Hz: {e}"));
                }
            });
        }
    });
    let errors = errors.into_inner().unwrap();
    if errors.is_empty() { Ok(results.into_inner().unwrap()) } else { Err(errors.join("\n")) }
}

fn load(path: &Path) -> Results {
    let mut r = Results::new();
    if let Ok(t) = std::fs::read_to_string(path) {
        for line in t.lines() {
            let f: Vec<&str> = line.split('\t').collect();
            if f.len() == 4 {
                let n = |s: &str| s.parse::<f64>().unwrap_or(f64::NAN);
                r.insert((f[0].parse().unwrap_or(0), f[1].to_string()), (n(f[2]), n(f[3])));
            }
        }
    }
    r
}

/// Runs the effects' cost probe (an ignored test in the library, which
/// alone can reach the effects) and reads its `COST key value` lines.
fn cost() -> Vec<(String, f64)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let out = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .current_dir(&root)
        .args([
            "test",
            "--release",
            "-q",
            "-p",
            "nerdsynth",
            "--lib",
            "synth::effects::cost",
            "--",
            "--ignored",
            "--nocapture",
        ])
        .output();
    let Ok(out) = out else { return Vec::new() };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let mut f = l.strip_prefix("COST ")?.split_whitespace();
            Some((f.next()?.to_string(), f.next()?.parse().ok()?))
        })
        .collect()
}

fn fmt(v: f64) -> String {
    if !v.is_finite() {
        return "–".into();
    }
    let a = v.abs();
    if a >= 1000.0 {
        format!("{v:.0}")
    } else if a >= 10.0 {
        format!("{v:.1}")
    } else if a >= 1.0 {
        format!("{v:.2}")
    } else {
        format!("{v:.3}")
    }
}

fn label(key: &str) -> String {
    let s = spec(key);
    match key.rsplit('.').next() {
        Some(b) if b.parse::<f64>().is_ok() => format!("{} @ {b} Hz", s.label),
        _ => s.label.to_string(),
    }
}

fn report(
    results: &Results,
    cost: &[(String, f64)],
    before: Option<&Results>,
    before_name: Option<&str>,
) -> (String, Vec<String>) {
    let mut t = String::new();
    let mut failures = Vec::new();
    let version = Command::new(render::fluidsynth_bin())
        .arg("--version")
        .output()
        .ok()
        .and_then(|o| String::from_utf8_lossy(&o.stdout).lines().find(|l| l.contains("version")).map(String::from))
        .unwrap_or_default();
    let _ = writeln!(t, "# Reverb and chorus metrics: NerdSynth vs FluidSynth\n");
    let _ = writeln!(t, "- FluidSynth: {} (`synth.reverb.engine=fdn`, default parameters, gain 1)", version.trim());
    let _ = writeln!(t, "- NerdSynth: working tree");
    let _ = writeln!(t, "- Rates judged in full: {RATES:?}; spot checks: {SPOT:?}");
    let _ = writeln!(t, "- Definitions and tolerances: see the README next to this tool.\n");
    let rates: Vec<u32> = {
        let mut r: Vec<u32> = results.keys().map(|k| k.0).collect();
        r.dedup();
        r
    };
    let order = |k: &str| {
        let s = spec(k);
        let kind = match s.kind {
            Kind::Must => 0,
            Kind::Quality => 1,
            Kind::Character => 2,
            Kind::Info => 3,
        };
        (k.starts_with("cho") as u8, kind)
    };
    for &rate in &rates {
        let _ = writeln!(t, "## {rate} Hz\n");
        let mut keys: Vec<&String> = results.keys().filter(|k| k.0 == rate).map(|k| &k.1).collect();
        keys.sort_by_key(|k| order(k));
        let head = match before_name {
            Some(n) => format!(
                "| metric | unit | FS | ours ({n}) | ours | rule | |\n| --- | --- | ---: | ---: | ---: | --- | --- |"
            ),
            None => "| metric | unit | FS | ours | rule | |\n| --- | --- | ---: | ---: | --- | --- |".into(),
        };
        let mut section = None;
        for key in keys {
            let s = spec(key);
            let group = (key.starts_with("cho"), s.kind);
            if section != Some(group) {
                section = Some(group);
                let name = match s.kind {
                    Kind::Must => "must hold",
                    Kind::Quality => "quality (no worse than FS)",
                    Kind::Character if SPOT.contains(&rate) => "character (near FS; not judged at this rate)",
                    Kind::Character => "character (near FS)",
                    Kind::Info => "reported",
                };
                let _ = writeln!(t, "\n### {} — {name}\n\n{head}", if group.0 { "Chorus" } else { "Reverb" });
            }
            let (fs, ours) = results[&(rate, key.clone())];
            // FluidSynth's reverb keeps its delays in frames, so its character
            // moves with the rate: only quality is judged at the spot rates.
            let judged = !(SPOT.contains(&rate) && s.kind == Kind::Character);
            let verdict = match s.judge.pass(fs, ours).filter(|_| judged) {
                Some(true) => "pass",
                Some(false) => {
                    if s.kind != Kind::Info {
                        failures.push(format!(
                            "{rate} Hz {key}: FS {} ours {} ({})",
                            fmt(fs),
                            fmt(ours),
                            s.judge.rule()
                        ));
                    }
                    "**FAIL**"
                }
                None => "",
            };
            let old = before
                .map(|b| format!("{} | ", b.get(&(rate, key.clone())).map_or("".into(), |v| fmt(v.1))))
                .unwrap_or_default();
            let _ = writeln!(
                t,
                "| {} | {} | {} | {old}{} | {} | {verdict} |",
                label(key),
                s.unit,
                fmt(fs),
                fmt(ours),
                s.judge.rule()
            );
        }
        let _ = writeln!(t);
    }
    if !cost.is_empty() {
        let _ = writeln!(t, "## Cost (NerdSynth, release build)\n");
        let _ = writeln!(
            t,
            "| measure | {}now |\n| --- | {}---: |",
            before_name.map_or("".into(), |n| format!("{n} | ")),
            if before.is_some() { "---: | " } else { "" }
        );
        for (k, v) in cost {
            let old = before
                .map(|b| format!("{} | ", b.get(&(0, k.clone())).map_or("".into(), |x| fmt(x.1))))
                .unwrap_or_default();
            let _ = writeln!(t, "| {k} | {old}{} |", fmt(*v));
        }
    }
    (t, failures)
}
