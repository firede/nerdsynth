//! Loads a MIDI file; if it loads, plays it in a loop for a while, seeking
//! once. Nothing may panic, and every call must take a bounded time.

#![no_main]

use std::sync::Arc;
use std::time::{Duration, Instant};

use libfuzzer_sys::fuzz_target;
use nerdsynth::midi::{LoopMarkers, Sequencer, Smf};

const RATE: u32 = 48_000;
/// Frames a call asks for: 0.1 s.
const STEP: u64 = 4_800;
/// The longest a call may take, far over what it needs.
const BOUND: Duration = Duration::from_millis(500);

fuzz_target!(|data: &[u8]| {
    let Some((&control, file)) = data.split_first() else { return };
    let markers = match control & 3 {
        0 => LoopMarkers::None,
        1 => LoopMarkers::RpgMaker,
        2 => LoopMarkers::IncredibleMachine,
        _ => LoopMarkers::FinalFantasy,
    };
    let started = Instant::now();
    let loaded = Smf::from_bytes(file, markers);
    assert!(started.elapsed() < BOUND, "loading took {:?}", started.elapsed());
    let Ok(smf) = loaded else { return };
    let duration = smf.duration();
    assert!(duration <= Duration::from_secs(24 * 60 * 60));

    let mut seq = Sequencer::with_sample_rate(Arc::new(smf), RATE).unwrap();
    seq.set_looping(control & 4 == 0);
    let mut last = 0;
    let mut until = 0;
    for call in 0..40u64 {
        if call == 20 {
            // Somewhere in the file, or past it.
            let at = duration.mul_f64((control >> 3) as f64 / 16.0);
            let started = Instant::now();
            seq.seek(at);
            assert!(started.elapsed() < BOUND, "seeking took {:?}", started.elapsed());
            assert!(seq.position() <= duration);
        }
        until += STEP;
        let started = Instant::now();
        let mut count = 0u64;
        seq.next_events(until, |frame, _| {
            assert!(frame >= last && frame < until, "frame {frame} after {last}, before {until}");
            last = frame;
            count += 1;
        });
        assert!(started.elapsed() < BOUND, "a call took {:?} for {count} events", started.elapsed());
        assert!(seq.position() <= duration);
        let _ = seq.finished();
    }
});
