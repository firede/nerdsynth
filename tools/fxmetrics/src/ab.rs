//! A/B renders of music with each synthesizer, for listening: piano with
//! the reverb, strings with the chorus, drums with the reverb, on
//! GeneralUser GS (found as the difference harness finds it). Each pair is
//! level-matched: NerdSynth's render is scaled to FluidSynth's RMS, so that
//! what differs is the effect's character, not its loudness.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use nerdsynth::testkit::smf::{SmfBuilder, Track};
use nerdsynth::{Config, Event, SoundFont, Synth};

use crate::render::{fluidsynth_args, fluidsynth_bin};
use crate::wav::{self, Audio};

const RATE: u32 = 44100;
const SF2: &str = "GeneralUser-GS-2.0.3.sf2";

/// A piece: MIDI messages at milliseconds.
struct Piece {
    name: &'static str,
    dur_ms: u32,
    events: Vec<(u32, Vec<u8>)>,
}

impl Piece {
    fn new(name: &'static str, dur_ms: u32, setup: &[[u8; 3]]) -> Piece {
        // A program change has one data byte.
        let len = |m: &[u8; 3]| if m[0] & 0xF0 == 0xC0 { 2 } else { 3 };
        Piece { name, dur_ms, events: setup.iter().map(|m| (0, m[..len(m)].to_vec())).collect() }
    }

    fn note(&mut self, channel: u8, key: u8, velocity: u8, on_ms: u32, len_ms: u32) {
        self.events.push((on_ms, vec![0x90 | channel, key, velocity]));
        self.events.push((on_ms + len_ms, vec![0x80 | channel, key, 64]));
    }

    fn sorted(&self) -> Vec<(u32, Vec<u8>)> {
        let mut e = self.events.clone();
        e.sort_by_key(|e| e.0);
        e
    }

    /// A MIDI file: a tick a millisecond.
    fn midi(&self) -> Vec<u8> {
        let mut t = Track::new().tempo(0, 1_000_000);
        let mut last = 0;
        for (ms, bytes) in self.sorted() {
            t = t.channel(u64::from(ms - last), bytes[0], &bytes[1..]);
            last = ms;
        }
        t = t.end(u64::from(self.dur_ms - last));
        SmfBuilder::new().division(1000).track(t).build()
    }
}

fn pieces() -> Vec<Piece> {
    // Piano: an arpeggio, a held chord, staccato chords with rests between.
    let mut piano = Piece::new("piano-reverb", 10000, &[[0xC0, 0, 0], [0xB0, 91, 100], [0xB0, 93, 0]]);
    for (i, k) in [60, 64, 67, 72, 76, 72, 67, 64].into_iter().enumerate() {
        piano.note(0, k, 90, 200 + i as u32 * 180, 160);
    }
    for k in [48, 55, 64, 67, 72] {
        piano.note(0, k, 85, 1800, 1200);
    }
    for (at, chord) in [(4000, [53, 60, 65, 69]), (5200, [55, 62, 67, 71]), (6400, [48, 60, 64, 72])] {
        for k in chord {
            piano.note(0, k, 100, at, 120);
        }
    }
    piano.note(0, 84, 70, 7600, 150);
    // Strings: sustained chords.
    let mut strings = Piece::new("strings-chorus", 10000, &[[0xC0, 48, 0], [0xB0, 91, 0], [0xB0, 93, 127]]);
    for (i, chord) in [[48, 55, 60, 64], [45, 52, 57, 64], [41, 53, 57, 65], [43, 50, 55, 67]].into_iter().enumerate() {
        for k in chord {
            strings.note(0, k, 90, 200 + i as u32 * 2000, 1900);
        }
    }
    // Drums: two bars at 120 bpm, then the tail.
    let mut drums = Piece::new("drums-reverb", 6500, &[[0xB9, 91, 100], [0xB9, 93, 0]]);
    drums.note(9, 49, 100, 200, 100);
    for eighth in 0..16u32 {
        let at = 200 + eighth * 250;
        drums.note(9, 42, if eighth % 2 == 0 { 90 } else { 60 }, at, 100);
        match eighth % 4 {
            0 => drums.note(9, 36, 110, at, 100),
            2 => drums.note(9, 38, 105, at, 100),
            _ => {}
        }
    }
    drums.note(9, 38, 120, 4200, 100);
    drums.note(9, 49, 110, 4200, 100);
    vec![piano, strings, drums]
}

/// GeneralUser GS, where the difference harness looks for it.
fn soundfont() -> Result<PathBuf, String> {
    let mut dirs = Vec::new();
    if let Some(d) = std::env::var_os("NERDSYNTH_SOUNDFONTS") {
        dirs.push(PathBuf::from(d));
    }
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        dirs.push(home.join(".nerdsynth/soundfonts"));
        dirs.push(home.join(".nerdice/soundfonts"));
    }
    dirs.iter()
        .map(|d| d.join(SF2))
        .find(|p| p.is_file())
        .ok_or_else(|| format!("{SF2} not found in {dirs:?}; see tools/diff/README.md"))
}

fn frames(piece: &Piece) -> usize {
    (u64::from(piece.dur_ms) * u64::from(RATE) / 1000) as usize
}

fn fluidsynth(dir: &Path, sf2: &Path, piece: &Piece) -> Result<Audio, String> {
    let mid = dir.join(format!("{}.mid", piece.name));
    std::fs::write(&mid, piece.midi()).map_err(|e| e.to_string())?;
    let out = dir.join(format!("{}-raw.wav", piece.name));
    let r = Command::new(fluidsynth_bin())
        .args(fluidsynth_args(RATE, true, sf2, &mid, &out))
        .output()
        .map_err(|e| format!("cannot run fluidsynth: {e}"))?;
    if !r.status.success() {
        return Err(format!("fluidsynth failed: {}", String::from_utf8_lossy(&r.stderr)));
    }
    let mut a = wav::read(&out)?;
    let _ = std::fs::remove_file(&out);
    let _ = std::fs::remove_file(&mid);
    a.fit(frames(piece));
    Ok(a)
}

fn nerdsynth(sf: &Arc<SoundFont>, piece: &Piece) -> Result<Audio, String> {
    let mut config = Config::new(RATE);
    config.event_capacity = 4096;
    let mut synth = Synth::new(sf.clone(), config).map_err(|e| e.to_string())?;
    for (ms, bytes) in piece.sorted() {
        let event = Event::from_midi(&bytes).ok_or("unparsed event")?;
        synth.schedule(u64::from(ms) * u64::from(RATE) / 1000, event).map_err(|e| e.to_string())?;
    }
    let mut a = Audio::silent(RATE, frames(piece));
    for (l, r) in a.left.chunks_mut(4096).zip(a.right.chunks_mut(4096)) {
        synth.render(l, r);
    }
    Ok(a)
}

fn rms(a: &Audio) -> f64 {
    let e: f64 = a.left.iter().chain(&a.right).map(|&x| f64::from(x) * f64::from(x)).sum();
    (e / (2 * a.left.len()).max(1) as f64).sqrt()
}

pub fn render_all(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let sf2 = soundfont()?;
    let bytes = std::fs::read(&sf2).map_err(|e| format!("{}: {e}", sf2.display()))?;
    let sf = Arc::new(SoundFont::from_bytes(&bytes).map_err(|e| e.to_string())?);
    for piece in pieces() {
        let fs = fluidsynth(dir, &sf2, &piece)?;
        let mut ns = nerdsynth(&sf, &piece)?;
        let gain = rms(&fs) / rms(&ns).max(1e-12);
        for x in ns.left.iter_mut().chain(ns.right.iter_mut()) {
            *x = (f64::from(*x) * gain) as f32;
        }
        for (synth, a) in [("fluidsynth", &fs), ("nerdsynth", &ns)] {
            let path = dir.join(format!("{}-{synth}.wav", piece.name));
            wav::write(&path, a).map_err(|e| e.to_string())?;
            eprintln!("{}", path.display());
        }
        eprintln!("  {}: NerdSynth scaled by {:+.2} dB", piece.name, 20.0 * gain.log10());
    }
    Ok(())
}
