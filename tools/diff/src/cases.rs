//! The test cases: each one a short MIDI score on one SoundFont, the
//! segments of time its metrics are measured in, and the metrics that
//! judge it.
//!
//! Every case tests one behavior. To add one, add a builder call to
//! [`all`]; ids must be unique and stable, since results and thresholds
//! refer to them.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::metrics::{Balance, Bands, Centroid, EnvCorr, EnvDev, F0, Loudness, Metric, Onset, Tail, Vibrato};
use crate::midi::{Msg, Timed};

/// The SoundFonts the cases use. They are not in the repository; see the
/// README for where they come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Sf2 {
    GeneralUser,
    FluidR3,
    TimGm6mb,
}

impl Sf2 {
    pub fn file_name(self) -> &'static str {
        match self {
            Sf2::GeneralUser => "GeneralUser-GS-2.0.3.sf2",
            Sf2::FluidR3 => "FluidR3_GM.sf2",
            Sf2::TimGm6mb => "TimGM6mb.sf2",
        }
    }

    /// A short tag for case ids.
    pub fn tag(self) -> &'static str {
        match self {
            Sf2::GeneralUser => "gu",
            Sf2::FluidR3 => "fluid",
            Sf2::TimGm6mb => "tim",
        }
    }

    /// Where the file is: `$NERDSYNTH_SOUNDFONTS`, then
    /// `~/.nerdsynth/soundfonts`, then `~/.nerdice/soundfonts`.
    pub fn path(self) -> Result<PathBuf, String> {
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
            .map(|d| d.join(self.file_name()))
            .find(|p| p.is_file())
            .ok_or_else(|| format!("{} not found in {:?}; see tools/diff/README.md", self.file_name(), dirs))
    }
}

/// Whether the reverb and the chorus run. Cases with effects are judged
/// with their own thresholds, since the effects differ the most between
/// implementations.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Group {
    Dry,
    Fx,
}

impl Group {
    pub fn name(self) -> &'static str {
        match self {
            Group::Dry => "dry",
            Group::Fx => "fx",
        }
    }
}

/// An expected pitch, measured from `from_ms` to `to_ms`.
#[derive(Clone, Debug)]
pub struct Pitch {
    /// In MIDI keys, fractional; it only centers the search, within 15
    /// semitones either way.
    pub key: f64,
    pub from_ms: u32,
    pub to_ms: u32,
}

/// A span of time measured on its own.
#[derive(Clone, Debug)]
pub struct Segment {
    pub label: String,
    pub start_ms: u32,
    pub end_ms: u32,
    pub pitch: Option<Pitch>,
}

#[derive(Clone, Debug)]
pub struct Case {
    pub id: String,
    pub category: &'static str,
    pub desc: String,
    pub sf2: Sf2,
    pub group: Group,
    /// The voices that can sound at a time.
    pub polyphony: u32,
    pub dur_ms: u32,
    pub events: Vec<Timed>,
    pub segments: Vec<Segment>,
    /// The metrics that judge the case; the others are reported only.
    pub focus: Vec<Metric>,
}

/// The default polyphony, high enough that no case but the polyphony
/// ones steals voices.
pub const POLYPHONY: u32 = 256;

// Metric sets.
const ENVELOPE: &[Metric] = &[EnvCorr, EnvDev, Loudness, F0, Centroid, Bands, Onset, Tail];
const LEVEL: &[Metric] = &[Loudness, EnvDev, Centroid, Balance];

/// A score being written on one channel.
struct Score {
    ev: Vec<Timed>,
    ch: u8,
}

impl Score {
    fn new(ch: u8) -> Score {
        Score { ev: Vec::new(), ch }
    }
    fn at(&mut self, ms: u32, msg: Msg) -> &mut Self {
        self.ev.push(Timed { ms, msg });
        self
    }
    fn on(&mut self, ms: u32, key: u8, vel: u8) -> &mut Self {
        let ch = self.ch;
        self.at(ms, Msg::NoteOn { ch, key, vel })
    }
    fn off(&mut self, ms: u32, key: u8) -> &mut Self {
        let ch = self.ch;
        self.at(ms, Msg::NoteOff { ch, key })
    }
    fn note(&mut self, ms: u32, len: u32, key: u8, vel: u8) -> &mut Self {
        self.on(ms, key, vel).off(ms + len, key)
    }
    fn cc(&mut self, ms: u32, cc: u8, val: u8) -> &mut Self {
        let ch = self.ch;
        self.at(ms, Msg::Cc { ch, cc, val })
    }
    fn program(&mut self, ms: u32, program: u8) -> &mut Self {
        let ch = self.ch;
        self.at(ms, Msg::Program { ch, program })
    }
    fn bend(&mut self, ms: u32, value: u16) -> &mut Self {
        let ch = self.ch;
        self.at(ms, Msg::Bend { ch, value })
    }
    /// Sets RPN 0, the pitch bend range, to `semitones` and `cents`, and
    /// deselects the RPN afterwards.
    fn bend_range(&mut self, ms: u32, semitones: u8, cents: u8) -> &mut Self {
        self.cc(ms, 101, 0).cc(ms, 100, 0).cc(ms, 6, semitones).cc(ms, 38, cents).cc(ms, 101, 127).cc(ms, 100, 127)
    }
}

fn seg(label: impl Into<String>, start_ms: u32, end_ms: u32) -> Segment {
    Segment { label: label.into(), start_ms, end_ms, pitch: None }
}

fn pitched(label: impl Into<String>, start_ms: u32, end_ms: u32, key: f64, from_ms: u32, to_ms: u32) -> Segment {
    Segment { label: label.into(), start_ms, end_ms, pitch: Some(Pitch { key, from_ms, to_ms }) }
}

struct Spec {
    id: String,
    category: &'static str,
    desc: String,
    sf2: Sf2,
    dur_ms: u32,
}

fn case(spec: Spec, score: &Score, segments: Vec<Segment>, focus: &[Metric]) -> Case {
    Case {
        id: spec.id,
        category: spec.category,
        desc: spec.desc,
        sf2: spec.sf2,
        group: Group::Dry,
        polyphony: POLYPHONY,
        dur_ms: spec.dur_ms,
        events: score.ev.clone(),
        segments,
        focus: focus.to_vec(),
    }
}

fn spec(category: &'static str, name: impl std::fmt::Display, sf2: Sf2, dur_ms: u32, desc: impl Into<String>) -> Spec {
    Spec { id: format!("{category}/{}/{name}", sf2.tag()), category, desc: desc.into(), sf2, dur_ms }
}

/// GM programs the per-program cases go through: piano, electric piano,
/// mallet, organ, guitar, bass, solo and ensemble strings, brass, flute,
/// synth lead, pad.
const PROGRAMS: &[(u8, &str)] = &[
    (0, "piano"),
    (4, "epiano"),
    (11, "vibraphone"),
    (16, "organ"),
    (24, "guitar"),
    (33, "bass"),
    (40, "violin"),
    (48, "strings"),
    (56, "trumpet"),
    (73, "flute"),
    (80, "square"),
    (89, "pad"),
];

const ALL_SF2: [Sf2; 3] = [Sf2::GeneralUser, Sf2::FluidR3, Sf2::TimGm6mb];

/// Every case.
pub fn all() -> Vec<Case> {
    let mut v = Vec::new();
    envelope(&mut v);
    velocity(&mut v);
    volume(&mut v);
    pan(&mut v);
    bend(&mut v);
    vibrato(&mut v);
    brightness(&mut v);
    sustain_loop(&mut v);
    hold_pedal(&mut v);
    drums(&mut v);
    exclusive(&mut v);
    polyphony(&mut v);
    program(&mut v);
    effects(&mut v);
    v
}

/// One note of each program, held 1.5 s and released: the volume envelope,
/// the pitch, the brightness and the release tail.
fn envelope(v: &mut Vec<Case>) {
    for sf2 in ALL_SF2 {
        let keys: &[u8] = if sf2 == Sf2::GeneralUser { &[36, 60, 84] } else { &[60] };
        for &(program, name) in PROGRAMS {
            for &key in keys {
                let mut s = Score::new(0);
                s.program(0, program).note(20, 1500, key, 100);
                v.push(case(
                    spec(
                        "env",
                        format!("{name}-{key}"),
                        sf2,
                        3500,
                        format!("program {program}, key {key}, velocity 100, 1.5 s"),
                    ),
                    &s,
                    vec![pitched("note", 0, 3500, key as f64, 300, 1400)],
                    ENVELOPE,
                ));
            }
        }
    }
}

/// The same note at velocities 1 to 127: the velocity to attenuation curve
/// and the velocity to filter cutoff (brightness).
fn velocity(v: &mut Vec<Case>) {
    const VELS: [u8; 9] = [1, 16, 32, 48, 64, 80, 96, 112, 127];
    let mut add = |sf2: Sf2, program: u8, name: &str| {
        let mut s = Score::new(0);
        s.program(0, program);
        let mut segs = Vec::new();
        for (i, &vel) in VELS.iter().enumerate() {
            let t = 20 + i as u32 * 1200;
            s.note(t, 600, 60, vel);
            segs.push(pitched(format!("vel {vel}"), t, t + 1200, 60.0, t + 150, t + 550));
        }
        v.push(case(
            spec("velocity", name, sf2, 20 + 9 * 1200, format!("program {program}, key 60, velocities {VELS:?}")),
            &s,
            segs,
            &[Loudness, Centroid, Bands, EnvCorr, EnvDev, Onset],
        ));
    };
    for (program, name) in [(0, "piano"), (48, "strings"), (56, "trumpet"), (81, "saw")] {
        add(Sf2::GeneralUser, program, name);
    }
    add(Sf2::FluidR3, 0, "piano");
    add(Sf2::TimGm6mb, 0, "piano");
}

/// CC7 and CC11 changed while a note is held, and both at once: their
/// curves to attenuation.
fn volume(v: &mut Vec<Case>) {
    const VALUES: [u8; 9] = [127, 100, 80, 64, 48, 32, 16, 1, 0];
    for (cc, name) in [(7u8, "cc7"), (11, "cc11")] {
        for (program, pname) in [(16u8, "organ"), (73, "flute")] {
            let mut s = Score::new(0);
            s.program(0, program);
            let mut segs = Vec::new();
            for (i, &val) in VALUES.iter().enumerate() {
                let t = 10 + i as u32 * 500;
                s.cc(t, cc, val);
                // Leave out the change's first 50 ms, where implementations
                // smooth it differently.
                segs.push(seg(format!("{name}={val}"), t + 50, t + 500));
            }
            s.on(20, 60, 100).off(10 + 9 * 500, 60);
            v.push(case(
                spec(
                    "volume",
                    format!("{name}-{pname}"),
                    Sf2::GeneralUser,
                    10 + 9 * 500 + 1500,
                    format!("program {program}, key 60 held, {name} stepped through {VALUES:?}"),
                ),
                &s,
                segs,
                LEVEL,
            ));
        }
    }
    // Both at once multiply (add in dB).
    let mut s = Score::new(0);
    s.program(0, 16).cc(0, 7, 64).cc(0, 11, 64).note(20, 1500, 60, 100);
    v.push(case(
        spec("volume", "cc7-cc11-organ", Sf2::GeneralUser, 3000, "program 16, CC7 = CC11 = 64"),
        &s,
        vec![seg("cc7=64 cc11=64", 100, 1500)],
        LEVEL,
    ));
}

/// CC10 before each note, and changed while a note is held: the pan law.
fn pan(v: &mut Vec<Case>) {
    const VALUES: [u8; 9] = [0, 16, 32, 48, 64, 80, 96, 112, 127];
    for (program, name) in [(73u8, "flute"), (0, "piano")] {
        let mut s = Score::new(0);
        s.program(0, program);
        let mut segs = Vec::new();
        for (i, &val) in VALUES.iter().enumerate() {
            let t = 20 + i as u32 * 700;
            s.cc(t - 10, 10, val).note(t, 400, 60, 100);
            segs.push(seg(format!("pan {val}"), t, t + 700));
        }
        v.push(case(
            spec(
                "pan",
                name,
                Sf2::GeneralUser,
                20 + 9 * 700 + 500,
                format!("program {program}, CC10 {VALUES:?} before each note"),
            ),
            &s,
            segs,
            &[Balance, Loudness],
        ));
    }
    let mut s = Score::new(0);
    s.program(0, 16).on(20, 60, 100);
    let mut segs = Vec::new();
    for (i, &val) in [0u8, 127, 32, 96, 64].iter().enumerate() {
        let t = 20 + i as u32 * 600;
        s.cc(t, 10, val);
        segs.push(seg(format!("pan {val}"), t + 50, t + 600));
    }
    s.off(20 + 5 * 600, 60);
    v.push(case(
        spec(
            "pan",
            "live-organ",
            Sf2::GeneralUser,
            20 + 5 * 600 + 1000,
            "program 16 held, CC10 changed during the note",
        ),
        &s,
        segs,
        &[Balance, Loudness],
    ));
    for sf2 in [Sf2::FluidR3, Sf2::TimGm6mb] {
        let mut s = Score::new(0);
        s.program(0, 73);
        let mut segs = Vec::new();
        for (i, &val) in [0u8, 32, 64, 96, 127].iter().enumerate() {
            let t = 20 + i as u32 * 700;
            s.cc(t - 10, 10, val).note(t, 400, 60, 100);
            segs.push(seg(format!("pan {val}"), t, t + 700));
        }
        v.push(case(
            spec("pan", "flute", sf2, 20 + 5 * 700 + 500, "program 73, CC10 0/32/64/96/127 before each note"),
            &s,
            segs,
            &[Balance, Loudness],
        ));
    }
}

/// Pitch bend at the default range, and after RPN 0 sets other ranges; an
/// NRPN in between must not change the selected RPN.
fn bend(v: &mut Vec<Case>) {
    const BENDS: [u16; 6] = [8192, 16383, 0, 12288, 4096, 8192];
    let mut add = |name: &str, program: u8, range: Option<(u8, u8)>, nrpn: bool, sf2: Sf2| {
        let mut s = Score::new(0);
        s.program(0, program);
        let semis = match range {
            Some((st, ct)) => {
                s.bend_range(0, st, ct);
                st as f64 + ct as f64 / 100.0
            }
            None => 2.0,
        };
        if nrpn {
            // Select RPN 0 again and set 12 semitones, then select a GS NRPN
            // (vibrato rate) and send data entry: the bend range must stay.
            s.cc(1, 101, 0).cc(1, 100, 0).cc(1, 6, 12).cc(1, 38, 0);
            s.cc(2, 99, 1).cc(2, 98, 8).cc(2, 6, 64);
        }
        s.on(20, 60, 100);
        let mut segs = Vec::new();
        for (i, &b) in BENDS.iter().enumerate() {
            let t = 20 + i as u32 * 600;
            s.bend(t, b);
            let expect = 60.0 + semis * (b as f64 - 8192.0) / 8192.0;
            segs.push(pitched(format!("bend {b}"), t, t + 600, expect, t + 150, t + 550));
        }
        s.off(20 + 6 * 600, 60);
        let desc = match range {
            Some((st, ct)) => format!("program {program}, RPN 0 = {st} semitones {ct} cents, bends {BENDS:?}"),
            None => format!("program {program}, default bend range, bends {BENDS:?}"),
        };
        v.push(case(spec("bend", name, sf2, 20 + 6 * 600 + 1000, desc), &s, segs, &[F0, Loudness]));
    };
    add("default-organ", 16, None, false, Sf2::GeneralUser);
    add("default-flute", 73, None, false, Sf2::GeneralUser);
    add("range12-organ", 16, Some((12, 0)), false, Sf2::GeneralUser);
    add("range1.50-organ", 16, Some((1, 50)), false, Sf2::GeneralUser);
    add("range24-organ", 16, Some((24, 0)), false, Sf2::GeneralUser);
    add("nrpn-keeps-rpn-organ", 16, Some((12, 0)), true, Sf2::GeneralUser);
    add("default-flute", 73, None, false, Sf2::FluidR3);
    add("range12-organ", 16, Some((12, 0)), false, Sf2::TimGm6mb);
}

/// The mod wheel (CC1) and channel pressure to vibrato depth; poly
/// pressure, which no default modulator uses.
fn vibrato(v: &mut Vec<Case>) {
    for (program, name) in [(16u8, "organ"), (73, "flute"), (48, "strings")] {
        for what in ["modwheel", "pressure", "polypressure"] {
            let values = [0u8, 64, 127];
            let mut s = Score::new(0);
            s.program(0, program).on(20, 69, 100);
            let mut segs = Vec::new();
            for (i, &val) in values.iter().enumerate() {
                let t = 20 + i as u32 * 1500;
                match what {
                    "modwheel" => s.cc(t, 1, val),
                    "pressure" => s.at(t, Msg::ChannelPressure { ch: 0, value: val }),
                    _ => s.at(t, Msg::PolyPressure { ch: 0, key: 69, value: val }),
                };
                segs.push(pitched(format!("{what} {val}"), t, t + 1500, 69.0, t + 400, t + 1450));
            }
            s.off(20 + 3 * 1500, 69);
            v.push(case(
                spec(
                    "vibrato",
                    format!("{what}-{name}"),
                    Sf2::GeneralUser,
                    20 + 3 * 1500 + 1000,
                    format!("program {program}, key 69 held, {what} {values:?}"),
                ),
                &s,
                segs,
                &[Vibrato, F0, Loudness],
            ));
        }
    }
}

/// CC74 (brightness) while a note is held: SpessaSynth maps it to the
/// filter cutoff by default, FluidSynth does not.
fn brightness(v: &mut Vec<Case>) {
    const VALUES: [u8; 5] = [64, 0, 32, 96, 127];
    for (program, name, key) in [(81u8, "saw", 48u8), (48, "strings", 60)] {
        let mut s = Score::new(0);
        s.program(0, program).on(20, key, 100);
        let mut segs = Vec::new();
        for (i, &val) in VALUES.iter().enumerate() {
            let t = 20 + i as u32 * 800;
            s.cc(t, 74, val);
            segs.push(seg(format!("cc74={val}"), t + 100, t + 800));
        }
        s.off(20 + 5 * 800, key);
        v.push(case(
            spec(
                "cc74",
                name,
                Sf2::GeneralUser,
                20 + 5 * 800 + 1000,
                format!("program {program}, key {key} held, CC74 {VALUES:?}"),
            ),
            &s,
            segs,
            &[Centroid, Bands, Loudness],
        ));
    }
}

/// Long notes of looped samples, low, high and very high (short loops
/// read with steps longer than the loop).
fn sustain_loop(v: &mut Vec<Case>) {
    for (program, name) in [(48u8, "strings"), (52, "choir"), (19, "church-organ"), (89, "pad"), (16, "organ")] {
        for key in [48u8, 72, 108] {
            if key == 108 && !matches!(program, 16 | 48) {
                continue;
            }
            let mut s = Score::new(0);
            s.program(0, program).note(20, 4000, key, 100);
            v.push(case(
                spec(
                    "loop",
                    format!("{name}-{key}"),
                    Sf2::GeneralUser,
                    5500,
                    format!("program {program}, key {key}, held 4 s"),
                ),
                &s,
                vec![pitched("note", 0, 5500, key as f64, 1000, 3900)],
                &[EnvCorr, EnvDev, Loudness, F0, Centroid, Tail],
            ));
        }
    }
    let mut s = Score::new(0);
    s.program(0, 48).note(20, 4000, 60, 100);
    for sf2 in [Sf2::FluidR3, Sf2::TimGm6mb] {
        v.push(case(
            spec("loop", "strings-60", sf2, 5500, "program 48, key 60, held 4 s"),
            &s,
            vec![pitched("note", 0, 5500, 60.0, 1000, 3900)],
            &[EnvCorr, EnvDev, Loudness, F0, Centroid, Tail],
        ));
    }
}

/// The hold pedal (CC64): notes released under it keep sounding until it
/// goes up; a key struck again under it; a value just below the switch
/// point.
fn hold_pedal(v: &mut Vec<Case>) {
    let focus = &[Tail, EnvCorr, EnvDev, Loudness];
    for (program, name) in [(0u8, "piano"), (16, "organ")] {
        let mut s = Score::new(0);
        s.program(0, program).cc(10, 64, 127).note(20, 280, 60, 100).cc(2000, 64, 0);
        v.push(case(
            spec(
                "pedal",
                format!("hold-{name}"),
                Sf2::GeneralUser,
                4500,
                format!("program {program}: pedal down, note 20–300 ms, pedal up at 2000 ms"),
            ),
            &s,
            vec![seg("held", 20, 2000), seg("after pedal up", 2000, 4500)],
            focus,
        ));
    }
    let mut s = Score::new(0);
    s.program(0, 0).cc(10, 64, 127).note(20, 180, 60, 100).note(1000, 200, 60, 100).cc(2500, 64, 0);
    v.push(case(
        spec(
            "pedal",
            "restrike-piano",
            Sf2::GeneralUser,
            5000,
            "program 0: pedal down, key 60 struck at 20 and 1000 ms, pedal up at 2500 ms",
        ),
        &s,
        vec![seg("first", 20, 1000), seg("restruck", 1000, 2500), seg("after pedal up", 2500, 5000)],
        focus,
    ));
    for val in [63u8, 64] {
        let mut s = Score::new(0);
        s.program(0, 16).cc(10, 64, val).note(20, 280, 60, 100).cc(2000, 64, 0);
        v.push(case(
            spec(
                "pedal",
                format!("value{val}-organ"),
                Sf2::GeneralUser,
                3500,
                format!("program 16: CC64 = {val}, note 20–300 ms, CC64 = 0 at 2000 ms"),
            ),
            &s,
            vec![seg("pedal", 20, 2000), seg("after", 2000, 3500)],
            focus,
        ));
    }
    let mut s = Score::new(0);
    s.program(0, 0).cc(10, 64, 127).note(20, 280, 60, 100).cc(2000, 64, 0);
    v.push(case(
        spec("pedal", "hold-piano", Sf2::FluidR3, 4500, "program 0: pedal down, note 20–300 ms, pedal up at 2000 ms"),
        &s,
        vec![seg("held", 20, 2000), seg("after pedal up", 2000, 4500)],
        focus,
    ));
}

const DRUM_KEYS: &[(u8, &str)] = &[
    (35, "kick2"),
    (36, "kick"),
    (38, "snare"),
    (39, "clap"),
    (42, "closed-hh"),
    (45, "low-tom"),
    (46, "open-hh"),
    (49, "crash"),
    (51, "ride"),
    (56, "cowbell"),
];

/// Hits on channel 10 (bank 128), one at a time, for each kit.
fn drums(v: &mut Vec<Case>) {
    let kits: &[(u8, &str)] = &[
        (0, "standard"),
        (8, "room"),
        (16, "power"),
        (24, "electronic"),
        (25, "808"),
        (32, "jazz"),
        (40, "brush"),
        (48, "orchestra"),
    ];
    for sf2 in ALL_SF2 {
        for &(kit, name) in kits {
            if sf2 != Sf2::GeneralUser && kit != 0 {
                continue;
            }
            let mut s = Score::new(9);
            s.program(0, kit);
            let mut segs = Vec::new();
            for (i, &(key, kname)) in DRUM_KEYS.iter().enumerate() {
                let t = 20 + i as u32 * 900;
                s.note(t, 100, key, 100);
                segs.push(seg(format!("{key} {kname}"), t, t + 900));
            }
            v.push(case(
                spec(
                    "drums",
                    name,
                    sf2,
                    20 + DRUM_KEYS.len() as u32 * 900 + 1500,
                    format!("channel 10, kit {kit}, one hit of each of {} keys", DRUM_KEYS.len()),
                ),
                &s,
                segs,
                &[EnvCorr, EnvDev, Loudness, Centroid, Onset, Tail, Balance],
            ));
        }
    }
    // A kit program the SoundFonts lack: should fall back to kit 0.
    for sf2 in ALL_SF2 {
        let mut s = Score::new(9);
        s.program(0, 20);
        let mut segs = Vec::new();
        for (i, &key) in [36u8, 38, 42].iter().enumerate() {
            let t = 20 + i as u32 * 900;
            s.note(t, 100, key, 100);
            segs.push(seg(format!("{key}"), t, t + 900));
        }
        v.push(case(
            spec("drums", "missing-kit20", sf2, 20 + 3 * 900 + 1500, "channel 10, kit 20 (absent), keys 36/38/42"),
            &s,
            segs,
            &[Loudness, EnvCorr, Centroid],
        ));
    }
    // GS reset, then "use for rhythm part" makes channel 11 a drum channel.
    let mut s = Score::new(10);
    s.at(0, Msg::SysEx(vec![0xF0, 0x41, 0x10, 0x42, 0x12, 0x40, 0x00, 0x7F, 0x00, 0x41, 0xF7]));
    s.at(60, Msg::SysEx(gs_sysex([0x40, 0x1A, 0x15], 1)));
    s.program(100, 0);
    let mut segs = Vec::new();
    for (i, &key) in [36u8, 38, 42].iter().enumerate() {
        let t = 120 + i as u32 * 900;
        s.note(t, 100, key, 100);
        segs.push(seg(format!("{key}"), t, t + 900));
    }
    v.push(case(
        spec(
            "drums",
            "gs-rhythm-part-ch11",
            Sf2::GeneralUser,
            120 + 3 * 900 + 1500,
            "GS reset, channel 11 set to rhythm part by sysex, keys 36/38/42",
        ),
        &s,
        segs,
        &[Loudness, EnvCorr, Centroid],
    ));
}

/// A GS data set (DT1) message to `addr` of one byte.
fn gs_sysex(addr: [u8; 3], data: u8) -> Vec<u8> {
    let sum: u32 = addr.iter().map(|&b| b as u32).sum::<u32>() + data as u32;
    let checksum = ((128 - sum % 128) % 128) as u8;
    vec![0xF0, 0x41, 0x10, 0x42, 0x12, addr[0], addr[1], addr[2], data, checksum, 0xF7]
}

/// Exclusive classes: a closed or pedal hi-hat stops an open one on the
/// same channel, not on another channel.
fn exclusive(v: &mut Vec<Case>) {
    let focus = &[Tail, EnvDev, Loudness, EnvCorr];
    for sf2 in ALL_SF2 {
        for (key, name) in [(42u8, "closed"), (44, "pedal")] {
            let mut s = Score::new(9);
            s.program(0, 0).note(20, 100, 46, 110).note(300, 100, key, 90);
            v.push(case(
                spec(
                    "exclusive",
                    format!("open-then-{name}"),
                    sf2,
                    2500,
                    format!("channel 10: open hi-hat at 20 ms, {name} hi-hat ({key}) at 300 ms"),
                ),
                &s,
                vec![seg("open", 20, 300), seg(name, 300, 2500)],
                focus,
            ));
        }
    }
    let mut s = Score::new(9);
    s.program(0, 0).note(20, 100, 46, 110).note(300, 100, 46, 110);
    v.push(case(
        spec("exclusive", "open-twice", Sf2::GeneralUser, 2500, "channel 10: open hi-hat at 20 and 300 ms"),
        &s,
        vec![seg("first", 20, 300), seg("second", 300, 2500)],
        focus,
    ));
    // Channel 11 made a drum channel; its closed hi-hat must not stop
    // channel 10's open one.
    let mut s = Score::new(9);
    s.at(0, Msg::SysEx(vec![0xF0, 0x41, 0x10, 0x42, 0x12, 0x40, 0x00, 0x7F, 0x00, 0x41, 0xF7]));
    s.at(60, Msg::SysEx(gs_sysex([0x40, 0x1A, 0x15], 1)));
    s.program(100, 0).at(100, Msg::Program { ch: 10, program: 0 });
    s.note(120, 100, 46, 110)
        .at(400, Msg::NoteOn { ch: 10, key: 42, vel: 90 })
        .at(500, Msg::NoteOff { ch: 10, key: 42 });
    v.push(case(
        spec(
            "exclusive",
            "other-channel",
            Sf2::GeneralUser,
            2600,
            "open hi-hat on channel 10 at 120 ms, closed hi-hat on channel 11 (GS rhythm part) at 400 ms",
        ),
        &s,
        vec![seg("open", 120, 400), seg("closed on ch 11", 400, 2600)],
        focus,
    ));
}

/// More notes than voices: which voices are stolen, and how.
fn polyphony(v: &mut Vec<Case>) {
    // `len` of 0 holds every note to the end of the last step.
    let mut add = |name: &str, program: u8, voices: u32, keys: &[u8], step: u32, len: u32| {
        let mut s = Score::new(0);
        s.program(0, program);
        let mut segs = Vec::new();
        for (i, &key) in keys.iter().enumerate() {
            let t = 20 + i as u32 * step;
            let len = if len == 0 { (keys.len() - i) as u32 * step } else { len };
            s.note(t, len, key, 100);
            segs.push(seg(format!("+{key}"), t, t + step));
        }
        let end = 20 + keys.len() as u32 * step;
        let mut c = case(
            spec(
                "polyphony",
                name,
                Sf2::GeneralUser,
                end + 2000,
                format!("program {program}, {voices} voices, {} notes {step} ms apart", keys.len()),
            ),
            &s,
            segs,
            &[Loudness, EnvDev, EnvCorr],
        );
        c.segments.push(seg("release", end, end + 2000));
        c.polyphony = voices;
        v.push(c);
    };
    // A chord built note by note, all held to the same end.
    add("organ-8voices", 16, 8, &[48, 52, 55, 60, 64, 67, 72, 76, 79, 84, 88, 91], 250, 0);
    // Fast repeated notes, released: the released voices should go first.
    add("piano-4voices", 0, 4, &[60, 62, 64, 65, 67, 69, 71, 72, 74, 76], 150, 120);
}

/// Program and bank changes: when they take effect.
fn program(v: &mut Vec<Case>) {
    let focus = &[Loudness, Centroid, EnvCorr, EnvDev, F0];
    // A program change at the same time as a note on, before it and after it.
    let mut s = Score::new(0);
    s.program(0, 0).note(20, 600, 60, 100).program(1000, 40).note(1000, 600, 60, 100);
    s.note(2000, 600, 60, 100).program(2000, 73);
    v.push(case(
        spec(
            "program",
            "same-tick",
            Sf2::GeneralUser,
            3600,
            "piano at 20 ms; at 1000 ms program 40 then a note; at 2000 ms a note then program 73",
        ),
        &s,
        vec![
            pitched("piano", 20, 1000, 60.0, 200, 600),
            pitched("violin (pc before note)", 1000, 2000, 60.0, 1300, 1600),
            pitched("violin (pc after note)", 2000, 3600, 60.0, 2300, 2600),
        ],
        focus,
    ));
    // A program change while a note sounds: the note keeps its preset.
    let mut s = Score::new(0);
    s.program(0, 16).note(20, 2000, 60, 100).program(500, 73).note(1000, 1000, 67, 100);
    v.push(case(
        spec(
            "program",
            "during-note",
            Sf2::GeneralUser,
            3500,
            "organ key 60 held 20–2020 ms, program 73 at 500 ms, key 67 at 1000 ms",
        ),
        &s,
        vec![seg("organ alone", 20, 1000), seg("organ + flute", 1000, 2020), seg("release", 2020, 3500)],
        focus,
    ));
    // Bank select takes effect at the next program change only.
    for sf2 in [Sf2::GeneralUser, Sf2::FluidR3] {
        let mut s = Score::new(0);
        s.program(0, 16).note(20, 600, 60, 100).cc(800, 0, 8).note(1000, 600, 60, 100);
        s.program(1900, 16).note(2000, 600, 60, 100);
        v.push(case(
            spec(
                "program",
                "bank-select-waits",
                sf2,
                3600,
                "organ; bank 8 selected at 800 ms, note at 1000 ms; program 16 at 1900 ms, note at 2000 ms (bank 8)",
            ),
            &s,
            vec![seg("bank 0", 20, 800), seg("bank 8 selected, no pc", 1000, 1900), seg("bank 8 after pc", 2000, 3600)],
            focus,
        ));
    }
    // A bank the SoundFont lacks: falls back to bank 0.
    for sf2 in ALL_SF2 {
        let mut s = Score::new(0);
        s.cc(0, 0, 5).program(0, 0).note(20, 600, 60, 100);
        v.push(case(
            spec("program", "missing-bank5", sf2, 1800, "bank 5 (absent), program 0"),
            &s,
            vec![pitched("note", 0, 1800, 60.0, 200, 600)],
            focus,
        ));
    }
}

/// Reverb and chorus sends; the effects run.
fn effects(v: &mut Vec<Case>) {
    let mut fx = |c: Case| {
        let mut c = c;
        c.group = Group::Fx;
        v.push(c);
    };
    let focus = &[EnvCorr, EnvDev, Loudness, Tail, Centroid, Balance];
    for sf2 in [Sf2::GeneralUser, Sf2::FluidR3] {
        let mut s = Score::new(0);
        s.program(0, 0).cc(0, 93, 0);
        let mut segs = Vec::new();
        for (i, &send) in [0u8, 40, 127].iter().enumerate() {
            let t = 20 + i as u32 * 3000;
            s.cc(t - 10, 91, send).note(t, 200, 60, 100);
            segs.push(seg(format!("reverb {send}"), t, t + 3000));
        }
        fx(case(spec("fx", "reverb-piano", sf2, 9500, "piano staccato, CC91 0/40/127, CC93 0"), &s, segs, focus));
    }
    let mut s = Score::new(0);
    s.program(0, 48).cc(0, 91, 0);
    let mut segs = Vec::new();
    for (i, &send) in [0u8, 64, 127].iter().enumerate() {
        let t = 20 + i as u32 * 3000;
        s.cc(t - 10, 93, send).note(t, 1500, 60, 100);
        segs.push(seg(format!("chorus {send}"), t, t + 3000));
    }
    fx(case(
        spec("fx", "chorus-strings", Sf2::GeneralUser, 9500, "strings 1.5 s, CC93 0/64/127, CC91 0"),
        &s,
        segs,
        focus,
    ));
    // A quiet note still feeds the effects.
    let mut s = Score::new(0);
    s.program(0, 0).cc(0, 7, 20).cc(0, 91, 127).cc(0, 93, 127).note(20, 200, 60, 10);
    fx(case(
        spec("fx", "quiet-send", Sf2::GeneralUser, 3500, "piano velocity 10, CC7 20, CC91 = CC93 = 127"),
        &s,
        vec![seg("note", 20, 3500)],
        focus,
    ));
    // Defaults: no send set.
    let mut s = Score::new(0);
    s.program(0, 0).note(20, 200, 60, 100);
    let mut d = Score::new(9);
    d.note(1500, 100, 38, 100);
    s.ev.extend(d.ev);
    fx(case(
        spec("fx", "default-sends", Sf2::GeneralUser, 4500, "piano and snare with the default sends"),
        &s,
        vec![seg("piano", 20, 1500), seg("snare", 1500, 4500)],
        focus,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_segments_in_range() {
        let cases = all();
        let mut ids: Vec<_> = cases.iter().map(|c| c.id.as_str()).collect();
        ids.sort();
        let n = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), n, "duplicate case ids");
        for c in &cases {
            for s in &c.segments {
                assert!(s.start_ms < s.end_ms && s.end_ms <= c.dur_ms, "{}: {}", c.id, s.label);
                if let Some(p) = &s.pitch {
                    assert!(p.from_ms < p.to_ms && p.to_ms <= c.dur_ms, "{}: {}", c.id, s.label);
                }
            }
            assert!(c.events.iter().all(|e| e.ms <= c.dur_ms), "{}", c.id);
        }
    }

    #[test]
    fn gs_checksum() {
        // Use-for-rhythm-part of part 11, map 1, from the SC-88 manual's
        // checksum rule: the address and data bytes and the checksum sum
        // to a multiple of 128.
        let m = gs_sysex([0x40, 0x1A, 0x15], 1);
        let sum: u32 = m[5..10].iter().map(|&b| b as u32).sum();
        assert_eq!(sum % 128, 0);
    }
}
