//! How many times faster than real time the synthesizer renders, at
//! 44.1 kHz, with 32, 128 and 256 voices sounding, with and without the
//! effects: `cargo run --release --example bench [path.sf2]`. Plays
//! GeneralUser GS's organ (sustained, looped, two voices a note) when no
//! path is given and it is at `~/.nerdice/soundfonts`.

use std::sync::Arc;
use std::time::Instant;

use nerdsynth::synth::Interpolation;
use nerdsynth::{Config, Event, SoundFont, Synth};

fn main() {
    let path = std::env::args().nth(1).map(std::path::PathBuf::from).or_else(|| {
        let p =
            std::path::PathBuf::from(std::env::var_os("HOME")?).join(".nerdice/soundfonts/GeneralUser-GS-2.0.3.sf2");
        p.exists().then_some(p)
    });
    let Some(path) = path else {
        eprintln!("usage: bench <file.sf2>");
        std::process::exit(2);
    };
    let sf = Arc::new(SoundFont::read(std::fs::File::open(&path).unwrap()).unwrap());
    const RATE: u32 = 44100;
    const BLOCK: usize = 512;
    let seconds = 10;
    println!("{}", path.display());
    println!("voices  effects  interpolation  realtime x");
    for voices in [32usize, 128, 256] {
        for effects in [false, true] {
            for interpolation in [Interpolation::Cubic, Interpolation::Linear] {
                let mut c = Config::new(RATE);
                c.max_voices = voices;
                c.effects = effects;
                c.interpolation = interpolation;
                let mut synth = Synth::new(Arc::clone(&sf), c).unwrap();
                // Organ (program 19) on 15 channels, a little detuned so that
                // the voices are not in step; notes until the voices are full.
                let mut k = 0u32;
                while synth.active_voices() < voices && k < 15 * 128 {
                    let channel = (k % 15) as u8;
                    let channel = if channel >= 9 { channel + 1 } else { channel };
                    if k < 15 {
                        synth.send(Event::ProgramChange { channel, program: 19 });
                        synth.send(Event::PitchBend { channel, value: 8192 + 40 * k as u16 });
                        synth.send(Event::ControlChange { channel, controller: 91, value: 60 });
                        synth.send(Event::ControlChange { channel, controller: 93, value: 30 });
                    }
                    synth.send(Event::NoteOn { channel, key: 36 + (k / 15 * 7 % 60) as u8, velocity: 100 });
                    k += 1;
                }
                let (mut l, mut r) = (vec![0.0f32; BLOCK], vec![0.0f32; BLOCK]);
                // Warm up past the attacks.
                for _ in 0..RATE as usize / BLOCK {
                    synth.render(&mut l, &mut r);
                }
                let sounding = synth.active_voices();
                let blocks = seconds * RATE as usize / BLOCK;
                let started = Instant::now();
                for _ in 0..blocks {
                    synth.render(&mut l, &mut r);
                    std::hint::black_box((&l, &r));
                }
                let took = started.elapsed().as_secs_f64();
                let audio = (blocks * BLOCK) as f64 / f64::from(RATE);
                println!("{sounding:>6}  {:>7}  {:>13}  {:>10.1}", effects, format!("{interpolation:?}"), audio / took);
            }
        }
    }
}
