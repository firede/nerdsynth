//! Rendering the cases with each synthesizer, to the same format: stereo
//! 32-bit float WAV at [`RATE`], exactly as long as the case, every event at
//! the frame [`fluid_frame`] gives.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde::Serialize;

use crate::cases::{Case, Group};
use crate::midi::{fluid_frame, sorted, write_smf};
use crate::wav;

pub const RATE: u32 = 44100;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Synth {
    Fluid,
    Spessa,
    Nerd,
}

impl Synth {
    pub const ALL: [Synth; 3] = [Synth::Fluid, Synth::Spessa, Synth::Nerd];

    pub fn name(self) -> &'static str {
        match self {
            Synth::Fluid => "fluidsynth",
            Synth::Spessa => "spessasynth",
            Synth::Nerd => "nerdsynth",
        }
    }

    pub fn parse(s: &str) -> Result<Synth, String> {
        Synth::ALL.into_iter().find(|x| x.name() == s).ok_or_else(|| format!("unknown synth {s:?}"))
    }

    /// Whether it renders from scratch on every run, rather than once:
    /// NerdSynth changes with the code, the references do not.
    pub fn volatile(self) -> bool {
        self == Synth::Nerd
    }
}

/// Where the bench keeps its files.
pub struct Dirs {
    /// `tools/diff`.
    pub root: PathBuf,
    /// `tools/diff/out`, ignored by git.
    pub out: PathBuf,
}

impl Dirs {
    pub fn new() -> Dirs {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        Dirs { out: root.join("out"), root }
    }

    pub fn midi(&self, case: &Case) -> PathBuf {
        self.out.join("cases").join(format!("{}.mid", case.id))
    }

    pub fn case_json(&self, case: &Case) -> PathBuf {
        self.out.join("cases").join(format!("{}.json", case.id))
    }

    pub fn wav(&self, synth: Synth, case: &Case) -> PathBuf {
        self.out.join("render").join(synth.name()).join(format!("{}.wav", case.id))
    }

    pub fn version_file(&self, synth: Synth) -> PathBuf {
        self.out.join("render").join(synth.name()).join("VERSION")
    }
}

/// A case as the renderers take it: the events with their frames.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaseFile {
    pub id: String,
    pub sample_rate: u32,
    pub frames: u64,
    pub soundfont: PathBuf,
    pub polyphony: u32,
    pub effects: bool,
    pub events: Vec<EventFile>,
}

#[derive(Serialize)]
pub struct EventFile {
    pub frame: u64,
    pub bytes: Vec<u8>,
}

pub fn frames(case: &Case) -> u64 {
    case.dur_ms as u64 * RATE as u64 / 1000
}

pub fn case_file(case: &Case) -> Result<CaseFile, String> {
    Ok(CaseFile {
        id: case.id.clone(),
        sample_rate: RATE,
        frames: frames(case),
        soundfont: case.sf2.path()?,
        polyphony: case.polyphony,
        effects: case.group == Group::Fx,
        events: sorted(&case.events)
            .into_iter()
            .map(|e| EventFile { frame: fluid_frame(e.ms, RATE), bytes: e.msg.bytes() })
            .collect(),
    })
}

/// Writes each case's MIDI file and case file.
pub fn generate(dirs: &Dirs, cases: &[&Case]) -> Result<(), String> {
    for case in cases {
        let mid = dirs.midi(case);
        mkdirs(&mid)?;
        std::fs::write(&mid, write_smf(&case.events, case.dur_ms)).map_err(|e| format!("{}: {e}", mid.display()))?;
        let json = serde_json::to_vec_pretty(&case_file(case)?).map_err(|e| e.to_string())?;
        let path = dirs.case_json(case);
        std::fs::write(&path, json).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(())
}

fn mkdirs(file: &Path) -> Result<(), String> {
    let dir = file.parent().unwrap();
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))
}

/// Renders the cases with `synth`; cases already rendered are kept unless
/// `force` (or the synth is volatile). Returns the synth's version.
pub fn render(dirs: &Dirs, synth: Synth, cases: &[&Case], force: bool, jobs: usize) -> Result<String, String> {
    if synth == Synth::Nerd && !cfg!(feature = "nerdsynth") {
        return Err("NerdSynth rendering is off: its engine is a stub. Build with --features nerdsynth".into());
    }
    let version = version(dirs, synth)?;
    let vfile = dirs.version_file(synth);
    // Renders of another version are stale.
    let stale = std::fs::read_to_string(&vfile).map(|v| v.trim() != version).unwrap_or(true);
    let todo: Vec<&Case> =
        cases.iter().copied().filter(|c| force || stale || synth.volatile() || !dirs.wav(synth, c).is_file()).collect();
    if todo.is_empty() {
        return Ok(version);
    }
    eprintln!("rendering {} cases with {} ({version})", todo.len(), synth.name());
    generate(dirs, &todo)?;
    for c in &todo {
        mkdirs(&dirs.wav(synth, c))?;
    }
    match synth {
        Synth::Fluid => parallel(&todo, jobs, |c| fluidsynth(dirs, c))?,
        Synth::Spessa => spessasynth(dirs, &todo, jobs)?,
        Synth::Nerd => parallel(&todo, jobs, |c| nerdsynth(dirs, c))?,
    }
    // Every rendering, to the same length and format.
    parallel(&todo, jobs, |c| {
        let path = dirs.wav(synth, c);
        let mut audio = wav::read(&path)?;
        if audio.rate != RATE {
            return Err(format!("{}: rate {} instead of {RATE}", path.display(), audio.rate));
        }
        audio.fit(frames(c) as usize);
        wav::write(&path, &audio).map_err(|e| e.to_string())
    })?;
    std::fs::write(&vfile, format!("{version}\n")).map_err(|e| e.to_string())?;
    Ok(version)
}

/// The synth's version, as recorded with its renderings.
pub fn version(dirs: &Dirs, synth: Synth) -> Result<String, String> {
    match synth {
        Synth::Fluid => {
            let out = Command::new(fluidsynth_bin())
                .arg("--version")
                .output()
                .map_err(|e| format!("cannot run fluidsynth ({e}); install it: brew install fluid-synth"))?;
            let text = String::from_utf8_lossy(&out.stdout);
            let line = text.lines().find(|l| l.contains("version")).unwrap_or("unknown");
            Ok(format!("FluidSynth {}", line.rsplit(' ').next().unwrap_or("?")))
        }
        Synth::Spessa => {
            let pkg = dirs.root.join("spessa/node_modules/spessasynth_core/package.json");
            let text = std::fs::read_to_string(&pkg)
                .map_err(|_| "spessasynth_core is not installed; run: mise run diff:setup".to_string())?;
            let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
            Ok(format!("SpessaSynth (spessasynth_core) {}", v["version"].as_str().unwrap_or("?")))
        }
        Synth::Nerd => Ok(format!("NerdSynth {} (working tree)", env!("CARGO_PKG_VERSION"))),
    }
}

fn parallel<F>(cases: &[&Case], jobs: usize, f: F) -> Result<(), String>
where
    F: Fn(&Case) -> Result<(), String> + Sync,
{
    let next = AtomicUsize::new(0);
    let errors = Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for _ in 0..jobs.clamp(1, cases.len().max(1)) {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(c) = cases.get(i) else { break };
                    if let Err(e) = f(c) {
                        errors.lock().unwrap().push(format!("{}: {e}", c.id));
                    }
                }
            });
        }
    });
    let errors = errors.into_inner().unwrap();
    if errors.is_empty() { Ok(()) } else { Err(errors.join("\n")) }
}

fn fluidsynth_bin() -> String {
    std::env::var("FLUIDSYNTH").unwrap_or_else(|_| "fluidsynth".into())
}

/// The arguments FluidSynth renders a case with; the README explains each.
pub fn fluidsynth_args(case: &Case, sf2: &Path, midi: &Path, out: &Path) -> Vec<String> {
    let fx = if case.group == Group::Fx { "1" } else { "0" };
    let mut a: Vec<String> = ["-q", "-n", "-i", "-R", fx, "-C", fx, "-g", "1", "-r"].map(String::from).to_vec();
    a.push(RATE.to_string());
    for o in [
        format!("synth.polyphony={}", case.polyphony),
        // FluidSynth 2.4's reverb; 2.5 made Dattorro's the default.
        "synth.reverb.engine=fdn".into(),
        "synth.midi-bank-select=gs".into(),
        "synth.cpu-cores=1".into(),
        "synth.dynamic-sample-loading=0".into(),
        "player.reset-synth=1".into(),
        "audio.period-size=64".into(),
    ] {
        a.push("-o".into());
        a.push(o);
    }
    for s in ["-O", "float", "-T", "wav", "-E", "little", "-F"] {
        a.push(s.into());
    }
    a.push(out.display().to_string());
    a.push(sf2.display().to_string());
    a.push(midi.display().to_string());
    a
}

fn fluidsynth(dirs: &Dirs, case: &Case) -> Result<(), String> {
    let out = dirs.wav(Synth::Fluid, case);
    let args = fluidsynth_args(case, &case.sf2.path()?, &dirs.midi(case), &out);
    let r = Command::new(fluidsynth_bin()).args(&args).output().map_err(|e| e.to_string())?;
    if !r.status.success() || !out.is_file() {
        return Err(format!("fluidsynth failed: {}", String::from_utf8_lossy(&r.stderr)));
    }
    Ok(())
}

/// Runs `spessa/render.mjs` in a few processes, each loading the
/// SoundFonts it needs once.
fn spessasynth(dirs: &Dirs, cases: &[&Case], jobs: usize) -> Result<(), String> {
    #[derive(Serialize)]
    struct Job {
        case: PathBuf,
        out: PathBuf,
    }
    // Cases of one SoundFont go to the same process.
    let mut sorted: Vec<&Case> = cases.to_vec();
    sorted.sort_by_key(|c| c.sf2);
    let procs = jobs.clamp(1, 6).min(sorted.len());
    let chunk = sorted.len().div_ceil(procs);
    let script = dirs.root.join("spessa/render.mjs");
    let mut children = Vec::new();
    for (i, part) in sorted.chunks(chunk).enumerate() {
        let list: Vec<Job> =
            part.iter().map(|c| Job { case: dirs.case_json(c), out: dirs.wav(Synth::Spessa, c) }).collect();
        let path = dirs.out.join(format!("render/spessasynth/jobs-{i}.json"));
        mkdirs(&path)?;
        std::fs::write(&path, serde_json::to_vec(&list).unwrap()).map_err(|e| e.to_string())?;
        let child = Command::new("node")
            .arg(&script)
            .arg(&path)
            .spawn()
            .map_err(|e| format!("cannot run node ({e}); node 24 is needed"))?;
        children.push(child);
    }
    let mut failed = false;
    for mut c in children {
        failed |= !c.wait().map_err(|e| e.to_string())?.success();
    }
    if failed { Err("spessa/render.mjs failed".into()) } else { Ok(()) }
}

#[cfg(feature = "nerdsynth")]
fn nerdsynth(dirs: &Dirs, case: &Case) -> Result<(), String> {
    use std::sync::Arc;

    use nerdsynth::{Config, Event, SoundFont, Synth};

    let path = case.sf2.path()?;
    let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let sf = Arc::new(SoundFont::from_bytes(&bytes).map_err(|e| e.to_string())?);
    let file = case_file(case)?;
    let mut config = Config::new(RATE);
    config.max_voices = case.polyphony as usize;
    config.effects = file.effects;
    config.event_capacity = file.events.len().clamp(16, 65536);
    let mut synth = Synth::new(sf, config).map_err(|e| e.to_string())?;
    for e in &file.events {
        let event = Event::from_midi(&e.bytes).ok_or_else(|| format!("unparsed event {:02X?}", e.bytes))?;
        synth.schedule(e.frame, event).map_err(|e| e.to_string())?;
    }
    let mut audio = wav::Audio::silent(RATE, file.frames as usize);
    for (l, r) in audio.left.chunks_mut(4096).zip(audio.right.chunks_mut(4096)) {
        synth.render(l, r);
    }
    wav::write(&dirs.wav(self::Synth::Nerd, case), &audio).map_err(|e| e.to_string())
}

#[cfg(not(feature = "nerdsynth"))]
fn nerdsynth(_dirs: &Dirs, _case: &Case) -> Result<(), String> {
    Err("NerdSynth rendering is off: its engine is a stub. Build with --features nerdsynth".into())
}
