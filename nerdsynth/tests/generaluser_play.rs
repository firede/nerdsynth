//! Plays every preset of GeneralUser GS 2.0.3 at a few keys and velocities,
//! with the controllers at their extremes, and checks that the output is
//! finite and of a sensible level. Skipped when the file is not there
//! (`~/.nerdice/soundfonts/GeneralUser-GS-2.0.3.sf2`, or the path in
//! `NERDSYNTH_GU_SF2`).

use std::path::PathBuf;
use std::sync::Arc;

use nerdsynth::{Config, Event, SoundFont, Synth};

fn path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("NERDSYNTH_GU_SF2") {
        return Some(p.into());
    }
    let home = std::env::var_os("HOME")?;
    let p = PathBuf::from(home).join(".nerdice/soundfonts/GeneralUser-GS-2.0.3.sf2");
    p.exists().then_some(p)
}

fn cc(channel: u8, controller: u8, value: u8) -> Event {
    Event::ControlChange { channel, controller, value }
}

/// The controller settings played: the defaults, everything at its top, and
/// everything at its bottom (but the volume, which would be silent).
fn settings(channel: u8) -> [Vec<Event>; 3] {
    let all = |v: u8, bend: u16| {
        let mut e: Vec<Event> = [1, 7, 10, 11, 71, 72, 73, 74, 91, 93]
            .iter()
            .map(|&c| cc(channel, c, if c == 7 { 127 } else { v }))
            .collect();
        e.push(Event::PitchBend { channel, value: bend });
        e.push(Event::ChannelPressure { channel, pressure: v });
        e.extend([cc(channel, 101, 0), cc(channel, 100, 0), cc(channel, 6, 24), cc(channel, 101, 127)]);
        e
    };
    [Vec::new(), all(127, 16383), all(0, 0)]
}

#[test]
fn every_preset_plays_finite_and_sensible() {
    let Some(path) = path() else {
        eprintln!("GeneralUser GS not found; skipped");
        return;
    };
    let sf = Arc::new(SoundFont::read(std::fs::File::open(&path).unwrap()).unwrap());
    let mut synth = Synth::new(Arc::clone(&sf), Config::new(44100)).unwrap();
    let (mut l, mut r) = (vec![0.0f32; 4410], vec![0.0f32; 4410]);
    let (mut loudest, mut quiet) = ((0.0f32, String::new()), Vec::new());
    for preset in sf.presets() {
        // Drum kits on channel 9, the rest on channel 0.
        let channel = if preset.bank() == 128 { 9 } else { 0 };
        for (s, events) in settings(channel).into_iter().enumerate() {
            synth.reset();
            if preset.bank() != 128 {
                synth.send(cc(channel, 0, preset.bank() as u8));
            }
            synth.send(Event::ProgramChange { channel, program: preset.program() as u8 });
            for e in events {
                synth.send(e);
            }
            let mut peak = 0.0f32;
            for key in [24u8, 60, 108] {
                for velocity in [1u8, 127] {
                    synth.send(Event::NoteOn { channel, key, velocity });
                    synth.render(&mut l[..2205], &mut r[..2205]);
                    synth.send(Event::NoteOff { channel, key });
                    synth.render(&mut l[..441], &mut r[..441]);
                    for x in l[..2205].iter().chain(&r[..2205]) {
                        assert!(
                            x.is_finite(),
                            "{} {}:{} key {key} vel {velocity} setting {s}",
                            preset.name(),
                            preset.bank(),
                            preset.program()
                        );
                        peak = peak.max(x.abs());
                    }
                }
            }
            // Let the tails ring out and check them too.
            for _ in 0..5 {
                synth.render(&mut l, &mut r);
                assert!(l.iter().chain(&r).all(|x| x.is_finite()), "{} tail", preset.name());
            }
            let name = format!("{} {}:{} setting {s}", preset.name(), preset.bank(), preset.program());
            // Six notes overlap at most in their releases: more than
            // 4.0 (+12 dBFS) would be a gain gone wrong.
            assert!(peak < 4.0, "{name}: peak {peak}");
            if s == 0 && peak < 1e-3 {
                quiet.push(name.clone());
            }
            if peak > loudest.0 {
                loudest = (peak, name);
            }
        }
    }
    eprintln!("{} presets; loudest {:.3} ({}); nearly silent: {quiet:?}", sf.presets().len(), loudest.0, loudest.1);
    assert!(quiet.len() <= 2, "nearly silent presets: {quiet:?}");
}
