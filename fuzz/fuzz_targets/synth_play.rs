//! A SoundFont built from the input (generators, modulators, sample shape
//! and loop) played by events from the input: no panic, no NaN or
//! infinity, and no render call that takes long.
#![no_main]

use std::sync::Arc;
use std::time::{Duration, Instant};

use libfuzzer_sys::fuzz_target;
use nerdsynth::event::SysEx;
use nerdsynth::synth::Interpolation;
use nerdsynth::testkit::sf2::{SampleSpec, Sf2, Zone, raw_modulator, raw_source};
use nerdsynth::{Config, Event, Synth};

struct Input<'a>(&'a [u8]);

impl Input<'_> {
    fn u8(&mut self) -> u8 {
        let Some((&b, rest)) = self.0.split_first() else { return 0 };
        self.0 = rest;
        b
    }
    fn u16(&mut self) -> u16 {
        u16::from(self.u8()) | u16::from(self.u8()) << 8
    }
    fn empty(&self) -> bool {
        self.0.is_empty()
    }
}

fn zone(input: &mut Input, sample: u16) -> Zone {
    let mut z = Zone::new();
    for _ in 0..input.u8() % 16 {
        let g = u16::from(input.u8() % 61);
        // Not the range, index and instrument generators: the builder sets them.
        if !matches!(g, 41..=44 | 53) {
            z = z.raw(g, input.u16());
        }
    }
    for _ in 0..input.u8() % 3 {
        // The arguments are taken from the input in this order.
        z = z.modulator(raw_modulator(
            raw_source(input.u16()),
            u16::from(input.u8() % 64),
            input.u16() as i16,
            raw_source(input.u16()),
            u16::from(input.u8() % 3),
        ));
    }
    z.sample(sample)
}

fuzz_target!(|data: &[u8]| {
    let mut input = Input(data);
    let len = 1 + usize::from(input.u16() % 4096);
    let shape = input.u8();
    let points: Vec<i16> = (0..len)
        .map(|i| match shape % 4 {
            0 => ((i as f64 * 0.3).sin() * 30000.0) as i16,
            1 => if i % 2 == 0 { i16::MAX } else { i16::MIN },
            2 => (i as i16).wrapping_mul(977),
            _ => 0,
        })
        .collect();
    let mut spec = SampleSpec::new("s", points).key(input.u8()).rate(u32::from(input.u16()).max(1) * 4);
    let (ls, le) = (u32::from(input.u16()), u32::from(input.u16()));
    spec = spec.looped(ls, le);
    if shape & 4 != 0 {
        spec = spec.low_bytes(vec![0xAB; len]);
    }
    let mut b = Sf2::new();
    let s = b.sample(spec);
    // Now and then many regions a note: up to 256 instrument zones, reached
    // through up to 64 preset zones.
    let many = input.u8() % 8 == 0;
    let zone_count = if many { 1 + usize::from(input.u8()) } else { 1 + usize::from(input.u8() % 3) };
    let zones: Vec<Zone> = (0..zone_count).map(|_| zone(&mut input, s)).collect();
    let i = b.instrument("i", zones);
    let mut preset = Zone::new();
    for _ in 0..input.u8() % 8 {
        let g = u16::from(input.u8() % 61);
        if !matches!(g, 41..=44 | 53 | 54 | 57 | 58) {
            preset = preset.raw(g, input.u16());
        }
    }
    let preset = preset.instrument(i);
    let preset_zones = if many { 1 + usize::from(input.u8() % 64) } else { 1 };
    b.preset("p", 0, 0, vec![preset; preset_zones]);
    b.preset("d", 128, 0, [Zone::new().instrument(i)]);
    let Ok(sf) = nerdsynth::SoundFont::from_bytes(&b.build()) else { return };

    let rates = [8000, 22050, 44100, 48000, 96000, 192_000];
    let mut c = Config::new(rates[usize::from(input.u8()) % rates.len()]);
    c.max_voices = if many { 1 + usize::from(input.u16() % 1024) } else { 1 + usize::from(input.u8() % 64) };
    c.event_capacity = 16 + usize::from(input.u8());
    c.effects = input.u8() % 2 == 0;
    c.interpolation = if input.u8() % 2 == 0 { Interpolation::Cubic } else { Interpolation::Linear };
    c.min_note_length = std::time::Duration::from_millis(u64::from(input.u8()) * 4);
    c.gain = f32::from(input.u8()) / 64.0;
    let Ok(mut synth) = Synth::new(Arc::new(sf), c) else { return };

    let mut l = vec![0.0f32; 4096];
    let mut r = vec![0.0f32; 4096];
    let mut renders = 0;
    while !input.empty() && renders < 64 {
        let channel = input.u8() % 17;
        let event = match input.u8() % 10 {
            0..=2 => Event::NoteOn { channel, key: input.u8(), velocity: input.u8() },
            3 => Event::NoteOff { channel, key: input.u8() },
            4 => Event::ControlChange { channel, controller: input.u8(), value: input.u8() },
            5 => Event::PitchBend { channel, value: input.u16() },
            6 => Event::ProgramChange { channel, program: input.u8() },
            7 => Event::PolyPressure { channel, key: input.u8(), pressure: input.u8() },
            8 => {
                let n = usize::from(input.u8() % 16);
                let bytes: Vec<u8> = (0..n).map(|_| input.u8() & 0x7F).collect();
                match SysEx::new(&bytes) {
                    Some(s) => Event::SysEx(s),
                    None => continue,
                }
            }
            _ => {
                let n = usize::from(input.u16()) % 4097;
                let started = Instant::now();
                synth.render(&mut l[..n], &mut r[..n]);
                let took = started.elapsed();
                assert!(took < Duration::from_millis(500), "render of {n} frames took {took:?}");
                assert!(l[..n].iter().chain(&r[..n]).all(|x| x.is_finite()));
                renders += 1;
                continue;
            }
        };
        if input.u8() % 2 == 0 {
            let started = Instant::now();
            synth.send(event);
            let took = started.elapsed();
            assert!(took < Duration::from_millis(500), "{event:?} took {took:?}");
        } else {
            let at = synth.now() + u64::from(input.u16());
            let _ = synth.schedule(at, event);
        }
    }
    for _ in 0..4 {
        synth.render(&mut l, &mut r);
        assert!(l.iter().chain(&r).all(|x| x.is_finite()));
    }
});
