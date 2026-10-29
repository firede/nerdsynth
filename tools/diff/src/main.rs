//! The differential test bench of NerdSynth: renders the same MIDI with the
//! same SoundFont through FluidSynth (the main reference), SpessaSynth (the
//! second one) and NerdSynth, and compares them metric by metric.
//!
//! See README.md for the commands, the cases and the metrics.

mod cases;
mod judge;
mod metrics;
mod midi;
mod render;
mod report;
mod wav;

use std::process::ExitCode;

use cases::Case;
use metrics::CaseResult;
use render::{Dirs, Synth};

const USAGE: &str = "\
usage: nerdsynth-diff <command> [options]

commands:
  list                     list the cases
  gen                      write each case's MIDI and case file to out/cases
  refs                     render the cases with FluidSynth and SpessaSynth
  calibrate                compare SpessaSynth with FluidSynth, write
                           thresholds.json and out/calibration.md
  run                      render with --synth, compare each with --ref, judge
                           by thresholds.json; exits 1 when a case fails

options:
  --filter A,B             only the cases whose id contains A or B
  --synth A,B              synths for `run` (default nerdsynth,fluidsynth):
                           fluidsynth, spessasynth, nerdsynth
  --ref NAME               the reference for `run` (default fluidsynth)
  --force                  render again even if renderings exist
  --jobs N                 parallel renders (default: the CPU count)
  --no-save                `calibrate` without writing thresholds.json
";

struct Opts {
    filter: Vec<String>,
    synths: Vec<Synth>,
    reference: Synth,
    force: bool,
    jobs: usize,
    save: bool,
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(2)
        }
    }
}

fn run(args: &[String]) -> Result<ExitCode, String> {
    let Some(cmd) = args.first() else {
        eprint!("{USAGE}");
        return Ok(ExitCode::from(2));
    };
    let mut o = Opts {
        filter: Vec::new(),
        synths: vec![Synth::Nerd, Synth::Fluid],
        reference: Synth::Fluid,
        force: false,
        jobs: std::thread::available_parallelism().map_or(4, |n| n.get()),
        save: true,
    };
    let mut it = args[1..].iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().ok_or_else(|| format!("{a} needs a value"));
        match a.as_str() {
            "--filter" => o.filter = value()?.split(',').map(String::from).collect(),
            "--synth" => o.synths = value()?.split(',').map(Synth::parse).collect::<Result<_, _>>()?,
            "--ref" => o.reference = Synth::parse(value()?)?,
            "--force" => o.force = true,
            "--jobs" => o.jobs = value()?.parse().map_err(|_| "--jobs needs a number")?,
            "--no-save" => o.save = false,
            "-h" | "--help" => {
                print!("{USAGE}");
                return Ok(ExitCode::SUCCESS);
            }
            _ => return Err(format!("unknown option {a}\n{USAGE}")),
        }
    }
    let all = cases::all();
    let cases: Vec<&Case> =
        all.iter().filter(|c| o.filter.is_empty() || o.filter.iter().any(|f| c.id.contains(f.as_str()))).collect();
    if cases.is_empty() {
        return Err("no case matches the filter".into());
    }
    let dirs = Dirs::new();
    match cmd.as_str() {
        "list" => {
            for c in &cases {
                println!("{:<40} {:<3} {}", c.id, c.group.name(), c.desc);
            }
            println!("{} cases", cases.len());
        }
        "gen" => {
            render::generate(&dirs, &cases)?;
            println!("{} cases written to {}", cases.len(), dirs.out.join("cases").display());
        }
        "refs" => {
            for s in [Synth::Fluid, Synth::Spessa] {
                let v = render::render(&dirs, s, &cases, o.force, o.jobs)?;
                println!("{v}: {} cases in {}", cases.len(), dirs.out.join("render").join(s.name()).display());
            }
        }
        "calibrate" => {
            let fv = render::render(&dirs, Synth::Fluid, &cases, o.force, o.jobs)?;
            let sv = render::render(&dirs, Synth::Spessa, &cases, o.force, o.jobs)?;
            let results = compare_all(&dirs, &cases, Synth::Fluid, Synth::Spessa, o.jobs)?;
            let t = judge::calibrate(&cases, &results, fv.clone(), sv.clone());
            let save = o.save && o.filter.is_empty();
            if save {
                t.save(&dirs.root.join("thresholds.json"))?;
            } else if o.save {
                eprintln!("thresholds.json not written: a filtered calibration covers only some cases");
            }
            let notes = std::fs::read_to_string(dirs.root.join("calibration-notes.md")).ok();
            let h = report::Header {
                title: "差分测试台校准：SpessaSynth 对 FluidSynth".into(),
                reference: fv,
                candidate: sv,
                settings: settings(),
            };
            let text = report::write(&h, &cases, &results, &t, true, notes.as_deref());
            write_out(&dirs, "calibration.md", &text)?;
            write_results(&dirs, "spessasynth-vs-fluidsynth", &results)?;
            summary(&cases, &results, &t);
            println!(
                "report: {}{}",
                dirs.out.join("calibration.md").display(),
                if save {
                    format!(", thresholds: {}", dirs.root.join("thresholds.json").display())
                } else {
                    String::new()
                }
            );
        }
        "run" => {
            let t = judge::Thresholds::load(&dirs.root.join("thresholds.json"))?;
            let mut synths = o.synths.clone();
            if !synths.contains(&o.reference) {
                synths.push(o.reference);
            }
            let mut versions = std::collections::BTreeMap::new();
            for &s in &synths {
                versions.insert(s, render::render(&dirs, s, &cases, o.force, o.jobs)?);
            }
            let mut failed = false;
            for &cand in synths.iter().filter(|&&s| s != o.reference) {
                let results = compare_all(&dirs, &cases, o.reference, cand, o.jobs)?;
                let name = format!("{}-vs-{}", cand.name(), o.reference.name());
                let h = report::Header {
                    title: format!("差分测试：{} 对 {}", cand.name(), o.reference.name()),
                    reference: versions[&o.reference].clone(),
                    candidate: versions[&cand].clone(),
                    settings: settings(),
                };
                let text = report::write(&h, &cases, &results, &t, false, None);
                write_out(&dirs, &format!("report-{name}.md"), &text)?;
                write_results(&dirs, &name, &results)?;
                println!("== {name}");
                failed |= summary(&cases, &results, &t);
                println!("report: {}", dirs.out.join(format!("report-{name}.md")).display());
            }
            return Ok(if failed { ExitCode::FAILURE } else { ExitCode::SUCCESS });
        }
        _ => return Err(format!("unknown command {cmd}\n{USAGE}")),
    }
    Ok(ExitCode::SUCCESS)
}

fn settings() -> Vec<String> {
    vec![
        format!("采样率 {} Hz，立体声 32 位浮点；事件在 FluidSynth 播放器生效的帧（64 帧一块）", render::RATE),
        "FluidSynth：-g 1 -R/-C 只在 fx 用例打开，synth.reverb.engine=fdn，四点插值（默认）".into(),
        "SpessaSynth：gain = 1/0.6（抵消它自带的 0.6），hermite 插值，effectsEnabled 只在 fx 用例打开".into(),
    ]
}

/// Compares every case's renderings, in parallel.
fn compare_all(
    dirs: &Dirs,
    cases: &[&Case],
    reference: Synth,
    cand: Synth,
    jobs: usize,
) -> Result<Vec<CaseResult>, String> {
    let slots: Vec<std::sync::Mutex<Option<Result<CaseResult, String>>>> =
        cases.iter().map(|_| std::sync::Mutex::new(None)).collect();
    let next = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|s| {
        for _ in 0..jobs.max(1) {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some(c) = cases.get(i) else { break };
                    let r = wav::read(&dirs.wav(reference, c))
                        .and_then(|a| wav::read(&dirs.wav(cand, c)).map(|b| metrics::compare(c, &a, &b)));
                    *slots[i].lock().unwrap() = Some(r);
                }
            });
        }
    });
    slots.into_iter().map(|m| m.into_inner().unwrap().unwrap()).collect()
}

fn write_out(dirs: &Dirs, name: &str, text: &str) -> Result<(), String> {
    std::fs::create_dir_all(&dirs.out).map_err(|e| e.to_string())?;
    let path = dirs.out.join(name);
    std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))
}

fn write_results(dirs: &Dirs, name: &str, results: &[CaseResult]) -> Result<(), String> {
    let path = dirs.out.join("results").join(format!("{name}.json"));
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    let text = serde_json::to_string_pretty(results).map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Prints the cases beyond the thresholds; true if there are any.
fn summary(cases: &[&Case], results: &[CaseResult], t: &judge::Thresholds) -> bool {
    let offset = judge::gain_offset(cases, results);
    println!("gain offset {offset:+.2} dB");
    let mut n = 0;
    for (c, r) in cases.iter().zip(results) {
        let f = judge::failures(c, r, offset, t);
        if f.is_empty() {
            continue;
        }
        n += 1;
        let list: Vec<String> = f
            .iter()
            .map(|f| {
                format!(
                    "{} {} ({})",
                    f.worst.metric.name(),
                    report::fmt_value(f.worst.metric, f.worst.value),
                    f.worst.segment
                )
            })
            .collect();
        println!("  {:<40} {}", c.id, list.join(", "));
    }
    println!("{n} of {} cases beyond the thresholds", cases.len());
    n > 0 || offset.abs() > t.max_gain_offset_db
}
