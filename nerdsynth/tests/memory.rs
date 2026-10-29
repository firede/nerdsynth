//! What loading allocates (P-SF-03, P-SF-12), and that playing allocates
//! nothing, measured by an allocator that counts each thread's bytes and
//! allocations, so the tests can run in parallel.
#![cfg(feature = "testkit")]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use nerdsynth::SoundFont;
use nerdsynth::testkit::sf2::{SampleSpec, Sf2, Zone, find_chunk, sine};

struct Counting;

thread_local! {
    static NOW: Cell<usize> = const { Cell::new(0) };
    static PEAK: Cell<usize> = const { Cell::new(0) };
    static CALLS: Cell<usize> = const { Cell::new(0) };
}

fn add(n: usize) {
    let _ = CALLS.try_with(|calls| calls.set(calls.get() + 1));
    let _ = NOW.try_with(|now| {
        now.set(now.get() + n);
        let _ = PEAK.try_with(|peak| peak.set(peak.get().max(now.get())));
    });
}

fn sub(n: usize) {
    let _ = NOW.try_with(|now| now.set(now.get().saturating_sub(n)));
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            add(layout.size());
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        unsafe { System.dealloc(p, layout) };
        sub(layout.size());
    }
    unsafe fn realloc(&self, p: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let q = unsafe { System.realloc(p, layout, new_size) };
        if !q.is_null() {
            // As if the old block lived until the new one was filled.
            add(new_size);
            sub(layout.size());
        }
        q
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// The most bytes `f` had allocated at once, above what was allocated
/// before.
fn peak_of<T>(f: impl FnOnce() -> T) -> (T, usize) {
    let base = NOW.with(Cell::get);
    PEAK.with(|p| p.set(base));
    let out = f();
    (out, PEAK.with(Cell::get) - base)
}

/// How many times `f` allocated (or reallocated).
fn allocations_of<T>(f: impl FnOnce() -> T) -> (T, usize) {
    let base = CALLS.with(Cell::get);
    let out = f();
    (out, CALLS.with(Cell::get) - base)
}

/// P-SF-12: loading a file of 8 MB of samples allocates about as much as
/// the samples, not twice.
#[test]
fn loading_allocates_little_more_than_the_samples() {
    let b =
        Sf2::single(SampleSpec::new("long", sine(4_000_000, 100.0, 10000)).low_bytes(vec![1; 4_000_000]), Zone::new());
    let bytes = b.build();
    let (sf, peak) = peak_of(|| SoundFont::from_bytes(&bytes).unwrap());
    let data = sf.sample_data().len() * 2 + sf.sample_data24().unwrap().len();
    assert!(data >= 12_000_000);
    eprintln!("peak {peak} bytes for {data} bytes of samples");
    assert!(peak as f64 <= data as f64 * 1.2, "peak {peak} bytes for {data} bytes of samples");
}

/// P-SF-03: a small file whose chunks claim gigabytes costs little.
#[test]
fn huge_claims_allocate_little() {
    for id in
        [b"smpl", b"sm24", b"phdr", b"pbag", b"pmod", b"pgen", b"inst", b"ibag", b"imod", b"igen", b"shdr", b"ICMT"]
    {
        let mut b = Sf2::single(SampleSpec::new("s", vec![0; 1000]), Zone::new());
        b.chunk(nerdsynth::testkit::sf2::Place::Info, b"ICMT", b"comment");
        b.sm24(vec![0; 1046]);
        let mut bytes = b.build();
        let at = find_chunk(&bytes, id).unwrap().start - 4;
        let lists: Vec<_> = [b"INFO", b"sdta", b"pdta"].iter().map(|l| find_chunk(&bytes, l).unwrap()).collect();
        // A multiple of every record size; and every enclosing chunk claims
        // as much, so that only the end of the data stops the reader.
        bytes[at..at + 4].copy_from_slice(&(0x7FFF_0000u32 / 96140 * 96140).to_le_bytes());
        for r in lists.iter().filter(|r| r.contains(&at)) {
            bytes[r.start - 8..r.start - 4].copy_from_slice(&0x7FFF_FFF8u32.to_le_bytes());
        }
        bytes[4..8].copy_from_slice(&0xFFFF_FFF0u32.to_le_bytes());
        let (result, peak) = peak_of(|| SoundFont::from_bytes(&bytes));
        assert!(result.is_err(), "{id:?} loaded");
        assert!(peak < 1 << 20, "{id:?}: allocated {peak} bytes for a file of {}", bytes.len());
    }
}

/// The most any file in the directory `NERDSYNTH_CORPUS` (such as the
/// fuzzer's corpus) allocates and takes to load, to check that no input of
/// a few kilobytes costs much: `NERDSYNTH_CORPUS=fuzz/corpus/soundfont_read
/// cargo test --release --all-features --test memory -- --ignored`.
#[test]
#[ignore]
fn corpus_costs() {
    let Some(dir) = std::env::var_os("NERDSYNTH_CORPUS") else { return };
    let (mut worst_peak, mut worst_time) = ((0, String::new()), (0.0, String::new()));
    let (mut n, mut loaded, mut regions) = (0, 0, 0);
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let bytes = std::fs::read(&path).unwrap();
        let started = std::time::Instant::now();
        let (_, peak) = peak_of(|| {
            if let Ok(sf) = SoundFont::from_bytes(&bytes) {
                loaded += 1;
                let mut mods = nerdsynth::soundfont::ModList::new();
                for p in 0..sf.presets().len().min(16) {
                    for r in sf.regions(p, 60, 100) {
                        regions += 1;
                        r.modulators(&mut mods);
                    }
                }
            }
        });
        let t = started.elapsed().as_secs_f64();
        let name = format!("{} ({} bytes)", path.display(), bytes.len());
        if peak > worst_peak.0 {
            worst_peak = (peak, name.clone());
        }
        if t > worst_time.0 {
            worst_time = (t, name);
        }
        n += 1;
    }
    eprintln!("{n} inputs, {loaded} load, {regions} regions at key 60");
    eprintln!(
        "most allocated {} bytes: {}; slowest {:.6} s: {}",
        worst_peak.0, worst_peak.1, worst_time.0, worst_time.1
    );
}

/// A SoundFont of a looped sine with exclusive classes, an LFO and a
/// resonant filter, for the playing tests.
fn playing_font() -> std::sync::Arc<SoundFont> {
    use nerdsynth::soundfont::GeneratorType as G;
    let mut b = Sf2::new();
    let s = b.sample(SampleSpec::new("s", sine(4000, 37.3, 20000)).looped(10, 3739));
    let i = b.instrument(
        "i",
        [
            Zone::new().keys(0, 63).set(G::SampleModes, 1).set(G::ExclusiveClass, 1).sample(s),
            Zone::new()
                .keys(64, 127)
                .set(G::SampleModes, 1)
                .set(G::ModLfoToFilterFc, 2000)
                .set(G::InitialFilterQ, 200)
                .sample(s),
        ],
    );
    b.preset("p", 0, 0, [Zone::new().instrument(i)]);
    b.preset("d", 128, 0, [Zone::new().instrument(i)]);
    std::sync::Arc::new(b.load())
}

/// Playing allocates nothing: sending, scheduling (until the queue is full)
/// and rendering, with voices stolen, the effects on, resets and SysEx.
#[test]
fn playing_allocates_nothing() {
    use nerdsynth::event::SysEx;
    use nerdsynth::{Config, Event, Synth};
    let mut c = Config::new(44100);
    c.max_voices = 8;
    c.event_capacity = 64;
    let mut synth = Synth::new(playing_font(), c).unwrap();
    let (mut l, mut r) = (vec![0.0f32; 4096], vec![0.0f32; 4096]);
    let mut out = vec![0.0f32; 8192];
    let gs_reset = Event::SysEx(SysEx::new(&[0x41, 0x10, 0x42, 0x12, 0x40, 0x00, 0x7F, 0x00, 0x41]).unwrap());
    let ((), calls) = allocations_of(|| {
        for round in 0..200u64 {
            for k in 0..12u8 {
                let key = (round as u8).wrapping_mul(7).wrapping_add(k * 5) % 128;
                synth.send(Event::NoteOn { channel: k % 16, key, velocity: 100 });
                let _ = synth.schedule(synth.now() + u64::from(k) * 97, Event::NoteOff { channel: k % 16, key });
                synth.send(Event::ControlChange {
                    channel: k % 16,
                    controller: [1, 7, 10, 91, 93, 74][k as usize % 6],
                    value: key,
                });
            }
            for _ in 0..100 {
                let _ = synth.schedule(synth.now() + 3, Event::PitchBend { channel: 0, value: 0 });
            }
            if round % 50 == 49 {
                synth.send(gs_reset);
            }
            if round % 2 == 0 {
                synth.render(&mut l[..1000 + round as usize], &mut r[..1000 + round as usize]);
            } else {
                synth.render_interleaved(&mut out[..2 * 777]);
            }
            if round == 150 {
                synth.reset();
            }
        }
    });
    assert_eq!(calls, 0, "playing allocated {calls} times");
    // The count works.
    assert_eq!(allocations_of(|| std::hint::black_box(vec![1u8; 10])).1, 1);
}

/// A sequencer playing a looping file through a small queue allocates
/// nothing either.
#[test]
fn sequencing_allocates_nothing() {
    use nerdsynth::midi::{LoopMarkers, Sequencer, Smf};
    use nerdsynth::testkit::smf::{SmfBuilder, Track};
    use nerdsynth::{Config, Synth};
    let mut t = Track::new().cc(0, 0, 111, 0);
    for i in 0..400u32 {
        let key = 30 + (i % 60) as u8;
        t = t.cc(0, (i % 16) as u8, 7, (i % 128) as u8).note_on(
            if i % 50 == 0 { 120 } else { 0 },
            (i % 16) as u8,
            key,
            90,
        );
    }
    let smf = Smf::from_bytes(&SmfBuilder::new().track(t.end(480)).build(), LoopMarkers::RpgMaker).unwrap();
    let mut c = Config::new(44100);
    c.event_capacity = 16;
    c.max_voices = 32;
    let mut synth = Synth::new(playing_font(), c).unwrap();
    let mut made = Sequencer::new(std::sync::Arc::new(smf), &synth);
    made.set_looping(true);
    // A clone too, as made on another thread and sent to the audio thread.
    for mut seq in [made.clone(), made] {
        let (mut l, mut r) = (vec![0.0f32; 4096], vec![0.0f32; 4096]);
        // Once, to settle anything lazily made on first use.
        seq.render(&mut synth, &mut l, &mut r);
        let ((), calls) = allocations_of(|| {
            for i in 0..600 {
                let n = [4096, 64, 1, 1000][i % 4];
                seq.render(&mut synth, &mut l[..n], &mut r[..n]);
            }
        });
        assert_eq!(calls, 0, "sequencing allocated {calls} times");
    }
}
