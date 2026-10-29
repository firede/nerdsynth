//! Tests of the synthesizer: the engine behaviours of `spec/behavior.md`
//! (E-01 to E-16) and P-SF-11's clamping when playing.
//!
//! Most use a constant ("DC") looped sample, so that the output is exactly
//! the gain of the voice: a point of 16384 is 0.5, times FluidSynth's
//! filter gain at a Q of 0 (1/sqrt(Q), Q = 10^(-3.01/20)), times the pan.

use std::sync::Arc;

use super::dsp::conv::{cb2amp, pan};
use super::dsp::envelope::Stage;
use super::dsp::filter::q_linear;
use super::*;
use crate::event::SysEx;
use crate::soundfont::GeneratorType as G;
use crate::testkit::sf2::{SampleHeader, SampleSpec, Sf2, Zone, modulator, sine, source};

const SR: u32 = 44100;

fn load(b: &Sf2) -> Arc<SoundFont> {
    Arc::new(b.load())
}

/// A constant looped sample of 16384 (0.5).
fn dc() -> SampleSpec {
    SampleSpec::new("dc", vec![16384; 2000]).looped(100, 1900)
}

fn single(sample: SampleSpec, zone: Zone) -> Arc<SoundFont> {
    load(&Sf2::single(sample, zone.set(G::SampleModes, 1)))
}

fn config() -> Config {
    let mut c = Config::new(SR);
    c.effects = false;
    c
}

fn synth(sf: Arc<SoundFont>) -> Synth {
    Synth::new(sf, config()).unwrap()
}

/// The gain of a centered voice at full volume (CC7 127) on a constant 0.5.
fn full() -> f64 {
    0.5 / q_linear(0.0).sqrt() * pan(0.0, true)
}

fn render(s: &mut Synth, frames: usize) -> (Vec<f32>, Vec<f32>) {
    let mut l = vec![0.0; frames];
    let mut r = vec![0.0; frames];
    assert_eq!(s.render(&mut l, &mut r), frames);
    (l, r)
}

fn cc(channel: u8, controller: u8, value: u8) -> Event {
    Event::ControlChange { channel, controller, value }
}

fn on(channel: u8, key: u8, velocity: u8) -> Event {
    Event::NoteOn { channel, key, velocity }
}

fn off(channel: u8, key: u8) -> Event {
    Event::NoteOff { channel, key }
}

/// The frequency of a signal by its upward zero crossings, interpolated.
fn frequency(x: &[f32]) -> f64 {
    let mut first = None;
    let mut last = 0.0;
    let mut count = 0;
    for i in 1..x.len() {
        if x[i - 1] < 0.0 && x[i] >= 0.0 {
            let t = (i - 1) as f64 + f64::from(x[i - 1] / (x[i - 1] - x[i]));
            if first.is_none() {
                first = Some(t);
            } else {
                count += 1;
            }
            last = t;
        }
    }
    f64::from(count) / (last - first.unwrap()) * f64::from(SR)
}

fn rms(x: &[f32]) -> f64 {
    (x.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
}

#[test]
fn config_ranges_are_checked() {
    use crate::error::Setting;
    let sf = single(dc(), Zone::new());
    let fails = |c: Config| match Synth::new(sf.clone(), c) {
        Err(Error::Config(setting)) => Some(setting),
        _ => None,
    };
    for (rate, voices, events, setting) in [
        (7999, 256, 4096, Setting::SampleRate),
        (192_001, 256, 4096, Setting::SampleRate),
        (SR, 0, 4096, Setting::MaxVoices),
        (SR, 4097, 4096, Setting::MaxVoices),
        (SR, 1, 15, Setting::EventCapacity),
    ] {
        let mut c = Config::new(rate);
        c.max_voices = voices;
        c.event_capacity = events;
        assert_eq!(fails(c), Some(setting), "{rate} {voices} {events}");
    }
    for gain in [-0.5, f32::NAN, f32::INFINITY, 1000.5] {
        let mut c = Config::new(SR);
        c.gain = gain;
        assert_eq!(fails(c), Some(Setting::Gain), "gain {gain}");
    }
    let mut c = Config::new(SR);
    c.min_note_length = std::time::Duration::from_millis(1001);
    assert_eq!(fails(c), Some(Setting::MinNoteLength));
    let mut c = Config::new(8000);
    c.max_voices = 1;
    c.event_capacity = 16;
    c.gain = 0.0;
    c.min_note_length = std::time::Duration::from_secs(1);
    assert!(Synth::new(sf, c).is_ok());
}

#[test]
fn the_queue_is_bounded() {
    let mut c = config();
    c.event_capacity = 16;
    let mut s = Synth::new(single(dc(), Zone::new()), c).unwrap();
    for i in 0..16 {
        assert_eq!(s.schedule(i, cc(0, 1, 0)), Ok(()));
    }
    assert_eq!(s.schedule(0, cc(0, 1, 0)), Err(QueueFull));
    render(&mut s, 20);
    assert_eq!(s.schedule(0, cc(0, 1, 0)), Ok(()));
}

/// E-01: a note on scheduled at a frame starts sounding there and not
/// before; a note off releases from its frame; frames within a block count.
#[test]
fn e01_events_take_effect_at_their_frame() {
    let sf = single(dc(), Zone::new().set(G::ReleaseVolEnv, -12000));
    for frame in [0u64, 1, 63, 64, 1000, 1001, 4097] {
        let mut s = synth(sf.clone());
        s.schedule(frame, on(0, 60, 127)).unwrap();
        s.schedule(frame + 3000, off(0, 60)).unwrap();
        let (l, _) = render(&mut s, 8000);
        let f = frame as usize;
        assert!(l[..=f].iter().all(|x| *x == 0.0), "sound before frame {frame}");
        // The gain ramps to its value at the end of the segment, which the
        // 1 ms delay and attack reach within a block.
        assert!(l[f + 1..f + 64 + 44].iter().any(|x| *x > 0.0), "silence after frame {frame}");
        let level = l[f + 2999];
        assert!(level > 0.1);
        assert!((l[f + 2999] - l[f + 2990]).abs() < 1e-6);
        assert!(l[f + 3010] < level, "not released at frame {}", frame + 3000);
    }
}

/// E-01: how `render` is called does not change the output.
#[test]
fn e01_the_output_does_not_depend_on_the_render_lengths() {
    let sf = single(SampleSpec::new("sine", sine(4000, 37.3, 20000)).looped(10, 3739), Zone::new());
    let play = |chunks: &[usize]| {
        let mut s = synth(sf.clone());
        for (frame, e) in [(5u64, on(0, 60, 100)), (700, cc(0, 1, 127)), (701, cc(0, 10, 0)), (3000, off(0, 60))] {
            s.schedule(frame, e).unwrap();
        }
        let mut out = vec![];
        let mut k = 0;
        while out.len() < 6000 {
            let n = chunks[k % chunks.len()].min(6000 - out.len());
            out.extend(render(&mut s, n).0);
            k += 1;
        }
        out
    };
    let one = play(&[6000]);
    for chunks in [&[1usize][..], &[7], &[64], &[1000], &[13, 1, 200, 64]] {
        assert_eq!(play(chunks), one, "chunks {chunks:?}");
    }
}

/// E-01: an event sent between render calls takes effect at the next frame
/// rendered, and events of past frames at the start of the next render.
#[test]
fn e01_sent_and_late_events() {
    let sf = single(dc(), Zone::new());
    let mut s = synth(sf.clone());
    render(&mut s, 100);
    s.send(on(0, 60, 127));
    let (l, _) = render(&mut s, 200);
    assert_eq!(l[0], 0.0);
    let first = l.iter().position(|x| *x > 0.0).unwrap();
    let mut s = synth(sf);
    render(&mut s, 100);
    s.schedule(10, on(0, 60, 127)).unwrap();
    let (l, _) = render(&mut s, 200);
    assert_eq!(l.iter().position(|x| *x > 0.0), Some(first));
}

/// The volume envelope (FluidSynth's semantics): linear attack in
/// amplitude, decay and release linear in dB over 96 dB, sustain in cB
/// (×0.96 of the 100 dB scale), at exact frames.
#[test]
fn volume_envelope_levels_at_frames() {
    // Attack 1 s, hold 0.5 s, decay 2 s, sustain 480 cB, release 1 s.
    let zone = Zone::new()
        .set(G::AttackVolEnv, 0)
        .set(G::HoldVolEnv, -1200)
        .set(G::DecayVolEnv, 1200)
        .set(G::SustainVolEnv, 480)
        .set(G::ReleaseVolEnv, 0);
    let mut s = synth(single(dc(), zone));
    s.send(cc(0, 7, 127));
    s.send(on(0, 60, 127));
    let (l, _) = render(&mut s, 6 * SR as usize);
    let full = full();
    let at = |f: usize| f64::from(l[f]);
    // FluidSynth's stage lengths, in blocks of 64 frames: an attack of
    // 1 + 689 blocks, a hold of 345.
    let (attack, hold) = (690 * 64, 345 * 64);
    for f in [64usize * 100, 64 * 300, 64 * 600] {
        assert!((at(f) - full * f as f64 / attack as f64).abs() < 1e-4, "attack at {f}: {}", at(f));
    }
    assert!((at(attack + 64 * 300) - full).abs() < 1e-4);
    let decay_start = attack + hold;
    let f = decay_start + 64 * 100;
    let expected = full * cb2amp(960.0 * (64.0 * 100.0) / (1378.0 * 64.0));
    assert!((at(f) / expected - 1.0).abs() < 1e-3, "decay {} {expected}", at(f));
    let sustain = full * cb2amp(0.96 * 480.0);
    assert!((at(5 * 44100) / sustain - 1.0).abs() < 1e-4, "sustain {} {sustain}", at(5 * 44100));
    s.send(off(0, 60));
    let (l, _) = render(&mut s, 44100);
    let f = 64 * 100;
    let expected = sustain * cb2amp(960.0 * f as f64 / (690.0 * 64.0));
    assert!((f64::from(l[f]) / expected - 1.0).abs() < 1e-3, "release {} {expected}", l[f]);
    render(&mut s, 44100);
    assert_eq!(s.active_voices(), 0);
}

/// E-02: scaleTuning scales the distance from the root key only; pitch
/// bend and tuning are not scaled.
#[test]
fn e02_scale_tuning_scales_the_key_only() {
    let sample = || SampleSpec::new("sine", sine(44100, 100.0, 20000)).looped(0, 44100);
    let measure = |zone: Zone, key: u8, bend: Option<u16>| {
        let mut s = synth(single(sample(), zone));
        if let Some(value) = bend {
            s.send(Event::PitchBend { channel: 0, value });
        }
        s.send(on(0, key, 127));
        let (l, _) = render(&mut s, 22050);
        frequency(&l[2000..])
    };
    let root = measure(Zone::new(), 60, None);
    assert!((root - 441.0).abs() < 0.05, "{root}");
    let half = measure(Zone::new().set(G::ScaleTuning, 50), 72, None);
    assert!((half / 441.0 - 2f64.powf(0.5)).abs() < 1e-4, "{half}");
    // Scale 0: every key plays the root; the bend still bends fully, by
    // FluidSynth's 12700 × 16383/16384 × 2/128 cents.
    let bent = measure(Zone::new().set(G::ScaleTuning, 0), 84, Some(16383));
    let cents = 12700.0 * 16383.0 / 16384.0 * 2.0 / 128.0;
    assert!((bent / 441.0 - 2f64.powf(cents / 1200.0)).abs() < 1e-4, "{bent}");
    // The modulation envelope's pitch at its sustain (level 1) is not scaled.
    let env = measure(Zone::new().set(G::ScaleTuning, 0).set(G::ModEnvToPitch, 700), 30, None);
    assert!((env / 441.0 - 2f64.powf(700.0 / 1200.0)).abs() < 1e-4, "{env}");
    let tuned = measure(Zone::new().set(G::ScaleTuning, 0).set(G::CoarseTune, 12).set(G::FineTune, -50), 30, None);
    assert!((tuned / 441.0 - 2f64.powf(1150.0 / 1200.0)).abs() < 1e-4, "{tuned}");
}

/// The pitch correction and the overriding root key.
#[test]
fn root_key_and_correction() {
    let sample = SampleSpec::new("sine", sine(44100, 100.0, 20000)).looped(0, 44100).key(69).correction(-20);
    let mut s = synth(single(sample, Zone::new().set(G::OverridingRootKey, 57)));
    s.send(on(0, 69, 127));
    let (l, _) = render(&mut s, 22050);
    let f = frequency(&l[2000..]);
    assert!((f / 441.0 - 2f64.powf(1180.0 / 1200.0)).abs() < 1e-4, "{f}");
}

/// E-03: generators with modulators are clamped to their ranges: an
/// attenuation driven below 0 stays at 0.
#[test]
fn e03_modulated_generators_are_clamped() {
    let louder = modulator(source(1, true, false, false, 0), G::InitialAttenuation, -960, Default::default());
    let level = |m: Option<crate::soundfont::Modulator>, att: i16| {
        let mut z = Zone::new().set(G::InitialAttenuation, att);
        if let Some(m) = m {
            z = z.modulator(m);
        }
        let mut s = synth(single(dc(), z));
        s.send(cc(0, 7, 127));
        s.send(cc(0, 1, 127));
        s.send(on(0, 60, 127));
        f64::from(render(&mut s, 4000).0[3999])
    };
    let plain = level(None, 0);
    assert!((plain - full()).abs() < 1e-5, "{plain}");
    assert_eq!(level(Some(louder), 0), plain);
    assert_eq!(level(Some(louder), 100), plain);
    assert!((level(None, 100) / plain - cb2amp(40.0)).abs() < 1e-5);
    // The clamp comes after the modulators, not before: a preset's -100 cB
    // (×0.4) and 100 cB of CC1 at 127 are 59.2 cB, not 99.2.
    let cc1 = modulator(source(1, true, false, false, 0), G::InitialAttenuation, 100, Default::default());
    let mut b = Sf2::new();
    let s0 = b.sample(dc());
    let i = b.instrument("i", [Zone::new().set(G::SampleModes, 1).modulator(cc1).sample(s0)]);
    b.preset("p", 0, 0, [Zone::new().set(G::InitialAttenuation, -100).instrument(i)]);
    let mut s = synth(load(&b));
    s.send(cc(0, 7, 127));
    s.send(cc(0, 1, 127));
    s.send(on(0, 60, 127));
    let got = f64::from(render(&mut s, 4000).0[3999]);
    let expected = cb2amp(-40.0 + 100.0 * 127.0 / 128.0);
    assert!((got / plain - expected).abs() < 1e-5, "{} {expected}", got / plain);
}

/// E-04: the default modulators apply; a zone's identical modulator
/// replaces the default one; a preset's adds to the instrument's.
#[test]
fn e04_modulators_merge() {
    let level = |b: Sf2, events: &[Event]| {
        let mut s = synth(load(&b));
        for e in events {
            s.send(*e);
        }
        s.send(on(0, 60, 127));
        f64::from(render(&mut s, 4000).0[3999])
    };
    let one = |zone: Zone| Sf2::single(dc(), zone.set(G::SampleModes, 1));
    let full = full();
    // Default CC7 (not scaled by 0.4, as FluidSynth): 40 log10(64/127) dB.
    let half = level(one(Zone::new()), &[cc(0, 7, 64)]);
    let expected = (64.0f64 / 127.0).powi(2);
    assert!((half / full - expected).abs() < 1e-4, "{half} {}", full * expected);
    // An identical modulator of amount 0 turns the default off.
    let cc7 = modulator(source(7, true, true, false, 1), G::InitialAttenuation, 0, Default::default());
    let none = level(one(Zone::new().modulator(cc7)), &[cc(0, 7, 0)]);
    assert!((none - full).abs() < 1e-5, "{none}");
    // A preset modulator adds to the instrument's: 200 + 200 cB of CC1.
    let cc1 = modulator(source(1, true, false, false, 0), G::InitialAttenuation, 200, Default::default());
    let mut b = Sf2::new();
    let s = b.sample(dc());
    let i = b.instrument("i", [Zone::new().set(G::SampleModes, 1).modulator(cc1).sample(s)]);
    b.preset("p", 0, 0, [Zone::new().modulator(cc1).instrument(i)]);
    let both = level(b, &[cc(0, 7, 127), cc(0, 1, 127)]);
    let expected = cb2amp(400.0 * 127.0 / 128.0);
    assert!((both / full - expected).abs() < 1e-4, "{both} {}", full * expected);
}

/// Modulators are evaluated again when the channel changes, at the event's
/// frame, ramping over the block that follows.
#[test]
fn controllers_change_playing_voices() {
    let mut s = synth(single(dc(), Zone::new()));
    s.send(cc(0, 7, 127));
    s.send(on(0, 60, 127));
    s.schedule(6400, cc(0, 7, 0)).unwrap();
    let (l, _) = render(&mut s, 7000);
    assert!((f64::from(l[6399]) - full()).abs() < 1e-5);
    assert!(l[6400 + 64] < 1e-4, "CC7 0 is {}", l[6464]);
}

/// The pan law: FluidSynth's sine over 1002 points; CC10 0 is fully left.
#[test]
fn pan_is_constant_power() {
    let sf = single(dc(), Zone::new());
    for (value, tenths) in [(0u8, -500.0), (64, 0.0), (127, 500.0 * 127.0 / 128.0)] {
        let mut s = synth(sf.clone());
        s.send(cc(0, 7, 127));
        s.send(cc(0, 10, value));
        s.send(on(0, 60, 127));
        let (l, r) = render(&mut s, 4000);
        let base = 0.5 / q_linear(0.0).sqrt();
        assert!((f64::from(l[3999]) - base * pan(tenths, true)).abs() < 1e-5, "{value}");
        assert!((f64::from(r[3999]) - base * pan(tenths, false)).abs() < 1e-5, "{value}");
    }
}

/// An instrument of keys 0–63 (left) and 64–127 (right) and a second region
/// of key 70 (right), the low zone of class 1, the others of `class_high`.
fn two_class_sf(class_high: i16) -> Arc<SoundFont> {
    let mut b = Sf2::new();
    let s = b.sample(dc());
    let z = || Zone::new().set(G::SampleModes, 1);
    let i = b.instrument(
        "hats",
        [
            z().keys(0, 63).set(G::ExclusiveClass, 1).set(G::Pan, -500).sample(s),
            z().keys(64, 127).set(G::ExclusiveClass, class_high).set(G::Pan, 500).sample(s),
            z().keys(70, 70).set(G::ExclusiveClass, class_high).set(G::Pan, 500).sample(s),
        ],
    );
    b.preset("p", 0, 0, [Zone::new().instrument(i)]);
    b.preset("q", 0, 1, [Zone::new().instrument(i)]);
    load(&b)
}

/// E-05: a note of an exclusive class stops the sounding notes of the
/// class on its channel and preset with a release of -2000 timecents, 218
/// blocks of 64 frames at 44.1 kHz, linear in decibels (FluidSynth's
/// `fluid_voice_kill_excl`); the regions of one note sound together.
#[test]
fn e05_exclusive_classes() {
    let mut s = synth(two_class_sf(1));
    s.send(cc(0, 7, 127));
    s.send(on(0, 30, 127));
    render(&mut s, 640);
    s.send(on(0, 70, 127));
    assert_eq!(s.active_voices(), 3);
    let (l, r) = render(&mut s, 14000);
    let base = 0.5 / q_linear(0.0).sqrt();
    let release = 218.0 * 64.0;
    for k in [0usize, 64, 3488, 6976, 10464] {
        let want = base * cb2amp(960.0 * (k as f64 + 1.0) / release);
        assert!((f64::from(l[k]) / want - 1.0).abs() < 0.02, "frame {k}: {} for {want}", l[k]);
    }
    assert!(l.windows(2).all(|w| w[1] <= w[0] + 1e-7 && w[0] - w[1] < 1e-3), "not a smooth release");
    assert!(l[13952..].iter().all(|x| x.abs() < 1e-6), "still sounding after the release");
    assert!((f64::from(r[13000]) - 2.0 * base).abs() < 1e-3, "{}", r[13000]);
    assert_eq!(s.active_voices(), 2);
}

/// E-05: the pedals do not hold a note stopped by an exclusive class, and a
/// note already releasing is stopped too (FluidSynth's voices stay
/// playing in their release).
#[test]
fn e05_exclusive_classes_stop_held_and_released_notes() {
    let sf = single(dc(), Zone::new().set(G::ExclusiveClass, 1).set(G::ReleaseVolEnv, 0));
    let sounding = |s: &Synth, key: u8| s.voices.iter().any(|v| !v.is_free() && v.key == key);

    let mut s = synth(sf.clone());
    s.send(cc(0, 64, 127));
    s.send(on(0, 60, 127));
    render(&mut s, 640);
    s.send(off(0, 60));
    s.send(on(0, 61, 127));
    render(&mut s, 14000);
    assert!(!sounding(&s, 60), "held by the hold pedal");

    let mut s = synth(sf);
    s.send(on(0, 60, 127));
    render(&mut s, 640);
    s.send(off(0, 60));
    render(&mut s, 64);
    assert!(sounding(&s, 60));
    s.send(on(0, 61, 127));
    render(&mut s, 14000);
    assert!(!sounding(&s, 60), "a released note keeps its own release");
}

/// E-05: other channels, other presets, and class 0 are not stopped.
#[test]
fn e05_exclusive_classes_stay_in_their_channel_and_preset() {
    let sf = two_class_sf(1);
    let mut s = synth(sf.clone());
    s.send(on(0, 30, 127));
    s.send(on(1, 70, 127));
    render(&mut s, 640);
    assert_eq!(s.active_voices(), 3, "another channel");
    let mut s = synth(sf);
    s.send(on(0, 30, 127));
    s.send(Event::ProgramChange { channel: 0, program: 1 });
    s.send(on(0, 70, 127));
    render(&mut s, 640);
    assert_eq!(s.active_voices(), 3, "another preset");
    let mut s = synth(two_class_sf(0));
    s.send(on(0, 30, 127));
    s.send(on(0, 70, 127));
    render(&mut s, 640);
    assert_eq!(s.active_voices(), 3, "class 0");
}

/// E-06: a voice taken for a new note fades out over 2 ms instead of
/// stopping, and the new note starts at once.
#[test]
fn e06_stolen_voices_fade_out() {
    let mut b = Sf2::new();
    let s0 = b.sample(dc());
    let z = || Zone::new().set(G::SampleModes, 1);
    let i = b.instrument(
        "lr",
        [z().keys(0, 63).set(G::Pan, -500).sample(s0), z().keys(64, 127).set(G::Pan, 500).sample(s0)],
    );
    b.preset("p", 0, 0, [Zone::new().instrument(i)]);
    let mut c = config();
    c.max_voices = 1;
    let mut s = Synth::new(load(&b), c).unwrap();
    s.send(cc(0, 7, 127));
    s.send(on(0, 30, 127));
    render(&mut s, 6400);
    s.send(on(0, 90, 127));
    assert_eq!(s.active_voices(), 2);
    let (l, r) = render(&mut s, 640);
    let base = (0.5 / q_linear(0.0).sqrt()) as f32;
    assert!((l[0] - base).abs() < 1e-3);
    assert!(l[89..].iter().all(|x| x.abs() < 1e-6));
    assert!(l.windows(2).all(|w| (w[0] - w[1]).abs() < 0.02), "a step in the fade");
    assert!(r[1] > 0.0, "the new note waits");
    assert_eq!(s.active_voices(), 1);
}

/// Stealing takes the voice of the lowest priority: a released one before
/// a held one.
#[test]
fn stealing_prefers_released_voices() {
    let sf = single(dc(), Zone::new().set(G::ReleaseVolEnv, 8000));
    let mut c = config();
    c.max_voices = 2;
    let mut s = Synth::new(sf, c).unwrap();
    s.send(on(0, 60, 127));
    render(&mut s, 1000);
    s.send(on(0, 61, 127));
    render(&mut s, 1000);
    s.send(off(0, 60));
    render(&mut s, 64);
    s.send(on(0, 62, 127));
    let fading: Vec<u8> = s.voices.iter().filter(|v| !v.is_free() && v.fading).map(|v| v.key).collect();
    assert_eq!(fading, [60]);
}

/// E-07: keys and velocities above 127 are 127, for note offs as for note
/// ons; channels beyond 15 are ignored.
#[test]
fn e07_note_off_finds_the_note_on_out_of_range() {
    let mut s = synth(single(dc(), Zone::new().set(G::ReleaseVolEnv, -12000)));
    s.send(on(0, 200, 255));
    render(&mut s, 640);
    assert_eq!(s.active_voices(), 1);
    assert_eq!(s.voices.iter().find(|v| !v.is_free()).map(|v| v.key), Some(127));
    s.send(off(0, 200));
    render(&mut s, 4410);
    assert_eq!(s.active_voices(), 0);
    s.send(on(16, 60, 100));
    assert_eq!(s.active_voices(), 0);
}

/// E-08: a note held for 50 hours: the clock and the voice's age are 64-bit,
/// events far past 2^32 frames land on their frame, and it still sounds.
#[test]
fn e08_fifty_hours() {
    let zone = Zone::new().set(G::VibLfoToPitch, 50).set(G::ModLfoToVolume, 20);
    let sample = SampleSpec::new("sine", sine(44100, 100.0, 20000)).looped(0, 44100);
    let mut s = synth(single(sample, zone));
    s.send(on(0, 60, 127));
    render(&mut s, 44100);
    let hours = 50 * 3600 * u64::from(SR);
    s.now += hours;
    for v in s.voices.iter_mut().filter(|v| !v.is_free()) {
        v.age_by(hours);
    }
    let at = s.now + 1000;
    s.schedule(at, cc(0, 7, 0)).unwrap();
    let (l, _) = render(&mut s, 2 * 44100);
    assert!(l.iter().all(|x| x.is_finite()));
    let f = frequency(&l[..1000]);
    assert!((f - 441.0).abs() < 15.0, "{f}");
    assert!(rms(&l[1100..]) < 1e-3, "CC7 0 at frame {at}");
    assert!(s.now() > 1 << 32);
    assert_eq!(s.active_voices(), 1);
}

/// E-09: a quiet note still sends to the reverb and the chorus by its
/// send: the reverb's tail follows it, and the chorus changes the sound.
#[test]
fn e09_quiet_notes_reach_the_effects() {
    let sf = single(dc(), Zone::new().set(G::ReleaseVolEnv, -7200));
    let play = |reverb: u8, chorus: u8| {
        let mut s = Synth::new(sf.clone(), Config::new(SR)).unwrap();
        s.send(cc(0, 91, reverb));
        s.send(cc(0, 93, chorus));
        s.send(on(0, 60, 1));
        let (during, _) = render(&mut s, 4410);
        s.send(off(0, 60));
        render(&mut s, 4410);
        assert_eq!(s.active_voices(), 0);
        let (l, r) = render(&mut s, 4410);
        (during, rms(&l) + rms(&r), s.is_silent())
    };
    let (dry, tail, silent) = play(0, 0);
    assert!(tail == 0.0 && silent, "{tail} {silent}");
    let (_, tail, silent) = play(127, 0);
    assert!(tail > 1e-6 && !silent, "reverb tail {tail}");
    let (wet, _, _) = play(0, 127);
    let diff: Vec<f32> = wet.iter().zip(&dry).map(|(a, b)| a - b).collect();
    assert!(rms(&diff[1000..]) > 1e-6, "chorus {}", rms(&diff[1000..]));
}

/// With `effects` off, the effects do not run: the sends make no sound.
#[test]
fn effects_off_means_no_effects() {
    let mut s = synth(single(dc(), Zone::new().set(G::ReleaseVolEnv, -7200)));
    s.send(cc(0, 91, 127));
    s.send(on(0, 60, 127));
    render(&mut s, 4410);
    s.send(off(0, 60));
    render(&mut s, 4410);
    let (l, r) = render(&mut s, 4410);
    assert!(l.iter().chain(&r).all(|x| *x == 0.0));
    assert!(s.is_silent());
}

/// E-10: a loop shorter than the step reads only the loop.
#[test]
fn e10_short_loops_stay_in_the_loop() {
    let mut points = vec![-30000i16; 100];
    points[40] = 16384;
    points[41] = 16384;
    let sample = SampleSpec::new("short", points).looped(40, 42).key(0);
    // Key 127 over root 0: 2^(127/12) ≈ 1500 points a frame.
    let zone = Zone::new().set(G::StartAddrsOffset, 40);
    let mut s = synth(single(sample, zone));
    s.send(cc(0, 7, 127));
    s.send(on(0, 127, 127));
    let (l, _) = render(&mut s, 4410);
    assert!(l[8..].iter().all(|x| *x >= 0.0), "read outside the loop");
    assert!((f64::from(l[4000]) - full()).abs() < 1e-3, "{}", l[4000]);
}

/// P-SF-11: sample headers outside the data and loops outside the sample
/// play clamped, without panicking.
#[test]
fn p_sf_11_headers_outside_the_data_are_clamped() {
    let header = |start, end, loop_start, loop_end| SampleHeader {
        name: "bad".into(),
        start,
        end,
        loop_start,
        loop_end,
        sample_rate: 44100,
        original_pitch: 60,
        pitch_correction: 0,
        link: 0,
        kind: 1,
    };
    for (start, end, ls, le, sounds) in [
        (0, 1_000_000, 10, 900_000, true),
        (50, 10, 0, 0, false),
        (1_000_000, 2_000_000, 0, 0, false),
        (0, 500, 600, 700, true),
        (0, 500, 300, 300, true),
    ] {
        let mut b = Sf2::new();
        b.pcm(&[10000; 1000]);
        let s = b.sample_header(header(start, end, ls, le));
        let i = b.instrument("i", [Zone::new().set(G::SampleModes, 1).sample(s)]);
        b.preset("p", 0, 0, [Zone::new().instrument(i)]);
        let mut syn = synth(load(&b));
        syn.send(on(0, 60, 127));
        let (l, _) = render(&mut syn, 44100);
        assert!(l.iter().all(|x| x.is_finite()));
        assert_eq!(l.iter().any(|x| *x != 0.0), sounds, "{start} {end} {ls} {le}");
        if ls >= 500 || le <= ls {
            assert!(l[30000..].iter().all(|x| *x == 0.0), "{start} {end} {ls} {le}");
        }
    }
}

/// E-11: modLfoToVolume swings the level by the same amount whatever its
/// sign.
#[test]
fn e11_negative_mod_lfo_to_volume_works() {
    let swing = |amount: i16| {
        // 5 Hz: 1200 log2(5/8.176) cents.
        let zone = Zone::new().set(G::ModLfoToVolume, amount).set(G::FreqModLfo, -851);
        let mut s = synth(single(dc(), zone));
        s.send(cc(0, 7, 127));
        s.send(on(0, 60, 127));
        let (l, _) = render(&mut s, 44100);
        let (lo, hi) = l[2000..].iter().fold((f32::MAX, 0f32), |(lo, hi), x| (lo.min(*x), hi.max(*x)));
        20.0 * f64::from(hi / lo).log10()
    };
    let up = swing(120);
    let down = swing(-120);
    // ±120 cB, of which FluidSynth applies the attenuation scale: 24 dB peak
    // to peak either way.
    assert!((up - 24.0).abs() < 0.5, "{up} dB");
    assert!((down - up).abs() < 0.1, "{down} dB");
}

/// Presets 0:0, 1:0, 0:5, 128:0 and 128:5.
fn banks_sf() -> Arc<SoundFont> {
    let mut b = Sf2::new();
    let s = b.sample(dc());
    let i = b.instrument("i", [Zone::new().sample(s)]);
    for (bank, program) in [(0, 0), (1, 0), (0, 5), (128, 0), (128, 5)] {
        b.preset(&format!("{bank}:{program}"), bank, program, [Zone::new().instrument(i)]);
    }
    load(&b)
}

fn preset_of(s: &Synth, ch: usize) -> Option<(u16, u16)> {
    s.channels[ch].preset.map(|p| {
        let p = &s.sound_font.presets()[p];
        (p.bank, p.program)
    })
}

/// E-12: a drum channel falls back to 128:program, then to 128:0; a
/// melodic one to 0:program, then 0:0.
#[test]
fn e12_drum_kits_fall_back() {
    let mut s = synth(banks_sf());
    assert_eq!(preset_of(&s, 9), Some((128, 0)));
    s.send(cc(9, 0, 3));
    s.send(Event::ProgramChange { channel: 9, program: 5 });
    assert_eq!(preset_of(&s, 9), Some((128, 5)));
    s.send(Event::ProgramChange { channel: 9, program: 7 });
    assert_eq!(preset_of(&s, 9), Some((128, 0)));
    s.send(cc(0, 0, 9));
    s.send(Event::ProgramChange { channel: 0, program: 5 });
    assert_eq!(preset_of(&s, 0), Some((0, 5)));
    s.send(Event::ProgramChange { channel: 0, program: 7 });
    assert_eq!(preset_of(&s, 0), Some((0, 0)));
    s.send(on(9, 36, 100));
    assert_eq!(s.active_voices(), 1);
}

/// E-13: bank select takes effect at the next program change.
#[test]
fn e13_bank_select_waits_for_the_program_change() {
    let mut s = synth(banks_sf());
    s.send(cc(0, 0, 1));
    assert_eq!(preset_of(&s, 0), Some((0, 0)));
    s.send(Event::ProgramChange { channel: 0, program: 0 });
    assert_eq!(preset_of(&s, 0), Some((1, 0)));
    // CC32 is ignored in GS mode.
    s.send(cc(0, 0, 0));
    s.send(cc(0, 32, 1));
    s.send(Event::ProgramChange { channel: 0, program: 0 });
    assert_eq!(preset_of(&s, 0), Some((0, 0)));
}

/// E-14: an NRPN does not change the RPN selected before it, and data entry
/// for the NRPN does not reach the RPN.
#[test]
fn e14_nrpns_leave_the_rpn_alone() {
    let mut s = synth(single(dc(), Zone::new()));
    for e in [cc(0, 101, 0), cc(0, 100, 0), cc(0, 6, 12), cc(0, 38, 0)] {
        s.send(e);
    }
    assert_eq!(s.channels[0].bend_range, 12.0);
    for e in [cc(0, 99, 1), cc(0, 98, 8), cc(0, 6, 3), cc(0, 38, 0)] {
        s.send(e);
    }
    assert_eq!(s.channels[0].bend_range, 12.0);
    assert_eq!((s.channels[0].cc[101], s.channels[0].cc[100]), (0, 0));
    for e in [cc(0, 101, 0), cc(0, 100, 0), cc(0, 6, 1), cc(0, 38, 50)] {
        s.send(e);
    }
    assert_eq!(s.channels[0].bend_range, 1.5);
    for e in [cc(0, 100, 2), cc(0, 6, 76), cc(0, 100, 1), cc(0, 6, 96), cc(0, 38, 0), cc(0, 100, 5), cc(0, 6, 1)] {
        s.send(e);
    }
    let c = &s.channels[0];
    assert_eq!(c.gen_offsets[G::CoarseTune as usize], 12.0);
    assert_eq!(c.gen_offsets[G::FineTune as usize], 50.0);
    assert_eq!(c.mod_depth_range, 100.0);
}

/// SoundFont NRPNs (§9.6) offset a generator.
#[test]
fn soundfont_nrpns_offset_generators() {
    let mut s = synth(single(dc(), Zone::new()));
    let data = 8192 + 100;
    for e in [cc(0, 99, 120), cc(0, 98, 48), cc(0, 38, (data & 127) as u8), cc(0, 6, (data >> 7) as u8)] {
        s.send(e);
    }
    assert_eq!(s.channels[0].gen_offsets[48], 100.0);
}

fn sysex(bytes: &[u8]) -> Event {
    Event::SysEx(SysEx::new(bytes).unwrap())
}

/// A GS data set with its checksum.
fn gs(address: [u8; 3], data: u8) -> Event {
    let sum = address.iter().map(|b| u32::from(*b)).sum::<u32>() + u32::from(data);
    let checksum = ((0x80 - (sum & 0x7F)) & 0x7F) as u8;
    sysex(&[0x41, 0x10, 0x42, 0x12, address[0], address[1], address[2], data, checksum])
}

/// E-15: GS and GM resets reset every channel; GS's use-for-rhythm-part
/// makes a drum channel.
#[test]
fn e15_resets_and_rhythm_parts() {
    let mut s = synth(banks_sf());
    for ch in 0..16 {
        s.send(cc(ch, 7, 5));
        s.send(cc(ch, 0, 1));
        s.send(Event::PitchBend { channel: ch, value: 0 });
    }
    s.send(Event::ProgramChange { channel: 0, program: 5 });
    s.send(on(0, 60, 100));
    // A wrong checksum does nothing.
    s.send(sysex(&[0x41, 0x10, 0x42, 0x12, 0x40, 0x00, 0x7F, 0x00, 0x42]));
    assert_eq!(s.channels[3].cc[7], 5);
    s.send(gs([0x40, 0x00, 0x7F], 0x00));
    for ch in 0..16 {
        let c = &s.channels[ch];
        assert_eq!((c.cc[7], c.pitch_bend), (100, 8192), "{ch}");
    }
    assert_eq!(preset_of(&s, 0), Some((0, 0)));
    assert_eq!(preset_of(&s, 9), Some((128, 0)));
    render(&mut s, 640);
    assert_eq!(s.active_voices(), 0, "the reset stops the sound");
    // Use for rhythm part, part 11 (channel index 10): 40 1A 15.
    s.send(gs([0x40, 0x1A, 0x15], 0x01));
    assert!(s.channels[10].drum);
    assert_eq!(preset_of(&s, 10), Some((128, 0)));
    // Part 10 (channel index 9) is 40 10 15.
    s.send(gs([0x40, 0x10, 0x15], 0x00));
    assert!(!s.channels[9].drum);
    assert_eq!(preset_of(&s, 9), Some((0, 0)));
    // GM System On resets too, and bank selects are then ignored.
    s.send(cc(2, 7, 1));
    s.send(sysex(&[0x7E, 0x7F, 0x09, 0x01]));
    assert_eq!(s.channels[2].cc[7], 100);
    assert!(s.channels[9].drum && !s.channels[10].drum);
    s.send(cc(0, 0, 1));
    s.send(Event::ProgramChange { channel: 0, program: 0 });
    assert_eq!(preset_of(&s, 0), Some((0, 0)));
    // A GS reset brings GS bank selects back.
    s.send(gs([0x40, 0x00, 0x7F], 0x00));
    s.send(cc(0, 0, 1));
    s.send(Event::ProgramChange { channel: 0, program: 0 });
    assert_eq!(preset_of(&s, 0), Some((1, 0)));
}

/// GM2 bank selects: CC0 120 makes a drum channel, 121 a melodic one.
#[test]
fn gm2_bank_select_switches_drums() {
    let mut s = synth(banks_sf());
    s.send(sysex(&[0x7E, 0x7F, 0x09, 0x03]));
    s.send(cc(3, 0, 120));
    s.send(Event::ProgramChange { channel: 3, program: 5 });
    assert_eq!(preset_of(&s, 3), Some((128, 5)));
    s.send(cc(3, 0, 121));
    s.send(cc(3, 32, 1));
    s.send(Event::ProgramChange { channel: 3, program: 0 });
    assert_eq!(preset_of(&s, 3), Some((1, 0)));
}

/// The hold pedal keeps released notes until it is up; the sostenuto pedal
/// keeps only the notes down when it was pressed; All Notes Off respects
/// the pedals, All Sound Off does not; RP-015 lifts them.
#[test]
fn pedals() {
    let sf = single(dc(), Zone::new().set(G::ReleaseVolEnv, -12000));
    let mut s = synth(sf.clone());
    s.send(cc(0, 64, 64));
    s.send(on(0, 60, 100));
    s.send(off(0, 60));
    render(&mut s, 4410);
    assert_eq!(s.active_voices(), 1);
    s.send(cc(0, 64, 63));
    render(&mut s, 4410);
    assert_eq!(s.active_voices(), 0);

    let mut s = synth(sf.clone());
    s.send(on(0, 60, 100));
    s.send(cc(0, 66, 127));
    s.send(on(0, 62, 100));
    s.send(off(0, 60));
    s.send(off(0, 62));
    render(&mut s, 4410);
    let held: Vec<u8> = s.voices.iter().filter(|v| !v.is_free()).map(|v| v.key).collect();
    assert_eq!(held, [60]);
    s.send(cc(0, 123, 0));
    render(&mut s, 4410);
    assert_eq!(s.active_voices(), 1, "All Notes Off respects the sostenuto");
    s.send(cc(0, 66, 0));
    render(&mut s, 4410);
    assert_eq!(s.active_voices(), 0);

    let mut s = synth(sf);
    s.send(cc(0, 64, 127));
    s.send(on(0, 60, 100));
    s.send(cc(0, 120, 0));
    render(&mut s, 200);
    assert_eq!(s.active_voices(), 0, "All Sound Off stops in 2 ms");
    s.send(on(0, 60, 100));
    s.send(off(0, 60));
    s.send(cc(0, 7, 3));
    s.send(cc(0, 121, 0));
    render(&mut s, 4410);
    assert_eq!(s.active_voices(), 0);
    assert_eq!(s.channels[0].cc[7], 3, "RP-015 keeps the volume");
}

/// A note on of a sounding key releases the old voice.
#[test]
fn a_repeated_note_releases_the_old_voice() {
    let mut s = synth(single(dc(), Zone::new().set(G::ReleaseVolEnv, -12000)));
    s.send(on(0, 60, 100));
    render(&mut s, 640);
    s.send(on(0, 60, 100));
    render(&mut s, 4410);
    assert_eq!(s.active_voices(), 1);
}

/// ROM samples are not played.
#[test]
fn rom_samples_do_not_sound() {
    let mut s = synth(single(dc().kind(0x8001, 0), Zone::new()));
    s.send(on(0, 60, 100));
    assert_eq!(s.active_voices(), 0);
}

/// 24-bit samples use their low bytes.
#[test]
fn twenty_four_bit_samples() {
    let sample = SampleSpec::new("dc24", vec![16384; 2000]).looped(100, 1900).low_bytes(vec![0x80; 2000]);
    let mut s = synth(single(sample, Zone::new()));
    s.send(cc(0, 7, 127));
    s.send(on(0, 60, 127));
    let (l, _) = render(&mut s, 4000);
    let expected = full() * (16384.0 * 256.0 + 128.0) / (16384.0 * 256.0);
    assert!((f64::from(l[3999]) - expected).abs() < 1e-6, "{} {expected}", l[3999]);
}

/// The level of a bright tone (a 6.3 kHz sine) through a filter at 9000
/// cents (about 1.5 kHz) and whatever the default modulators take from it.
fn filtered(c: Config, events: &[Event], velocity: u8) -> f64 {
    let sample = SampleSpec::new("hi", sine(44100, 7.0, 20000)).looped(0, 44100);
    let mut s = Synth::new(single(sample, Zone::new().set(G::InitialFilterFc, 9000)), c).unwrap();
    s.send(cc(0, 7, 127));
    for e in events {
        s.send(*e);
    }
    s.send(on(0, 60, velocity));
    let (l, _) = render(&mut s, 8000);
    rms(&l[4000..])
}

/// The default velocity-to-filter modulator is off, as FluidSynth plays;
/// the other forms darken soft notes.
#[test]
fn velocity_to_filter_forms() {
    let with = |form: VelocityToFilter, velocity: u8| {
        let mut c = config();
        c.velocity_to_filter = form;
        filtered(c, &[], velocity)
    };
    use VelocityToFilter::{Linear, Off, Switched};
    // The switched form of SoundFont 2.01: velocities below 64 are untouched,
    // those from 64 lose -2400 × (1 - v/128) cents (1200 at 64).
    assert_eq!(with(Switched, 40), with(Off, 40));
    assert!(with(Switched, 70) < 0.5 * with(Off, 70));
    assert!(with(Switched, 127) > 0.9 * with(Off, 127));
    assert!(with(Linear, 40) < 0.5 * with(Off, 40));
    assert!((with(Linear, 127) / with(Off, 127) - 1.0).abs() < 0.05);
}

/// CC74 moves the cutoff only when asked for.
#[test]
fn cc74_to_filter_is_optional() {
    let with = |on74: bool, value: u8| {
        let mut c = config();
        c.cc74_to_filter = on74;
        filtered(c, &[cc(0, 74, value)], 127)
    };
    assert_eq!(with(false, 0), with(false, 127));
    assert!(with(true, 0) < 0.1 * with(false, 0));
    assert!(with(true, 127) > 3.0 * with(false, 127));
    assert!((with(true, 64) / with(false, 64) - 1.0).abs() < 0.05);
}

/// A small generator of pseudo-random numbers.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn byte(&mut self) -> u8 {
        self.next() as u8
    }
}

/// A random event, its data in range or not.
fn random_event(rng: &mut Rng) -> Event {
    let channel = rng.byte() % 18;
    match rng.next() % 9 {
        0 | 1 => on(channel, rng.byte(), rng.byte()),
        2 => off(channel, rng.byte()),
        3 => cc(channel, rng.byte(), rng.byte()),
        4 => Event::PitchBend { channel, value: rng.next() as u16 },
        5 => Event::ProgramChange { channel, program: rng.byte() },
        6 => Event::ChannelPressure { channel, pressure: rng.byte() },
        7 => Event::PolyPressure { channel, key: rng.byte(), pressure: rng.byte() },
        _ => {
            let n = (rng.next() % 12) as usize;
            let mut d: Vec<u8> = (0..n).map(|_| rng.byte() & 0x7F).collect();
            if n > 3 && rng.next() % 2 == 0 {
                d[..4].copy_from_slice(&[0x41, 0x10, 0x42, 0x12]);
            }
            sysex(&d)
        }
    }
}

/// A SoundFont whose generators and modulators are random.
fn extreme_sf(rng: &mut Rng) -> Arc<SoundFont> {
    let mut b = Sf2::new();
    let s0 = b.sample(SampleSpec::new("a", sine(3000, 9.5, 32767)).looped(5, 7).key(rng.byte()));
    let s1 = b.sample(SampleSpec::new("b", vec![-32768, 32767, -32768, 32767]).looped(0, 4));
    let mut zones = vec![];
    for k in 0..4 {
        let mut z = Zone::new();
        for _ in 0..12 {
            let g = (rng.next() % 61) as u16;
            if !matches!(g, 41..=44 | 53) {
                z = z.raw(g, rng.next() as u16);
            }
        }
        z = z.modulator(modulator(
            crate::soundfont::Source(rng.next() as u16 & 0x0FFF),
            G::InitialFilterFc,
            rng.next() as i16,
            Default::default(),
        ));
        zones.push(z.sample(if k % 2 == 0 { s0 } else { s1 }));
    }
    let i = b.instrument("i", zones);
    b.preset("p", 0, 0, [Zone::new().set(G::InitialFilterQ, 960).instrument(i)]);
    b.preset("d", 128, 0, [Zone::new().instrument(i)]);
    load(&b)
}

/// No sequence of events gives NaN or infinity, with any generators.
#[test]
fn no_events_make_non_finite_output() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for round in 0..20 {
        let sf = extreme_sf(&mut rng);
        let mut c = Config::new([8000, 44100, 192_000][round % 3]);
        c.max_voices = 1 + round * 3;
        c.interpolation = if round % 2 == 0 { Interpolation::Cubic } else { Interpolation::Linear };
        let mut s = Synth::new(sf, c).unwrap();
        for _ in 0..40 {
            for _ in 0..rng.next() % 20 {
                let e = random_event(&mut rng);
                if rng.next() % 2 == 0 {
                    s.send(e);
                } else {
                    let _ = s.schedule(s.now() + rng.next() % 3000, e);
                }
            }
            let n = (rng.next() % 3000) as usize;
            let (l, r) = render(&mut s, n);
            assert!(l.iter().chain(&r).all(|x| x.is_finite()), "round {round}");
            // Gains can be huge, as in FluidSynth: up to +96 dB from
            // modLfoToVolume and 48 dB from the filter's resonance, for
            // each voice. Finite is what matters.
            let peak = l.iter().chain(&r).fold(0f32, |m, x| m.max(x.abs()));
            assert!(peak < 1e9, "round {round}: {peak}");
        }
    }
}

/// `render_interleaved` is `render`, interleaved.
#[test]
fn interleaved_is_the_same() {
    let sf = single(SampleSpec::new("sine", sine(4000, 37.3, 20000)).looped(10, 3739), Zone::new().set(G::Pan, -200));
    let mut a = synth(sf.clone());
    let mut b = synth(sf);
    a.send(on(0, 60, 100));
    b.send(on(0, 60, 100));
    let (l, r) = render(&mut a, 1000);
    let mut out = vec![0.0; 2000];
    assert_eq!(b.render_interleaved(&mut out), 1000);
    for i in 0..1000 {
        assert_eq!((out[2 * i], out[2 * i + 1]), (l[i], r[i]));
    }
}

/// `reset` stops everything at once and keeps the clock.
#[test]
fn reset_stops_everything() {
    let mut s = synth(single(dc(), Zone::new()));
    s.send(on(0, 60, 100));
    s.send(cc(0, 7, 3));
    s.schedule(100_000, on(0, 61, 100)).unwrap();
    render(&mut s, 640);
    s.reset();
    assert_eq!(s.active_voices(), 0);
    assert!(s.is_silent());
    assert_eq!(s.now(), 640);
    assert_eq!(s.channels[0].cc[7], 100);
    let (l, _) = render(&mut s, 200_000);
    assert!(l.iter().all(|x| *x == 0.0), "the queue was not cleared");
}

/// A note off during the attack keeps the level: FluidSynth converts the
/// linear attack's amplitude to the release's dB.
#[test]
fn release_during_the_attack_keeps_the_level() {
    let mut s = synth(single(dc(), Zone::new().set(G::AttackVolEnv, 0).set(G::ReleaseVolEnv, 2400)));
    s.send(cc(0, 7, 127));
    s.send(on(0, 60, 127));
    render(&mut s, 6400);
    s.send(off(0, 60));
    let v = s.voices.iter().find(|v| !v.is_free()).unwrap();
    assert_eq!(v.volume_envelope().0, Stage::Release);
    let (l, _) = render(&mut s, 640);
    let before = full() * 6400.0 / 44100.0;
    assert!((f64::from(l[0]) / before - 1.0).abs() < 2e-2, "{} {before}", l[0]);
    assert!((f64::from(l[64]) / before - 1.0).abs() < 2e-2);
}

/// A note on starts at most `max_voices` voices however many regions it
/// has, and takes the voices it needs from the older notes.
#[test]
fn note_ons_start_at_most_max_voices() {
    let mut b = Sf2::new();
    let s0 = b.sample(dc());
    let i = b.instrument("many", (0..300).map(|_| Zone::new().set(G::SampleModes, 1).sample(s0)));
    b.preset("p", 0, 0, [Zone::new().instrument(i)]);
    let mut c = config();
    c.max_voices = 16;
    let mut s = Synth::new(load(&b), c).unwrap();
    s.send(on(0, 60, 127));
    assert_eq!(s.active_voices(), 16);
    render(&mut s, 640);
    s.send(on(0, 61, 127));
    let live: Vec<u8> = s.voices.iter().filter(|v| v.is_live()).map(|v| v.key).collect();
    assert_eq!(live, [61; 16]);
    assert_eq!(s.active_voices(), 32);
    render(&mut s, 640);
    assert_eq!(s.active_voices(), 16);
}

/// E-01: events of other channels do not touch a voice: its output is the
/// same, to the bit, with or without them.
#[test]
fn e01_events_of_other_channels_leave_voices_alone() {
    let zone = Zone::new()
        .set(G::VibLfoToPitch, 50)
        .set(G::ModLfoToFilterFc, 2000)
        .set(G::DecayVolEnv, -2000)
        .set(G::SustainVolEnv, 300);
    let sample = SampleSpec::new("sine", sine(44100, 100.0, 20000)).looped(0, 44100);
    let sf = single(sample, zone);
    let play = |dense: bool| {
        let mut s = synth(sf.clone());
        s.send(on(0, 60, 127));
        if dense {
            for f in (5..4000).step_by(3) {
                s.schedule(f, cc(15, 1, (f % 128) as u8)).unwrap();
            }
        }
        s.schedule(2001, off(0, 60)).unwrap();
        render(&mut s, 4000)
    };
    assert_eq!(play(false), play(true));
}

/// E-01: what a sequencer plays does not depend on the lengths it is
/// rendered in, though it queues events only as far as each render goes.
#[test]
fn e01_sequenced_output_does_not_depend_on_the_render_lengths() {
    use crate::midi::{LoopMarkers, Sequencer, Smf};
    use crate::testkit::smf::{SmfBuilder, Track};
    let track = Track::new()
        .note_on(0, 0, 60, 100)
        .cc(37, 0, 7, 90)
        .note_on(11, 1, 64, 100)
        .pitch_bend(50, 0, 9000)
        .cc(3, 1, 10, 20)
        .note_off(200, 0, 60)
        .note_on(7, 0, 67, 80)
        .note_off(300, 1, 64)
        .end(500);
    let smf = Arc::new(Smf::from_bytes(&SmfBuilder::new().track(track).build(), LoopMarkers::None).unwrap());
    let sf = single(dc(), Zone::new().set(G::DecayVolEnv, -2000).set(G::SustainVolEnv, 300));
    let play = |block: usize| {
        let mut s = synth(sf.clone());
        let mut seq = Sequencer::new(Arc::clone(&smf), &s);
        let (mut l, mut r) = (vec![0.0; 60000], vec![0.0; 60000]);
        for (cl, cr) in l.chunks_mut(block).zip(r.chunks_mut(block)) {
            assert_eq!(seq.render(&mut s, cl, cr), cl.len());
        }
        (l, r)
    };
    let whole = play(60000);
    assert!(whole.0.iter().any(|x| *x != 0.0));
    for block in [37, 64, 100, 1000] {
        assert!(play(block) == whole, "blocks of {block}");
    }
}

/// The filter stays stable under fast deep modulation of a high Q, as
/// FluidSynth's (whose peaks in these cases are 0.3 to 1.7): gliding the
/// coefficients instead of the cutoff reached 1e34.
#[test]
fn the_filter_stays_stable_under_fast_deep_modulation() {
    let square: Vec<i16> = (0..2000).map(|i| if i % 100 < 50 { 16000 } else { -16000 }).collect();
    for (q, depth, rate) in [(100, 12000, 4500), (200, 4800, 4500), (300, 12000, 4500), (960, 12000, 4500)] {
        let zone = Zone::new()
            .set(G::InitialFilterQ, q)
            .set(G::InitialFilterFc, 8000)
            .set(G::ModLfoToFilterFc, depth)
            .set(G::FreqModLfo, rate);
        let mut s = synth(single(SampleSpec::new("square", square.clone()).looped(0, 2000), zone));
        s.send(cc(0, 7, 127));
        s.send(on(0, 60, 127));
        let (l, r) = render(&mut s, 2 * 44100);
        let peak = l.iter().chain(&r).fold(0.0f32, |m, x| m.max(x.abs()));
        assert!(peak < 10.0, "Q {q}, depth {depth}, rate {rate}: peak {peak}");
        assert_eq!(s.active_voices(), 1, "Q {q}, depth {depth}, rate {rate}: stopped");
        assert!(l[l.len() - 4410..].iter().any(|x| x.abs() > 1e-3), "Q {q}, depth {depth}, rate {rate}: silent");
    }
}

/// A zone keeps at most 64 modulators, as many as a voice can play: a
/// preset zone of hundreds made every note on quadratic.
#[test]
fn zones_keep_at_most_64_modulators() {
    let mut zone = Zone::new();
    for cc in [1u8, 2, 4, 5, 7, 10, 11] {
        for dest in [G::InitialFilterFc, G::Pan, G::ChorusEffectsSend, G::ReverbEffectsSend, G::FineTune] {
            for curve in 0..4 {
                zone = zone.modulator(modulator(source(cc, true, false, false, curve), dest, 10, Default::default()));
            }
        }
    }
    let mut b = Sf2::new();
    let s0 = b.sample(dc());
    let i = b.instrument("i", [Zone::new().set(G::SampleModes, 1).sample(s0)]);
    b.preset("p", 0, 0, [zone.instrument(i)]);
    let sf = load(&b);
    assert_eq!(sf.presets[0].zones[0].zone.modulators.len(), 64);
    let mut s = synth(sf);
    s.send(on(0, 60, 127));
    assert_eq!(s.active_voices(), 1);
}

/// Regions with nothing to play take no voice: a note on of them at full
/// polyphony leaves the sounding notes alone.
#[test]
fn regions_with_nothing_to_play_take_no_voice() {
    let mut b = Sf2::new();
    let s0 = b.sample(dc());
    let empty = b.sample(SampleSpec::new("empty", vec![0; 1]));
    let i = b.instrument("i", [Zone::new().set(G::SampleModes, 1).sample(s0)]);
    let e = b.instrument("e", (0..500).map(|_| Zone::new().sample(empty)));
    b.preset("p", 0, 0, [Zone::new().instrument(i)]);
    b.preset("e", 0, 1, [Zone::new().instrument(e)]);
    let mut c = config();
    c.max_voices = 4;
    let mut s = Synth::new(load(&b), c).unwrap();
    for key in 60..64 {
        s.send(on(0, key, 100));
    }
    s.send(Event::ProgramChange { channel: 1, program: 1 });
    s.send(on(1, 60, 100));
    assert_eq!(s.voices.iter().filter(|v| v.is_live()).count(), 4);
    assert!(s.voices.iter().all(|v| !v.fading));
}

/// E-03: scaleTuning, summed over the instrument and the preset, is
/// clamped to 1200 cents a key.
#[test]
fn e03_scale_tuning_is_clamped() {
    // 100 Hz at the root: 200 Hz a key up with 1200 cents a key, 1144 Hz
    // with 2100.
    let sample = SampleSpec::new("sine", sine(44100, 441.0, 20000)).looped(0, 44100).key(60);
    let mut b = Sf2::new();
    let s0 = b.sample(sample);
    let i = b.instrument("i", [Zone::new().set(G::SampleModes, 1).set(G::ScaleTuning, 100).sample(s0)]);
    b.preset("p", 0, 0, [Zone::new().set(G::ScaleTuning, 2000).instrument(i)]);
    let mut s = synth(load(&b));
    s.send(on(0, 61, 127));
    let (l, _) = render(&mut s, 44100);
    let f = frequency(&l[4410..]);
    assert!((f / 200.0 - 1.0).abs() < 0.01, "{f}");
}

/// The sostenuto pedal holds the notes sounding when it is pressed, not
/// those started while it is down, even if it is sent again.
#[test]
fn sostenuto_holds_only_the_notes_sounding_when_pressed() {
    let sf = single(dc(), Zone::new().set(G::ReleaseVolEnv, 0));
    let mut s = synth(sf);
    s.send(on(0, 60, 100));
    s.send(cc(0, 66, 127));
    s.send(on(0, 62, 100));
    s.send(cc(0, 66, 100));
    s.send(off(0, 60));
    s.send(off(0, 62));
    let status = |s: &Synth, key| s.voices.iter().find(|v| !v.is_free() && v.key == key).map(|v| v.status);
    assert_eq!(status(&s, 60), Some(voice::Status::Sostenuto));
    assert_eq!(status(&s, 62), Some(voice::Status::Released));
}

/// A low cutoff at a high sample rate with resonance: the filter's state
/// is far larger than its output there (the gain at resonance is about Q
/// over sin² of the cutoff's angle), and the voice plays on.
#[test]
fn resonance_at_a_low_cutoff_plays_on() {
    for (rate, q) in [(192_000u32, 400i16), (44_100, 700)] {
        let hz = 440.0 * ((2700.0 - 6900.0) / 1200.0f64).exp2();
        let period = f64::from(rate) / hz;
        let len = (period * 40.0).round() as usize;
        let sample = SampleSpec::new("sine", sine(len, period, 16000)).looped(0, len as u32).key(60).rate(rate);
        let zone = Zone::new().set(G::InitialFilterFc, 2700).set(G::InitialFilterQ, q);
        let mut c = config();
        c.sample_rate = rate;
        let mut s = Synth::new(single(sample, zone), c).unwrap();
        s.send(cc(0, 7, 127));
        s.send(on(0, 60, 127));
        let (l, _) = render(&mut s, rate as usize * 10);
        assert_eq!(s.active_voices(), 1, "{rate} Hz, Q {q}: stopped");
        assert!(l[l.len() - 1000..].iter().any(|x| x.abs() > 0.01));
    }
}

/// The gain multiplies the output.
#[test]
fn the_gain_scales_the_output() {
    let sf = single(SampleSpec::new("sine", sine(4000, 37.3, 20000)).looped(10, 3739), Zone::new());
    let play = |gain: f32| {
        let mut c = Config::new(SR);
        c.gain = gain;
        let mut s = Synth::new(sf.clone(), c).unwrap();
        s.send(cc(0, 91, 127));
        s.send(on(0, 60, 100));
        render(&mut s, 5000)
    };
    let (l, r) = play(1.0);
    let (ql, qr) = play(0.25);
    assert!(l.iter().any(|x| x.abs() > 0.1));
    assert!(l.iter().zip(&ql).all(|(a, b)| a * 0.25 == *b));
    assert!(r.iter().zip(&qr).all(|(a, b)| a * 0.25 == *b));
}

/// A note sounds at least `min_note_length`: released sooner, by a note
/// off or the hold pedal, it releases then; released later, as it was.
#[test]
fn short_notes_sound_the_minimum_length() {
    let sf = single(dc(), Zone::new().set(G::ReleaseVolEnv, -12000));
    let play = |min_ms: u64, events: &[(u64, Event)]| {
        let mut c = config();
        c.min_note_length = std::time::Duration::from_millis(min_ms);
        let mut s = Synth::new(sf.clone(), c).unwrap();
        for &(frame, e) in events {
            s.schedule(frame, e).unwrap();
        }
        render(&mut s, 3000).0
    };
    // 10 ms is 441 frames.
    let at_min = play(10, &[(0, on(0, 60, 127)), (441, off(0, 60))]);
    assert!(at_min[440] > 0.1, "{}", at_min[440]);
    // The release, -7200 tc at the least (16 ms for 96 dB), from there.
    assert!(at_min[441 + 4] < at_min[440]);
    assert!(at_min[441 + 1500] < 1e-4);
    // From the end of the attack, within rounding: an event cuts the
    // segment it falls in, which changes how the gain gets to its level.
    let near = |x: &[f32]| x[200..].iter().zip(&at_min[200..]).all(|(a, b)| (a - b).abs() < 1e-6);
    for frame in [0, 1, 63, 64, 300, 440] {
        assert!(near(&play(10, &[(0, on(0, 60, 127)), (frame, off(0, 60))])), "note off at {frame}");
        let pedal = [(0, cc(0, 64, 127)), (0, on(0, 60, 127)), (0, off(0, 60)), (frame, cc(0, 64, 0))];
        assert!(near(&play(10, &pedal)), "pedal up at {frame}");
    }
    let long = [(0, on(0, 60, 127)), (1000, off(0, 60))];
    assert!(play(10, &long) == play(0, &long));
    // None: a note of no time is silent.
    assert!(play(0, &[(0, on(0, 60, 127)), (0, off(0, 60))]).iter().all(|x| *x == 0.0));
}

/// A sound font whose note ons cost far more than the budget holds: 256
/// regions, each with 64 modulators.
fn costly_sf() -> Arc<SoundFont> {
    let mut b = Sf2::new();
    let s0 = b.sample(dc());
    let mut iz = Zone::new().set(G::SampleModes, 1);
    for (i, d) in [G::InitialFilterFc, G::Pan, G::FineTune, G::CoarseTune].into_iter().enumerate() {
        for cc in 0..16 {
            iz = iz.modulator(modulator(source(cc + 16 * i as u8, true, false, false, 0), d, 1, Default::default()));
        }
    }
    let i = b.instrument("i", (0..256).map(|_| iz.clone().sample(s0)));
    b.preset("p", 0, 0, [Zone::new().instrument(i)]);
    load(&b)
}

/// Events cost work out of a budget earned by the frames rendered: a flood
/// of costly events waits and plays late, in order, and all of it.
#[test]
fn a_flood_of_costly_events_plays_late_in_order() {
    let mut s = synth(costly_sf());
    for k in 0..200u8 {
        s.schedule(0, on(0, k % 128, 100)).unwrap();
        s.schedule(0, cc(0, 7, k % 128)).unwrap();
    }
    s.schedule(0, cc(0, 7, 99)).unwrap();
    render(&mut s, 64);
    let waiting = s.queue.len();
    assert!(waiting > 300, "{waiting} events left after 64 frames");
    // An event sent meanwhile waits behind them.
    s.send(cc(0, 7, 33));
    assert!(s.queue.len() + 2 > waiting);
    assert_ne!(s.channels[0].cc[7], 33);
    let mut frames = 64;
    while s.queue.len() > 0 {
        render(&mut s, 64);
        frames += 64;
        assert!(frames < 10 * SR as usize);
    }
    assert_eq!(s.channels[0].cc[7], 33);
    // About a quarter of the time at a nanosecond a unit: each note on starts
    // 256 voices of 64 modulators.
    assert!(frames > SR as usize / 4, "{frames} frames");
}

/// Music plays far within the budget: 256 notes at once, all on time.
#[test]
fn chords_play_within_the_budget() {
    let mut s = synth(single(dc(), Zone::new()));
    for k in 0..256u32 {
        s.schedule(0, on((k % 2) as u8, (k / 2) as u8, 100)).unwrap();
        s.schedule(0, cc((k % 2) as u8, 10, k as u8 % 128)).unwrap();
    }
    render(&mut s, 1);
    assert_eq!(s.queue.len(), 0);
    assert_eq!(s.active_voices(), 256);
}

/// E-01: when events wait for the budget does not depend on the render
/// lengths either.
#[test]
fn e01_waiting_for_the_budget_does_not_depend_on_the_render_lengths() {
    let sf = costly_sf();
    let play = |chunks: &[usize]| {
        let mut s = synth(sf.clone());
        for k in 0..20u64 {
            s.schedule(k * 7, on((k % 16) as u8, 40 + k as u8, 100)).unwrap();
            s.schedule(k * 7 + 3, cc((k % 16) as u8, 10, 0)).unwrap();
            s.schedule(k * 7 + 3000, off((k % 16) as u8, 40 + k as u8)).unwrap();
        }
        let mut out = vec![];
        let mut k = 0;
        while out.len() < 20000 {
            let n = chunks[k % chunks.len()].min(20000 - out.len());
            out.extend(render(&mut s, n).0);
            k += 1;
        }
        out
    };
    let one = play(&[20000]);
    assert!(one.iter().any(|x| *x != 0.0));
    for chunks in [&[1usize][..], &[7], &[64], &[1000], &[13, 1, 200, 64]] {
        assert!(play(chunks) == one, "chunks {chunks:?}");
    }
}

/// Events waiting for the budget drain at the rate it earns, whatever the
/// sample rate and the voice slots: waiting does not itself cost.
#[test]
fn waiting_events_drain_at_the_budget_rate() {
    let sf = single(dc(), Zone::new());
    for (rate, voices) in [(44100, 256), (48000, 256), (96000, 256), (192_000, 256), (48000, 4096)] {
        let mut c = config();
        c.sample_rate = rate;
        c.max_voices = voices;
        c.event_capacity = 65536;
        let mut s = Synth::new(sf.clone(), c).unwrap();
        // 4 million units of control changes, twice what the budget holds.
        for k in 0..60_000u32 {
            s.schedule(0, cc(0, 1, (k % 128) as u8)).unwrap();
        }
        let mut frames = 0;
        while s.queue.len() > 0 {
            render(&mut s, 64);
            frames += 64;
            // 2 million units over at 1e8 a second: 20 ms, and a block.
            assert!(frames < rate as usize / 20, "{rate} Hz, {voices} voices: {} left", s.queue.len());
        }
    }
}

/// Sending while the queue is full of events waiting for the budget plays
/// one of them to make room, not all.
#[test]
fn sending_to_a_full_queue_does_one_more_event() {
    let mut c = config();
    c.event_capacity = 64;
    let mut s = Synth::new(costly_sf(), c).unwrap();
    for k in 0..64u8 {
        s.schedule(0, on(0, k, 100)).unwrap();
    }
    render(&mut s, 1);
    let waiting = s.queue.len();
    assert!(waiting > 32);
    s.schedule(0, cc(0, 7, 1)).unwrap_or_default();
    while s.queue.len() < 64 {
        s.schedule(0, cc(0, 7, 2)).unwrap();
    }
    let voices = s.active_voices();
    s.send(cc(0, 7, 99));
    assert_eq!(s.queue.len(), 64);
    // One note on more played, of 256 voices.
    assert!(s.active_voices() <= voices + 2 * 256, "{voices} → {}", s.active_voices());
    while s.queue.len() > 0 {
        render(&mut s, 64);
    }
    assert_eq!(s.channels[0].cc[7], 99);
}

/// A synthesizer with events due waiting for the budget is not silent.
#[test]
fn waiting_events_are_not_silence() {
    let mut s = synth(costly_sf());
    for k in 0..20u8 {
        s.schedule(0, on(0, k, 100)).unwrap();
    }
    render(&mut s, 1);
    s.send(Event::ControlChange { channel: 0, controller: 120, value: 0 });
    // Still waiting behind the note ons.
    assert!(!s.is_silent());
}

/// An odd sample at the end of an interleaved buffer is cleared.
#[test]
fn interleaved_odd_lengths() {
    let mut s = synth(single(dc(), Zone::new()));
    s.send(on(0, 60, 127));
    let mut out = [1.0f32; 201];
    assert_eq!(s.render_interleaved(&mut out), 100);
    assert_eq!(out[200], 0.0);
}

/// E-01: under a dense stream of events that keeps the budget spent, the
/// output still does not depend on the render lengths.
#[test]
fn e01_dense_events_under_the_budget_do_not_depend_on_the_render_lengths() {
    let mut b = Sf2::new();
    let s0 = b.sample(dc());
    let i = b.instrument("i", (0..32).map(|_| Zone::new().set(G::SampleModes, 1).sample(s0)));
    b.preset("p", 0, 0, [Zone::new().instrument(i)]);
    let sf = load(&b);
    let play = |rate: u32, seed: u64, sizes: &mut dyn FnMut() -> usize| {
        let mut c = config();
        c.sample_rate = rate;
        let mut s = Synth::new(sf.clone(), c).unwrap();
        let mut rng = Rng(seed);
        let mut frame = 0;
        for _ in 0..600 {
            frame += rng.next() % 20;
            let e = match rng.next() % 10 {
                0..=2 => on(0, rng.byte() % 128, 100),
                3 | 4 => off(0, rng.byte() % 128),
                5..=8 => cc(0, 1, rng.byte() % 128),
                _ => Event::PitchBend { channel: 0, value: rng.next() as u16 % 16384 },
            };
            s.schedule(frame, e).unwrap();
        }
        let mut out = vec![];
        while out.len() < 8000 {
            let n = sizes().min(8000 - out.len());
            out.extend(render(&mut s, n).0);
        }
        out
    };
    for rate in [44100, 48000, 96000] {
        for seed in 1..5 {
            let blocks = play(rate, seed, &mut || 64);
            let mut rng = Rng(seed * 7919 + 1);
            let random = play(rate, seed, &mut || 1 + (rng.next() % 700) as usize);
            assert!(blocks == random, "{rate} Hz, seed {seed}");
        }
    }
}
