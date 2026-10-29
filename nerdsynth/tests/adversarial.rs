//! Adversarial probes, from the independent reviews: hostile SoundFonts and
//! MIDI files, floods of costly events, a host sharing the queue with a
//! sequencer, and the render lengths under the work budget. Most measure
//! time, so all are ignored by default; run them in release, one at a time:
//!
//! ```text
//! mise run adversarial
//! ```
//!
//! Each prints what it measured and checks a generous bound: keeping up with
//! real time with room to spare, several times what the development machine
//! (Apple Silicon) measures, so that a regression of the kind the reviews
//! found (hundreds of milliseconds, seconds) fails and noise does not. In a
//! debug build the checks that time nothing still hold. `spec/decisions.md`
//! has the numbers each one guards; `cost_report` prints what the budget's
//! units stand for on the machine at hand.
#![cfg(feature = "testkit")]

use std::sync::Arc;
use std::time::{Duration, Instant};

use nerdsynth::event::SysEx;
use nerdsynth::midi::LoopMarkers;
use nerdsynth::soundfont::GeneratorType as G;
use nerdsynth::testkit::sf2::{SampleSpec, Sf2, Zone, modulator, raw_modulator, raw_source, source};
use nerdsynth::testkit::smf::{SmfBuilder, Track};
use nerdsynth::{Config, Event, Sequencer, Smf, SoundFont, Synth};

/// Whether times mean anything: an optimized build.
const TIMED: bool = !cfg!(debug_assertions);

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

/// Prints `took` and, in an optimized build, checks it is within `limit`.
fn within(what: &str, took: Duration, limit: Duration) {
    println!("{what}: {:.3} ms (limit {:.0} ms)", ms(took), ms(limit));
    if TIMED {
        assert!(took <= limit, "{what}: {:.3} ms, over {:.0} ms", ms(took), ms(limit));
    }
}

fn timed<T>(f: impl FnOnce() -> T) -> (T, Duration) {
    let t = Instant::now();
    let x = f();
    (x, t.elapsed())
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn on(channel: u8, key: u8) -> Event {
    Event::NoteOn { channel, key, velocity: 100 }
}

fn cc(channel: u8, controller: u8, value: u8) -> Event {
    Event::ControlChange { channel, controller, value }
}

fn gs_reset() -> Event {
    Event::SysEx(SysEx::new(&[0x41, 0x10, 0x42, 0x12, 0x40, 0x00, 0x7F, 0x00, 0x41]).unwrap())
}

/// A short sample, not looped: voices end soon, so that a flood measures the
/// events' work rather than the voices'.
fn short() -> SampleSpec {
    SampleSpec::new("short", vec![8000; 256])
}

/// A looped sample.
fn held() -> SampleSpec {
    SampleSpec::new("held", (0..2000).map(|i| ((i * 37 % 200) as i16 - 100) * 100).collect()).looped(100, 1900)
}

/// `n` distinct modulators of the CCs, for zones.
fn modulators(zone: Zone, n: usize) -> Zone {
    let dests = [G::InitialFilterFc, G::Pan, G::FineTune, G::CoarseTune, G::ReverbEffectsSend, G::ChorusEffectsSend];
    let mut zone = zone;
    for k in 0..n {
        let cc = [1u8, 2, 4, 5, 8, 9, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21][k / 4 % 16];
        let m = modulator(
            source(cc, true, false, false, (k % 4) as u8),
            dests[k / 64 % dests.len()],
            1,
            Default::default(),
        );
        zone = zone.modulator(m);
    }
    zone
}

/// A preset of `regions` instrument zones of `sample`, each with `mods`
/// modulators.
fn font(sample: SampleSpec, regions: usize, mods: usize, looped: bool) -> Arc<SoundFont> {
    let mut b = Sf2::new();
    let s = b.sample(sample);
    let zone = modulators(Zone::new().set(G::SampleModes, i16::from(looped)), mods);
    let i = b.instrument("i", (0..regions).map(|_| zone.clone().sample(s)));
    b.preset("p", 0, 0, [Zone::new().instrument(i)]);
    Arc::new(b.load())
}

/// A preset of a million region pairs, none of which plays key 0.
fn unmatched_pairs() -> Arc<SoundFont> {
    let mut b = Sf2::new();
    let s = b.sample(short());
    let i = b.instrument("i", (0..1000).map(|_| Zone::new().keys(1, 127).sample(s)));
    b.preset("p", 0, 0, (0..1000).map(|_| Zone::new().instrument(i)));
    Arc::new(b.load())
}

fn synth(sf: &Arc<SoundFont>, rate: u32, voices: usize) -> Synth {
    let mut c = Config::new(rate);
    c.max_voices = voices;
    Synth::new(Arc::clone(sf), c).unwrap()
}

/// A second of audio in renders of 64 frames, the queue filled with `event`
/// before each: the total time and the worst render.
fn flood(s: &mut Synth, event: &dyn Fn(u64) -> Event) -> (Duration, Duration) {
    let (mut l, mut r) = (vec![0.0f32; 64], vec![0.0f32; 64]);
    let (mut total, mut worst) = (Duration::ZERO, Duration::ZERO);
    let mut k = 0;
    for _ in 0..s.config().sample_rate / 64 {
        let now = s.now();
        while s.schedule(now, event(k)).is_ok() {
            k += 1;
        }
        let ((), took) = timed(|| {
            s.render(&mut l, &mut r);
        });
        assert!(l.iter().chain(&r).all(|x| x.is_finite()));
        total += took;
        worst = worst.max(took);
    }
    (total, worst)
}

/// Floods of the costliest events keep up with real time, at every sample
/// rate and at a high polyphony, and no render takes long: the work budget
/// (rounds 3 and 4: 2.1 s for one render; 48 kHz throughput collapsing).
#[test]
#[ignore]
fn floods_of_costly_events_keep_up() {
    let costly = font(short(), 256, 64, false);
    let one = font(short(), 1, 0, false);
    let unmatched = unmatched_pairs();
    let note = |k: u64| on((k % 16) as u8, (k * 7 % 128) as u8);
    type Scenario<'a> = (&'a str, &'a Arc<SoundFont>, &'a dyn Fn(u64) -> Event);
    let scenarios: [Scenario; 5] = [
        ("note ons of 256 regions of 64 modulators", &costly, &note),
        ("note ons through 1M region pairs, none playing", &unmatched, &|_| on(0, 0)),
        ("note ons of one region", &one, &note),
        ("note offs", &one, &|k| Event::NoteOff { channel: 0, key: (k % 128) as u8 }),
        ("GS resets among note ons", &one, &|k| if k % 64 == 0 { note(k) } else { gs_reset() }),
    ];
    for (rate, voices) in [(44100, 256), (48000, 256), (96000, 256), (192_000, 256), (48000, 4096)] {
        for (name, sf, event) in scenarios {
            let mut s = synth(sf, rate, voices);
            let (total, worst) = flood(&mut s, event);
            let what = format!("{rate} Hz, {voices} voices, {name}");
            // Real time: events take a quarter, the voices they start the rest
            // (at 192 kHz, about 0.6 s here).
            within(&format!("{what}: 1 s of audio"), total, Duration::from_secs(1));
            within(&format!("{what}: worst render of 64 frames"), worst, Duration::from_millis(15));
        }
    }
}

/// Files with crowds of events at once, or a 1 ms loop full of them, keep
/// up through `Sequencer::render`, and the host's `send` right after a
/// render stays cheap (rounds 3 and 4: 25 ms renders, 1.19 s for 1 s of
/// audio, a send of 311 ms).
#[test]
#[ignore]
fn sequencer_crowds_keep_up() {
    let one = font(short(), 1, 0, false);
    let costly = font(short(), 256, 64, false);
    let crowd = |n: u32, f: &dyn Fn(Track, u32) -> Track| {
        let t = (0..n).fold(Track::new().running_status(true), f);
        SmfBuilder::new().track(t.end(10)).build()
    };
    // A loop of 20 ticks of 52 µs, about 1 ms, with 20000 events at tick 0.
    let mut tight = Track::new().running_status(true).tempo(0, 5000);
    for i in 0..20_000u32 {
        tight = tight.cc(0, (i % 16) as u8, 1, (i % 128) as u8);
    }
    let files: [(&str, Vec<u8>, &Arc<SoundFont>, bool); 4] = [
        ("300000 CCs at tick 0", crowd(300_000, &|t, i| t.cc(0, (i % 16) as u8, 1, (i % 128) as u8)), &one, false),
        (
            "100000 note ons at tick 0",
            crowd(100_000, &|t, i| t.note_on(0, (i % 16) as u8, (i % 128) as u8, 100)),
            &one,
            false,
        ),
        (
            "20000 costly note ons at tick 0",
            crowd(20_000, &|t, i| t.note_on(0, 0, (i % 128) as u8, 100)),
            &costly,
            false,
        ),
        ("a 1 ms loop of 20000 CCs", SmfBuilder::new().division(96).track(tight.end(20)).build(), &one, true),
    ];
    for rate in [44100, 48000, 96000] {
        for (name, bytes, sf, looping) in &files {
            let smf = Arc::new(Smf::from_bytes(bytes, LoopMarkers::None).unwrap());
            let mut s = synth(sf, rate, 256);
            let mut seq = Sequencer::new(smf, &s);
            seq.set_looping(*looping);
            let (mut l, mut r) = (vec![0.0f32; 512], vec![0.0f32; 512]);
            let (mut total, mut worst, mut worst_send) = (Duration::ZERO, Duration::ZERO, Duration::ZERO);
            for k in 0..rate as usize / 512 {
                let (n, took) = timed(|| seq.render(&mut s, &mut l, &mut r));
                assert_eq!(n, 512);
                total += took;
                worst = worst.max(took);
                let ((), took) = timed(|| s.send(on(15, (k % 128) as u8)));
                worst_send = worst_send.max(took);
            }
            let what = format!("{rate} Hz, {name}");
            within(&format!("{what}: 1 s of audio"), total, Duration::from_millis(600));
            within(&format!("{what}: worst render of 512 frames"), worst, Duration::from_millis(15));
            within(&format!("{what}: worst send after a render"), worst_send, Duration::from_millis(5));
        }
    }
}

/// A host keeping most of the queue scheduled ahead does not make the
/// sequencer render frame by frame (round 5: twice the time, over real time
/// at 1024 voices).
#[test]
#[ignore]
fn a_host_scheduling_ahead_costs_the_sequencer_nothing() {
    let sf = font(held(), 1, 0, true);
    let mut t = Track::new();
    for k in 0..200u32 {
        t = t.note_on(if k == 0 { 0 } else { 10 }, 0, 60 + (k % 12) as u8, 100);
    }
    let smf = Arc::new(Smf::from_bytes(&SmfBuilder::new().track(t.end(0)).build(), LoopMarkers::None).unwrap());
    for voices in [256, 1024] {
        let play = |ahead: usize| {
            let mut s = synth(&sf, 48000, voices);
            // Voices to render, as a busy game would have.
            for k in 0..voices - 64 {
                s.send(on(1 + (k / 128 % 8) as u8, (k % 128) as u8));
            }
            // Past the budget: let them all start.
            let (mut l, mut r) = (vec![0.0f32; 4800], vec![0.0f32; 4800]);
            for _ in 0..10 {
                s.render(&mut l, &mut r);
            }
            let mut seq = Sequencer::new(Arc::clone(&smf), &s);
            for _ in 0..ahead {
                s.schedule(49_000, cc(15, 1, 1)).unwrap();
            }
            let (mut l, mut r) = (vec![0.0f32; 512], vec![0.0f32; 512]);
            let mut total = Duration::ZERO;
            for _ in 0..48000 / 512 {
                total += timed(|| seq.render(&mut s, &mut l, &mut r)).1;
            }
            total
        };
        let (alone, shared) = (play(0), play(3700));
        println!("{voices} voices: 1 s of audio {:.1} ms alone, {:.1} ms with the host ahead", ms(alone), ms(shared));
        within(
            &format!("{voices} voices: the host's share"),
            shared.saturating_sub(alone),
            alone / 2 + Duration::from_millis(20),
        );
    }
}

/// Hostile SoundFonts load, and their note ons play, in bounded time
/// (rounds 1 to 4: a big global zone 45.7 s to load; note ons of 0.1 to
/// 48 s through many regions, modulators or offsets).
#[test]
#[ignore]
fn hostile_soundfonts_load_and_play_in_time() {
    // A global zone of 65000 modulators over 16000 zones.
    let ccs: Vec<u16> = (1u16..120).filter(|c| !matches!(c, 6 | 32 | 38 | 98..=101)).collect();
    let mut many = Vec::new();
    'all: for dest in 0u16..61 {
        for &cc in &ccs {
            for bits in 0u16..16 {
                if many.len() == 65000 {
                    break 'all;
                }
                many.push(raw_modulator(raw_source(0x80 | cc | (bits << 8)), dest, 1, raw_source(0), 0));
            }
        }
    }
    let mut b = Sf2::new();
    let s = b.sample(short());
    let global = many.iter().fold(Zone::new(), |z, m| z.modulator(*m));
    let i = b.instrument("i", std::iter::once(global).chain((0..16000).map(|_| Zone::new().sample(s))));
    b.preset("p", 0, 0, [Zone::new().instrument(i)]);
    let bytes = b.build();
    let (sf, took) = timed(|| SoundFont::from_bytes(&bytes).unwrap());
    within("load: a global zone of 65000 modulators over 16000 zones", took, Duration::from_millis(500));
    drop(sf);

    // A preset zone of 60000 modulators over 256 regions.
    let mut b = Sf2::new();
    let s = b.sample(held());
    let i = b.instrument("i", (0..256).map(|_| Zone::new().set(G::SampleModes, 1).sample(s)));
    let pz = many[..60000].iter().fold(Zone::new(), |z, m| z.modulator(*m));
    b.preset("p", 0, 0, [pz.instrument(i)]);
    let preset_mods = Arc::new(b.load());

    // A million regions whose start offset is past the sample's end.
    let mut b = Sf2::new();
    let s = b.sample(SampleSpec::new("s", vec![1000; 64]));
    let zones = std::iter::once(Zone::new().set(G::StartAddrsCoarseOffset, 32767));
    let i = b.instrument("i", zones.chain((0..1000).map(|_| Zone::new().sample(s))));
    b.preset("p", 0, 0, (0..1000).map(|_| Zone::new().instrument(i)));
    let offsets = Arc::new(b.load());
    assert_eq!(offsets.regions(0, 60, 100).count(), 0);

    // A million regions of one exclusive class.
    let mut b = Sf2::new();
    let s = b.sample(held());
    let zone = Zone::new().set(G::SampleModes, 1).set(G::ExclusiveClass, 1);
    let i = b.instrument("i", (0..1024).map(|_| zone.clone().sample(s)));
    b.preset("p", 0, 0, (0..1023).map(|_| Zone::new().instrument(i)));
    let exclusive = Arc::new(b.load());

    let unmatched = unmatched_pairs();
    let cases: [(&str, &Arc<SoundFont>, u8); 4] = [
        ("60000 preset modulators over 256 regions", &preset_mods, 60),
        ("1M regions with nothing to play", &offsets, 60),
        ("1M region pairs, none playing the key", &unmatched, 0),
        ("1M regions of one exclusive class", &exclusive, 60),
    ];
    for voices in [256, 4096] {
        for (name, sf, key) in cases {
            let mut s = synth(sf, 44100, voices);
            let (mut l, mut r) = (vec![0.0f32; 64], vec![0.0f32; 64]);
            let mut worst = Duration::ZERO;
            for k in 0..4 {
                let ((), took) = timed(|| {
                    s.send(on(0, key + k));
                    s.render(&mut l, &mut r);
                });
                worst = worst.max(took);
            }
            within(&format!("{voices} voices, {name}: a note on and 64 frames"), worst, Duration::from_millis(40));
        }
    }
}

/// With events dense enough to keep the budget spent, the output does not
/// depend on the render lengths (round 5: 79 of 117 runs differed). A larger
/// run of `synth::tests::e01_dense_events_under_the_budget_...`.
#[test]
#[ignore]
fn render_lengths_do_not_matter_under_the_budget() {
    let sf = font(held(), 32, 0, true);
    let play = |rate: u32, gap: u64, seed: u64, sizes: &mut dyn FnMut() -> usize| {
        let mut c = Config::new(rate);
        c.effects = false;
        c.event_capacity = 4096;
        let mut s = Synth::new(Arc::clone(&sf), c).unwrap();
        let mut rng = Rng(seed);
        let mut frame = 0;
        for _ in 0..3000 {
            frame += rng.below(gap);
            let e = match rng.below(10) {
                0..=2 => on(0, rng.below(128) as u8),
                3 | 4 => Event::NoteOff { channel: 0, key: rng.below(128) as u8 },
                5..=8 => cc(0, 1, rng.below(128) as u8),
                _ => Event::PitchBend { channel: 0, value: rng.below(16384) as u16 },
            };
            s.schedule(frame, e).unwrap();
        }
        let (mut l, mut r) = (vec![0.0f32; 60000], vec![0.0f32; 60000]);
        let mut done = 0;
        while done < l.len() {
            let n = sizes().min(l.len() - done);
            s.render(&mut l[done..done + n], &mut r[done..done + n]);
            done += n;
        }
        l
    };
    let mut runs = 0;
    for rate in [44100, 48000, 96000] {
        for gap in [10, 20, 40, 100] {
            for seed in 1..10 {
                let blocks = play(rate, gap, seed, &mut || 64);
                let mut rng = Rng(seed * 7919 + 1);
                let random = play(rate, gap, seed, &mut || 1 + rng.below(700) as usize);
                assert!(blocks == random, "{rate} Hz, a gap of {gap} at most, seed {seed}");
                runs += 1;
            }
        }
    }
    println!("{runs} runs alike");
}

/// Random files through `Sequencer::render` with a tiny queue, random render
/// lengths, the host's events, seeks, loops switched, sample rates changed
/// and resets: no panic (the chase's debug assertions included in a debug
/// build), every frame asked for, nothing not finite, no long call.
#[test]
#[ignore]
fn random_sequencing_stress() {
    let sf = font(held(), 4, 0, true);
    let rates = [8000u32, 44100, 48000, 192_000];
    let mut worst = Duration::ZERO;
    for seed in 1..400u64 {
        let mut r = Rng(seed * 2_654_435_761 + 7);
        let mut t = Track::new();
        if r.below(2) == 0 {
            t = t.sysex(0, &[0x41, 0x10, 0x42, 0x12, 0x40, 0x00, 0x7F, 0x00, 0x41]);
        }
        for _ in 0..r.below(400) {
            let d = if r.below(3) == 0 { 0 } else { r.below(50) };
            let ch = r.below(16) as u8;
            let controllers = [0, 32, 6, 38, 98, 99, 100, 101, 121, 64, 66, 7, 111, 116, 117];
            t = match r.below(9) {
                0 | 1 => t.note_on(d, ch, r.below(128) as u8, r.below(128) as u8),
                2 => t.note_off(d, ch, r.below(128) as u8),
                3 => t.cc(d, ch, controllers[r.below(15) as usize], r.below(128) as u8),
                4 => t.program(d, ch, r.below(128) as u8),
                5 => t.pitch_bend(d, ch, r.below(16384) as u16),
                6 => t.sysex(d, &[0x7E, 0x7F, 0x09, [1u8, 3][r.below(2) as usize]]),
                7 => t.tempo(d, 1 + r.below(2_000_000) as u32),
                _ => t.sysex(d, &[0x41, 0x10, 0x42, 0x12, 0x40, 0x10 | r.below(16) as u8, 0x15, 1, 0x1A]),
            };
        }
        let markers = [LoopMarkers::None, LoopMarkers::RpgMaker, LoopMarkers::FinalFantasy][r.below(3) as usize];
        let bytes = SmfBuilder::new().division(1 + r.below(960) as u16).track(t.end(r.below(100))).build();
        let Ok(smf) = Smf::from_bytes(&bytes, markers) else { continue };
        let smf = Arc::new(smf);
        let mut c = Config::new(rates[r.below(4) as usize]);
        c.event_capacity = 16 + r.below(64) as usize;
        c.max_voices = 1 + r.below(64) as usize;
        c.effects = r.below(2) == 0;
        let mut s = Synth::new(Arc::clone(&sf), c.clone()).unwrap();
        let mut seq = Sequencer::new(Arc::clone(&smf), &s);
        seq.set_looping(r.below(3) != 0);
        let (mut l, mut rr) = (vec![0.0f32; 4096], vec![0.0f32; 4096]);
        for call in 0..300 {
            let n = 1 + r.below(1500) as usize;
            match r.below(20) {
                0 => seq.seek(Duration::from_millis(r.below(20000))),
                1 => seq.set_looping(r.below(2) == 0),
                2 => {
                    for _ in 0..r.below(40) {
                        let _ = s.schedule(s.now() + r.below(5000), on(r.below(16) as u8, r.below(128) as u8));
                    }
                }
                3 => {
                    for _ in 0..r.below(40) {
                        s.send(cc(r.below(16) as u8, r.below(128) as u8, r.below(128) as u8));
                    }
                }
                4 => {
                    let alone = r.below(3000).min(4096) as usize;
                    s.render(&mut l[..alone], &mut rr[..alone]);
                }
                5 if call % 7 == 0 => {
                    let mut other = c.clone();
                    other.sample_rate = rates[r.below(4) as usize];
                    s = Synth::new(Arc::clone(&sf), other).unwrap();
                }
                6 => s.send(gs_reset()),
                _ => {}
            }
            let (got, took) = timed(|| seq.render(&mut s, &mut l[..n], &mut rr[..n]));
            assert_eq!(got, n, "seed {seed}, call {call}");
            assert!(l[..n].iter().chain(&rr[..n]).all(|x| x.is_finite()), "seed {seed}, call {call}");
            worst = worst.max(took);
        }
    }
    within("random sequencing: worst call", worst, Duration::from_millis(50));
}

/// What the work budget's units stand for here: the time of each kind of
/// work, to set against the units `synth/budget.rs` charges (about a
/// nanosecond each on the development machine). Prints only.
#[test]
#[ignore]
fn cost_report() {
    // Each measured once the budget is full, few enough to stay within it.
    let refill = |s: &mut Synth| {
        let (mut l, mut r) = (vec![0.0f32; 4096], vec![0.0f32; 4096]);
        for _ in 0..s.config().sample_rate / 2048 {
            s.render(&mut l, &mut r);
        }
    };
    let per = |s: &mut Synth, n: u64, event: &dyn Fn(u64) -> Event| {
        refill(s);
        let ((), took) = timed(|| (0..n).for_each(|k| s.send(event(k))));
        took.as_secs_f64() * 1e9 / n as f64
    };
    let one = font(short(), 1, 0, false);
    for voices in [16, 256, 4096] {
        let mut s = synth(&one, 44100, voices);
        let off = per(&mut s, 50, &|k| Event::NoteOff { channel: 0, key: (k % 128) as u8 });
        let control = per(&mut s, 50, &|k| cc(1, 1, (k % 128) as u8));
        let note = per(&mut s, 50, &|k| on((k % 16) as u8, (k / 16 % 128) as u8));
        println!(
            "{voices} voices ({} slots): note off {off:.0} ns, CC {control:.0} ns, one-region note on {note:.0} ns",
            2 * voices
        );
    }
    let unmatched = unmatched_pairs();
    let mut s = synth(&unmatched, 44100, 256);
    let pairs = per(&mut s, 4, &|_| on(0, 0));
    println!("going through region pairs: {:.2} ns a pair", pairs / 1_001_000.0);
    for mods in [0, 16, 64] {
        let sf = font(held(), 128, mods, true);
        let mut s = synth(&sf, 44100, 256);
        let start = per(&mut s, 1, &|_| on(0, 60));
        println!("{mods} modulators: starting a voice {:.0} ns", start / 128.0);
    }
}
