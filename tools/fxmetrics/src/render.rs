//! The test signals, and rendering them with FluidSynth and NerdSynth.
//!
//! A tiny SoundFont is built per sample rate, its samples at that rate and
//! played at their own pitch, so that no interpolation touches them: a
//! click, a click train, looped noise, and looped sines of whole hertz.
//! Preset 0 sends only to the reverb, preset 1 only to the chorus, both at
//! 100 %. Each test is rendered with the effects on and off; the wet signal
//! is the difference (the dry path is the same in both renders).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use nerdsynth::soundfont::GeneratorType as G;
use nerdsynth::testkit::sf2::{SampleSpec, Sf2, Zone};
use nerdsynth::testkit::smf::{SmfBuilder, Track};
use nerdsynth::{Config, Event, SoundFont, Synth as Nerd};

use crate::wav::{self, Audio};

/// The keys of the samples.
pub const KEY_CLICK: u8 = 0;
pub const KEY_TRAIN: u8 = 1;
pub const KEY_NOISE: u8 = 2;
/// Chorus sines, at [`CHORUS_SINES`].
pub const KEY_CHORUS_SINE: u8 = 3;
/// Reverb tones, at [`tone_hz`].
pub const KEY_TONE: u8 = 10;

pub const CHORUS_SINES: [u32; 3] = [440, 2000, 8000];
pub const TONES: usize = 96;

/// The click train's rate: clicks per second (its period, rounded to whole
/// frames, is in [`train_period`]).
pub const TRAIN_PER_SECOND: usize = 40;

/// The `i`th reverb tone, in whole hertz: quarter tones up from 110 Hz.
pub fn tone_hz(i: usize) -> u32 {
    (110.0 * 2f64.powf(i as f64 / 24.0)).round() as u32
}

pub fn train_period(rate: u32) -> usize {
    (rate as usize).div_ceil(TRAIN_PER_SECOND)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Synth {
    Fluid,
    Nerd,
}

impl Synth {
    pub fn name(self) -> &'static str {
        match self {
            Synth::Fluid => "fluidsynth",
            Synth::Nerd => "nerdsynth",
        }
    }
}

/// A note, in milliseconds.
#[derive(Clone, Copy, Debug)]
pub struct Note {
    pub key: u8,
    pub on_ms: u32,
    pub off_ms: u32,
}

/// A test: a program, its notes, and its length.
pub struct Test {
    pub name: &'static str,
    pub program: u8,
    pub notes: Vec<Note>,
    pub dur_ms: u32,
}

/// The reverb test: a click, then the tones one after another.
pub const TONE_START_MS: u32 = 6000;
pub const TONE_ON_MS: u32 = 1000;
pub const TONE_STEP_MS: u32 = 2500;
pub const CLICK_MS: u32 = 100;

pub fn reverb_test() -> Test {
    let mut notes = vec![Note { key: KEY_CLICK, on_ms: CLICK_MS, off_ms: CLICK_MS + 50 }];
    for i in 0..TONES {
        let on = TONE_START_MS + i as u32 * TONE_STEP_MS;
        notes.push(Note { key: KEY_TONE + i as u8, on_ms: on, off_ms: on + TONE_ON_MS });
    }
    Test { name: "reverb", program: 0, notes, dur_ms: TONE_START_MS + TONES as u32 * TONE_STEP_MS }
}

/// The chorus test: the click train, each sine, then noise.
pub const TRAIN_MS: (u32, u32) = (100, 22100);
pub const SINE_MS: u32 = 8000;
pub const SINE_START_MS: u32 = 22600;
pub const SINE_STEP_MS: u32 = 8500;
pub const NOISE_MS: (u32, u32) = (48500, 56500);

pub fn chorus_test() -> Test {
    let mut notes = vec![Note { key: KEY_TRAIN, on_ms: TRAIN_MS.0, off_ms: TRAIN_MS.1 }];
    for i in 0..CHORUS_SINES.len() {
        let on = SINE_START_MS + i as u32 * SINE_STEP_MS;
        notes.push(Note { key: KEY_CHORUS_SINE + i as u8, on_ms: on, off_ms: on + SINE_MS });
    }
    notes.push(Note { key: KEY_NOISE, on_ms: NOISE_MS.0, off_ms: NOISE_MS.1 });
    Test { name: "chorus", program: 1, notes, dur_ms: NOISE_MS.1 + 1000 }
}

/// 24-bit points of a signal in -1..1, as SF2 16-bit points and low bytes.
fn pcm24(x: &[f64]) -> (Vec<i16>, Vec<u8>) {
    let v: Vec<i32> = x.iter().map(|s| (s.clamp(-1.0, 1.0) * 8_388_607.0).round() as i32).collect();
    (v.iter().map(|v| (v >> 8) as i16).collect(), v.iter().map(|v| (v & 0xFF) as u8).collect())
}

fn sample(name: &str, x: &[f64], rate: u32, key: u8, looped: Option<(u32, u32)>) -> SampleSpec {
    let (hi, lo) = pcm24(x);
    let mut s = SampleSpec::new(name, hi).low_bytes(lo).rate(rate).key(key);
    if let Some((a, b)) = looped {
        s = s.looped(a, b);
    }
    s
}

/// Deterministic Gaussian-ish noise: the sum of four uniforms.
pub fn noise(n: usize, seed: u64) -> Vec<f64> {
    let mut s = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut uniform = move || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        (s >> 11) as f64 / (1u64 << 53) as f64 - 0.5
    };
    (0..n).map(|_| (uniform() + uniform() + uniform() + uniform()) * 0.866).collect()
}

/// A second of a sine of `hz` whole hertz, looped whole, and a few points
/// more to close the loop.
fn sine_loop(rate: u32, hz: u32, amplitude: f64) -> (Vec<f64>, (u32, u32)) {
    let n = rate as usize;
    let x = (0..n + 8)
        .map(|i| amplitude * (std::f64::consts::TAU * f64::from(hz) * (i % n) as f64 / f64::from(rate)).sin())
        .collect();
    (x, (0, n as u32))
}

/// The SoundFont of the tests at `rate`.
pub fn soundfont(rate: u32) -> Vec<u8> {
    let mut b = Sf2::new();
    let mut samples: Vec<(u8, u16, bool)> = Vec::new();
    // A click, after the attack has opened.
    let mut click = vec![0.0; 512];
    click[256] = 0.9;
    samples.push((KEY_CLICK, b.sample(sample("click", &click, rate, KEY_CLICK, None)), false));
    let period = train_period(rate);
    let n = period * TRAIN_PER_SECOND;
    let mut train = vec![0.0; n + 8];
    for i in (0..n + 8).step_by(period) {
        train[i] = 0.9;
    }
    samples.push((KEY_TRAIN, b.sample(sample("train", &train, rate, KEY_TRAIN, Some((0, n as u32)))), true));
    let n = rate as usize;
    let mut x = noise(n, 11);
    x.iter_mut().for_each(|v| *v *= 0.25);
    let head: Vec<f64> = x[..8].to_vec();
    x.extend(head);
    samples.push((KEY_NOISE, b.sample(sample("noise", &x, rate, KEY_NOISE, Some((0, n as u32)))), true));
    for (i, &hz) in CHORUS_SINES.iter().enumerate() {
        let key = KEY_CHORUS_SINE + i as u8;
        let (x, l) = sine_loop(rate, hz, 0.5);
        samples.push((key, b.sample(sample("sine", &x, rate, key, Some(l))), true));
    }
    for i in 0..TONES {
        let key = KEY_TONE + i as u8;
        let (x, l) = sine_loop(rate, tone_hz(i), 0.5);
        samples.push((key, b.sample(sample("tone", &x, rate, key, Some(l))), true));
    }
    let instrument = |b: &mut Sf2, name: &str, reverb: i16, chorus: i16| {
        let zones: Vec<Zone> = samples
            .iter()
            .map(|&(key, id, looped)| {
                let mut z = Zone::new()
                    .keys(key, key)
                    .set(G::ReverbEffectsSend, reverb)
                    .set(G::ChorusEffectsSend, chorus)
                    .set(G::SampleModes, i16::from(looped));
                if looped {
                    // Soft edges: no click to excite the effects.
                    z = z.set(G::AttackVolEnv, -6000).set(G::ReleaseVolEnv, -4000);
                }
                z.sample(id)
            })
            .collect();
        b.instrument(name, zones)
    };
    let r = instrument(&mut b, "reverb", 1000, 0);
    let c = instrument(&mut b, "chorus", 0, 1000);
    b.preset("reverb", 0, 0, [Zone::new().instrument(r)]);
    b.preset("chorus", 0, 1, [Zone::new().instrument(c)]);
    b.build()
}

/// The events of a test, in milliseconds: the program and zero sends
/// (so that only the generators send), then the notes.
fn events(test: &Test) -> Vec<(u32, [u8; 3], usize)> {
    let mut ev: Vec<(u32, [u8; 3], usize)> =
        vec![(0, [0xC0, test.program, 0], 2), (0, [0xB0, 91, 0], 3), (0, [0xB0, 93, 0], 3)];
    for n in &test.notes {
        ev.push((n.on_ms, [0x90, n.key, 127], 3));
        ev.push((n.off_ms, [0x80, n.key, 64], 3));
    }
    ev.sort_by_key(|e| e.0);
    ev
}

/// A MIDI file of a test: 1000 ticks a quarter at 60 bpm, a tick a
/// millisecond.
fn midi(test: &Test) -> Vec<u8> {
    let mut t = Track::new().tempo(0, 1_000_000);
    let mut last = 0;
    for (ms, bytes, len) in events(test) {
        t = t.channel(u64::from(ms - last), bytes[0], &bytes[1..len]);
        last = ms;
    }
    t = t.end(u64::from(test.dur_ms - last));
    SmfBuilder::new().division(1000).track(t).build()
}

/// The stereo renders of a test with the effects on and off.
pub struct Pair {
    pub on: Audio,
    pub off: Audio,
}

impl Pair {
    /// The wet signal: on less off.
    pub fn wet(&self) -> Audio {
        let sub = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| x - y).collect();
        Audio {
            rate: self.on.rate,
            left: sub(&self.on.left, &self.off.left),
            right: sub(&self.on.right, &self.off.right),
        }
    }
}

pub fn frames(test: &Test, rate: u32) -> usize {
    (u64::from(test.dur_ms) * u64::from(rate) / 1000) as usize
}

/// Renders `test` with FluidSynth at `rate`, with the effects on and off.
pub fn fluidsynth(dir: &Path, test: &Test, rate: u32) -> Result<Pair, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let sf2 = dir.join(format!("tests-{rate}.sf2"));
    std::fs::write(&sf2, soundfont(rate)).map_err(|e| e.to_string())?;
    let mid = dir.join(format!("{}.mid", test.name));
    std::fs::write(&mid, midi(test)).map_err(|e| e.to_string())?;
    let render = |fx: bool| -> Result<Audio, String> {
        let out = dir.join(format!("fs-{}-{rate}-{}.wav", test.name, if fx { "on" } else { "off" }));
        let args = fluidsynth_args(rate, fx, &sf2, &mid, &out);
        let r =
            Command::new(fluidsynth_bin()).args(&args).output().map_err(|e| format!("cannot run fluidsynth: {e}"))?;
        if !r.status.success() {
            return Err(format!("fluidsynth failed: {}", String::from_utf8_lossy(&r.stderr)));
        }
        let mut a = wav::read(&out)?;
        if a.rate != rate {
            return Err(format!("{}: rate {}", out.display(), a.rate));
        }
        a.fit(frames(test, rate));
        Ok(a)
    };
    Ok(Pair { on: render(true)?, off: render(false)? })
}

pub fn fluidsynth_bin() -> String {
    std::env::var("FLUIDSYNTH").unwrap_or_else(|_| "fluidsynth".into())
}

/// FluidSynth's settings: those of the difference harness (`tools/diff`):
/// gain 1, the 2.4 (`fdn`) reverb, default reverb and chorus parameters.
pub fn fluidsynth_args(rate: u32, fx: bool, sf2: &Path, midi: &Path, out: &Path) -> Vec<String> {
    let fx = if fx { "1" } else { "0" };
    let mut a: Vec<String> = ["-q", "-n", "-i", "-R", fx, "-C", fx, "-g", "1", "-r"].map(String::from).to_vec();
    a.push(rate.to_string());
    for o in [
        "synth.polyphony=256",
        "synth.reverb.engine=fdn",
        "synth.midi-bank-select=gs",
        "synth.cpu-cores=1",
        "synth.dynamic-sample-loading=0",
        "player.reset-synth=1",
        "audio.period-size=64",
    ] {
        a.push("-o".into());
        a.push(o.into());
    }
    for s in ["-O", "float", "-T", "wav", "-E", "little", "-F"] {
        a.push(s.into());
    }
    a.push(out.display().to_string());
    a.push(sf2.display().to_string());
    a.push(midi.display().to_string());
    a
}

/// Renders `test` with NerdSynth at `rate`, with the effects on and off.
pub fn nerdsynth(test: &Test, rate: u32) -> Result<Pair, String> {
    let sf = Arc::new(SoundFont::from_bytes(&soundfont(rate)).map_err(|e| e.to_string())?);
    let render = |fx: bool| -> Result<Audio, String> {
        let mut config = Config::new(rate);
        config.effects = fx;
        config.event_capacity = 4096;
        let mut synth = Nerd::new(sf.clone(), config).map_err(|e| e.to_string())?;
        for (ms, bytes, len) in events(test) {
            let frame = u64::from(ms) * u64::from(rate) / 1000;
            let event = Event::from_midi(&bytes[..len]).ok_or("unparsed event")?;
            synth.schedule(frame, event).map_err(|e| e.to_string())?;
        }
        let mut a = Audio::silent(rate, frames(test, rate));
        for (l, r) in a.left.chunks_mut(4096).zip(a.right.chunks_mut(4096)) {
            synth.render(l, r);
        }
        Ok(a)
    };
    Ok(Pair { on: render(true)?, off: render(false)? })
}

pub fn out_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("out")
}
