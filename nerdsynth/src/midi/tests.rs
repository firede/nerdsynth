use std::io::Read;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::{LoopMarkers, Sequencer, Smf};
use crate::event::SysEx;
use crate::synth::BankStyle;
use crate::testkit::smf::{SmfBuilder, Track};
use crate::{Error, Event};

fn load(builder: &SmfBuilder, markers: LoopMarkers) -> Smf {
    Smf::from_bytes(&builder.build(), markers).unwrap()
}

/// The events of `smf` in order.
fn events(smf: &Smf) -> Vec<Event> {
    smf.events.iter().map(|e| e.event(&smf.sysex)).collect()
}

/// A sequencer at any rate, for times easy to read: at 1000 Hz a frame is a
/// millisecond.
fn sequencer(smf: Arc<Smf>, rate: u32) -> Sequencer {
    Sequencer::make(smf, rate, BankStyle::Gs)
}

/// The events up to `until`, handed out by one call.
fn play(smf: Smf, rate: u32, looping: bool, until: u64) -> Vec<(u64, Event)> {
    let mut seq = sequencer(Arc::new(smf), rate);
    seq.set_looping(looping);
    let mut out = Vec::new();
    seq.next_events(until, |frame, event| out.push((frame, event)));
    out
}

fn note_ons(events: &[(u64, Event)]) -> Vec<(u64, u8)> {
    events
        .iter()
        .filter_map(|&(frame, e)| match e {
            Event::NoteOn { key, velocity, .. } if velocity > 0 => Some((frame, key)),
            _ => None,
        })
        .collect()
}

fn cc(channel: u8, controller: u8, value: u8) -> Event {
    Event::ControlChange { channel, controller, value }
}

/// The events releasing every note of `channels`, as a sequencer hands
/// them out.
fn release_events(channels: u16) -> Vec<Event> {
    let mut out = Vec::new();
    super::release(channels, |e| out.push(e));
    out
}

fn is_release(e: &Event) -> bool {
    matches!(e, Event::ControlChange { controller: 64 | 66 | 123 | 121, value: 0, .. })
}

fn invalid(bytes: &[u8]) -> bool {
    matches!(Smf::from_bytes(bytes, LoopMarkers::None), Err(Error::Invalid(e)) if e.format() == crate::error::Format::Midi)
}

/// At 120 beats per minute and 480 ticks a beat, 960 ticks are a second.
fn one_note(delta: u64) -> Track {
    Track::new().note_on(delta, 0, 60, 100).note_off(480, 0, 60)
}

// P-MF-01

#[test]
fn same_tick_keeps_track_order_then_track_order() {
    let a = Track::new().cc(0, 0, 1, 1).cc(0, 0, 1, 2).cc(10, 0, 1, 5).end(0);
    let b = Track::new().cc(0, 1, 1, 3).cc(5, 1, 1, 4).cc(5, 1, 1, 6).end(0);
    let smf = load(&SmfBuilder::new().track(a).track(b), LoopMarkers::None);
    let values: Vec<u8> = events(&smf)
        .iter()
        .map(|e| match e {
            Event::ControlChange { value, .. } => *value,
            _ => panic!(),
        })
        .collect();
    assert_eq!(values, [1, 2, 3, 4, 5, 6]);
}

/// Go version's bug: a note on in an earlier track at the tick of the loop
/// start was before the loop, and went silent from the second time round.
#[test]
fn loop_start_marker_plays_notes_of_its_tick_in_earlier_tracks() {
    let notes = Track::new().note_on(960, 0, 60, 100).note_off(480, 0, 60).end(480);
    let marker = Track::new().cc(960, 0, 111, 0).end(0);
    let smf = load(&SmfBuilder::new().track(notes).track(marker), LoopMarkers::RpgMaker);
    let out = play(smf, 1000, true, 4500);
    assert_eq!(note_ons(&out), [(1000, 60), (2000, 60), (3000, 60), (4000, 60)]);
    // The marker is not an event.
    assert!(!out.iter().any(|(_, e)| matches!(e, Event::ControlChange { controller: 111, .. })));
}

/// A note on at the tick of the loop end is after the loop, whatever its
/// track: it never sounds while looping, and so is never cut at once.
#[test]
fn loop_end_marker_goes_before_notes_of_its_tick() {
    let notes = Track::new().note_on(0, 0, 60, 100).note_on(1920, 0, 62, 100).note_off(480, 0, 62).end(0);
    let markers = Track::new().cc(960, 0, 110, 0).cc(960, 0, 111, 0).end(0);
    let builder = SmfBuilder::new().track(notes).track(markers);
    let looped = play(load(&builder, LoopMarkers::IncredibleMachine), 1000, true, 10_000);
    assert_eq!(note_ons(&looped), [(0, 60)]);
    let once = play(load(&builder, LoopMarkers::IncredibleMachine), 1000, false, 10_000);
    assert_eq!(note_ons(&once), [(0, 60), (2000, 62)]);
    assert!(!once.iter().any(|(_, e)| matches!(e, Event::ControlChange { controller: 110 | 111, .. })));
}

// P-MF-02

#[test]
fn running_status_goes_on_after_meta_and_sysex() {
    let track = Track::new()
        .running_status(true)
        .note_on(0, 0, 60, 100)
        .meta(0, 0x01, b"x")
        .sysex(0, &[0x41, 0xF7])
        .note_on(96, 0, 60, 0)
        .end(0);
    assert_eq!(&track.bytes()[14..17], [0x60, 0x3C, 0x00], "the second note on has no status");
    let smf = load(&SmfBuilder::new().track(track), LoopMarkers::None);
    assert_eq!(
        events(&smf),
        [
            Event::NoteOn { channel: 0, key: 60, velocity: 100 },
            Event::SysEx(SysEx::new(&[0x41]).unwrap()),
            Event::NoteOn { channel: 0, key: 60, velocity: 0 },
        ]
    );
}

// P-MF-03

#[test]
fn track_without_end_ends_with_its_chunk() {
    let unended = Track::new().note_on(0, 0, 60, 100).note_off(1920, 0, 60);
    let smf = load(&SmfBuilder::new().track(unended).track(one_note(0)), LoopMarkers::None);
    assert_eq!(events(&smf).len(), 4);
    assert_eq!(smf.duration(), Duration::from_secs(2));
}

#[test]
fn events_after_end_of_track_are_ignored() {
    // Even ones that would not load.
    let junk = one_note(0).end(0).note_on(0, 0, 64, 100).raw(&[0x00, 0xF1, 0x00]);
    let smf = load(&SmfBuilder::new().track(junk), LoopMarkers::None);
    assert_eq!(events(&smf).len(), 2);
}

#[test]
fn end_of_track_sets_the_duration() {
    let smf = load(&SmfBuilder::new().track(one_note(0).end(0)).track(Track::new().end(2880)), LoopMarkers::None);
    assert_eq!(smf.duration(), Duration::from_secs(3));
}

// P-MF-04

#[test]
fn tempo_changes_set_the_time() {
    let track = Track::new().note_on(0, 0, 60, 100).tempo(480, 1_000_000).note_off(480, 0, 60).end(0);
    let smf = load(&SmfBuilder::new().track(track), LoopMarkers::None);
    assert_eq!(play(smf, 44100, false, u64::MAX)[1].0, 66150);
}

#[test]
fn tempo_of_zero_is_ignored() {
    let track = Track::new().tempo(0, 0).note_on(480, 0, 60, 100).end(0);
    let smf = load(&SmfBuilder::new().track(track), LoopMarkers::None);
    assert_eq!(smf.duration(), Duration::from_millis(500));
}

#[test]
fn tempo_events_of_any_length_but_three_are_ignored() {
    let track = Track::new().meta(0, 0x51, &[0x0F, 0x42]).meta(0, 0x51, &[0x0F, 0x42, 0x40, 0]).end(480);
    let smf = load(&SmfBuilder::new().track(track), LoopMarkers::None);
    assert_eq!(smf.duration(), Duration::from_millis(500));
}

#[test]
fn smpte_division_counts_frames_whatever_the_tempo() {
    let track = Track::new().tempo(0, 1_000_000).note_on(500, 0, 60, 100).end(0);
    let smf = load(&SmfBuilder::new().smpte(25, 40).track(track), LoopMarkers::None);
    assert_eq!(smf.duration(), Duration::from_millis(500));
    // 30 drop-frame is 30000 / 1001 frames a second.
    let smf = load(&SmfBuilder::new().smpte(29, 2).track(Track::new().end(60)), LoopMarkers::None);
    assert_eq!(smf.duration(), Duration::from_millis(1001));
    let smf = load(&SmfBuilder::new().smpte(24, 1).track(Track::new().note_on(1, 0, 60, 1).end(0)), LoopMarkers::None);
    assert_eq!(play(smf, 48000, false, u64::MAX)[0].0, 2000);
}

#[test]
fn invalid_divisions_fail() {
    let track = || one_note(0);
    assert!(invalid(&SmfBuilder::new().division(0).track(track()).build()));
    assert!(invalid(&SmfBuilder::new().smpte(25, 0).track(track()).build()));
    assert!(invalid(&SmfBuilder::new().smpte(23, 40).track(track()).build()));
    // -128 frames a second: negating it overflowed.
    assert!(invalid(&SmfBuilder::new().division(0x8028).track(track()).build()));
}

// P-MF-05

#[test]
fn system_common_and_realtime_statuses_fail() {
    for status in (0xF1..=0xFE).filter(|&s| s != 0xF7) {
        let track = Track::new().note_on(0, 0, 60, 100).raw(&[0x00, status, 0x00, 0x00]).end(0);
        let bytes = SmfBuilder::new().track(track).build();
        assert!(invalid(&bytes), "{status:#04X}");
    }
}

// P-MF-06

#[test]
fn files_over_a_day_fail() {
    // A tick a second.
    let at = |ticks| SmfBuilder::new().division(1).track(Track::new().tempo(0, 1_000_000).end(ticks)).build();
    let smf = Smf::from_bytes(&at(86_400), LoopMarkers::None).unwrap();
    assert_eq!(smf.duration(), Duration::from_secs(86_400));
    assert!(invalid(&at(86_401)));
    // The slowest tempo, in a note's delta.
    let slow = Track::new().tempo(0, 0xFF_FFFF).note_on(5150, 0, 60, 100).end(0);
    assert!(invalid(&SmfBuilder::new().division(1).track(slow).build()));
}

#[test]
fn ticks_add_up_without_overflowing() {
    let mut track = Track::new();
    for _ in 0..10_000 {
        track = track.note_on(0x0FFF_FFFF, 0, 60, 100);
    }
    assert!(invalid(&SmfBuilder::new().track(track).build()));
}

// P-MF-07

#[test]
fn many_empty_tracks_load_fast() {
    let mut builder = SmfBuilder::new();
    for _ in 0..65535 {
        builder = builder.track(Track::new().end(0));
    }
    let bytes = builder.build();
    let started = Instant::now();
    let smf = Smf::from_bytes(&bytes, LoopMarkers::None).unwrap();
    let took = started.elapsed();
    assert!(smf.events.is_empty());
    assert!(took < Duration::from_millis(100), "took {took:?}");
}

#[test]
fn many_tracks_of_notes_merge_in_order() {
    let mut builder = SmfBuilder::new();
    for i in 0..65535u64 {
        builder = builder.track(Track::new().note_on(i % 1000, 0, (i % 128) as u8, 1).end(0));
    }
    let bytes = builder.build();
    let started = Instant::now();
    let smf = Smf::from_bytes(&bytes, LoopMarkers::None).unwrap();
    let took = started.elapsed();
    assert_eq!(smf.events.len(), 65535);
    assert!(smf.events.windows(2).all(|w| w[0].time <= w[1].time));
    assert!(took < Duration::from_millis(500), "took {took:?}");
}

// P-MF-08

/// Go version's bug: the whole file looped back to its first event, not to
/// its start.
#[test]
fn whole_file_loop_goes_back_to_time_zero() {
    let smf = load(&SmfBuilder::new().track(one_note(480).end(960)), LoopMarkers::None);
    assert_eq!(smf.duration(), Duration::from_secs(2));
    let out = play(smf, 1000, true, 6600);
    assert_eq!(note_ons(&out), [(500, 60), (2500, 60), (4500, 60), (6500, 60)]);
}

#[test]
fn loop_without_start_marker_starts_at_zero() {
    let track = Track::new().note_on(480, 0, 60, 100).cc(960, 0, 117, 0).note_on(0, 0, 61, 100).end(960);
    let out = play(load(&SmfBuilder::new().track(track), LoopMarkers::FinalFantasy), 1000, true, 4000);
    assert_eq!(note_ons(&out), [(500, 60), (2000, 60), (3500, 60)]);
}

// P-MF-09

/// The frame of `ticks` at 120 beats per minute, 480 ticks a beat:
/// `ticks · 500000 µs / 480 · rate`, rounded.
fn frame_of_ticks(ticks: u128, rate: u128) -> u64 {
    let (num, den) = (ticks * 500_000 * rate, 480 * 1_000_000);
    ((2 * num + den) / (2 * den)) as u64
}

#[test]
fn loops_keep_time_exactly_over_many_times_round() {
    // A loop of 1000 ticks, 45937.5 frames at 44100 Hz.
    let track = Track::new().note_on(7, 0, 60, 100).note_off(500, 0, 60).end(493);
    let smf = load(&SmfBuilder::new().track(track), LoopMarkers::None);
    let rounds = 1000;
    let until = frame_of_ticks(1000 * rounds, 44100);
    let out = play(smf.clone(), 44100, true, until);
    let ons = note_ons(&out);
    assert_eq!(ons.len(), rounds as usize);
    for (k, &(frame, _)) in ons.iter().enumerate() {
        assert_eq!(frame, frame_of_ticks(7 + 1000 * k as u128, 44100), "round {k}");
    }
    let releases: Vec<u64> = out.iter().filter(|(_, e)| *e == cc(0, 123, 0)).map(|&(f, _)| f).collect();
    for (k, &frame) in releases.iter().enumerate() {
        assert_eq!(frame, frame_of_ticks(1000 * (k as u128 + 1), 44100));
    }

    // Asking for the frames in pieces of any size hands out the same.
    let mut seq = sequencer(Arc::new(smf), 44100);
    seq.set_looping(true);
    let mut pieces = Vec::new();
    let (mut frame, mut seed) = (0u64, 12345u64);
    while frame < until {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        frame = (frame + (seed >> 33) % 5000).min(until);
        seq.next_events(frame, |f, e| pieces.push((f, e)));
    }
    assert_eq!(pieces, out);
}

#[test]
fn going_round_releases_every_note_first() {
    let track = Track::new().cc(0, 0, 64, 127).note_on(0, 0, 60, 100).end(960);
    let out = play(load(&SmfBuilder::new().track(track), LoopMarkers::None), 1000, true, 1500);
    let wrap: Vec<Event> = out.iter().filter(|&&(f, _)| f == 1000).map(|&(_, e)| e).collect();
    let mut expected = release_events(1);
    expected.extend([cc(0, 64, 127), Event::NoteOn { channel: 0, key: 60, velocity: 100 }]);
    assert_eq!(wrap, expected);
    // Pedals go up before All Notes Off.
    assert_eq!(&expected[..4], [cc(0, 64, 0), cc(0, 66, 0), cc(0, 123, 0), cc(0, 121, 0)]);
}

#[test]
fn going_round_sends_the_state_at_the_loop_start() {
    let track = Track::new()
        .program(0, 2, 5)
        .cc(0, 2, 7, 90)
        .cc(960, 2, 111, 0)
        .note_on(0, 2, 60, 100)
        .cc(480, 2, 7, 30)
        .program(0, 2, 9)
        .end(480);
    let out = play(load(&SmfBuilder::new().track(track), LoopMarkers::RpgMaker), 1000, true, 2500);
    let wrap: Vec<Event> = out.iter().filter(|&&(f, e)| f == 2000 && !is_release(&e)).map(|&(_, e)| e).collect();
    assert_eq!(
        wrap,
        [
            Event::ProgramChange { channel: 2, program: 5 },
            cc(2, 7, 90),
            Event::NoteOn { channel: 2, key: 60, velocity: 100 }
        ]
    );
}

/// A loop of no length cannot keep a call going.
#[test]
fn loops_shorter_than_a_millisecond_are_no_loops() {
    let track = Track::new().note_on(0, 0, 60, 100).cc(480, 0, 116, 0).cc(0, 0, 117, 0).end(480);
    let out = play(load(&SmfBuilder::new().track(track), LoopMarkers::FinalFantasy), 1000, true, 2500);
    // The markers are ignored: the whole file loops.
    assert_eq!(note_ons(&out), [(0, 60), (1000, 60), (2000, 60)]);

    let instant = Track::new().note_on(0, 0, 60, 100).note_off(0, 0, 60).end(0);
    let smf = load(&SmfBuilder::new().track(instant), LoopMarkers::None);
    assert!(smf.looped.is_none());
    let mut seq = sequencer(Arc::new(smf), 48000);
    seq.set_looping(true);
    let mut count = 0;
    seq.next_events(u64::MAX, |_, _| count += 1);
    assert_eq!(count, 2);
    assert!(seq.finished());
}

#[test]
fn looping_turned_on_past_the_loop_goes_back_at_once() {
    let smf = load(&SmfBuilder::new().track(one_note(0).end(480)), LoopMarkers::None);
    let mut seq = sequencer(Arc::new(smf), 1000);
    let mut out = Vec::new();
    seq.next_events(1500, |f, e| out.push((f, e)));
    assert!(seq.finished());
    seq.set_looping(true);
    assert!(!seq.finished());
    seq.next_events(4000, |f, e| out.push((f, e)));
    assert_eq!(note_ons(&out), [(0, 60), (1500, 60), (2500, 60), (3500, 60)]);
}

// Seeking.

fn state_file() -> SmfBuilder {
    let track = Track::new()
        .cc(0, 1, 0, 1)
        .program(0, 1, 5)
        .cc(0, 1, 0, 2)
        .cc(0, 1, 7, 100)
        .cc(0, 1, 101, 0)
        .cc(0, 1, 100, 0)
        .cc(0, 1, 6, 12)
        .note_on(0, 1, 60, 100)
        .pitch_bend(100, 1, 9000)
        .channel_pressure(0, 1, 50)
        .poly_pressure(0, 1, 60, 40)
        .note_off(860, 1, 60)
        .cc(0, 1, 7, 80)
        .note_on(480, 1, 62, 100)
        .end(480);
    SmfBuilder::new().track(track)
}

#[test]
fn seek_sends_the_state_and_no_notes() {
    let mut seq = sequencer(Arc::new(load(&state_file(), LoopMarkers::None)), 1000);
    let mut out = Vec::new();
    seq.next_events(300, |f, e| out.push((f, e)));
    seq.seek(Duration::from_millis(1200));
    assert_eq!(seq.position(), Duration::from_millis(1200));
    out.clear();
    seq.next_events(1000, |f, e| out.push((f, e)));
    let mut expected: Vec<(u64, Event)> = release_events(1 << 1).into_iter().map(|e| (300, e)).collect();
    expected.extend(
        [
            // Bank selects, programs and parameters in their order.
            cc(1, 0, 1),
            Event::ProgramChange { channel: 1, program: 5 },
            cc(1, 0, 2),
            cc(1, 101, 0),
            cc(1, 100, 0),
            cc(1, 6, 12),
            // Then the rest.
            cc(1, 7, 80),
            Event::PitchBend { channel: 1, value: 9000 },
            Event::ChannelPressure { channel: 1, pressure: 50 },
            Event::PolyPressure { channel: 1, key: 60, pressure: 40 },
        ]
        .map(|e| (300, e)),
    );
    expected.push((600, Event::NoteOn { channel: 1, key: 62, velocity: 100 }));
    assert_eq!(out, expected);
    assert_eq!(seq.position(), Duration::from_millis(1900));
}

#[test]
fn seek_forgets_what_a_reset_undid() {
    let gs_reset = [0x41, 0x10, 0x42, 0x12, 0x40, 0x00, 0x7F, 0x00, 0x41, 0xF7];
    let track = Track::new().cc(0, 0, 7, 50).sysex(0, &gs_reset).cc(0, 0, 10, 20).end(960);
    let mut seq = sequencer(Arc::new(load(&SmfBuilder::new().track(track), LoopMarkers::None)), 1000);
    seq.seek(Duration::from_millis(500));
    let mut out = Vec::new();
    seq.next_events(1, |_, e| out.push(e));
    let state: Vec<Event> = out.into_iter().filter(|e| !is_release(e)).collect();
    assert_eq!(state, [Event::SysEx(SysEx::new(&gs_reset[..9]).unwrap()), cc(0, 10, 20)]);
}

#[test]
fn seek_into_the_loop_and_past_it() {
    // The loop is from 1 s to 2 s; the file goes on to 3 s.
    let track = Track::new()
        .note_on(0, 0, 60, 100)
        .cc(960, 0, 116, 0)
        .note_on(480, 0, 61, 100)
        .cc(480, 0, 117, 0)
        .note_on(480, 0, 62, 100)
        .end(480);
    let smf = Arc::new(load(&SmfBuilder::new().track(track), LoopMarkers::FinalFantasy));

    // Within the loop, it goes round from there.
    let mut seq = sequencer(smf.clone(), 1000);
    seq.set_looping(true);
    seq.seek(Duration::from_millis(1250));
    let mut out = Vec::new();
    seq.next_events(3000, |f, e| out.push((f, e)));
    assert_eq!(note_ons(&out), [(250, 61), (1250, 61), (2250, 61)]);
    assert_eq!(seq.position(), Duration::from_millis(1250));

    // Past the loop's end, where going round would be.
    let mut seq = sequencer(smf.clone(), 1000);
    seq.set_looping(true);
    seq.seek(Duration::from_millis(3700));
    assert_eq!(seq.position(), Duration::from_millis(1700));
    let mut out = Vec::new();
    seq.next_events(1000, |f, e| out.push((f, e)));
    assert_eq!(note_ons(&out), [(800, 61)]);

    // Not looping, past the loop's end is past it.
    let mut seq = sequencer(smf, 1000);
    seq.seek(Duration::from_millis(2200));
    let mut out = Vec::new();
    seq.next_events(1000, |f, e| out.push((f, e)));
    assert_eq!(note_ons(&out), [(300, 62)]);
    assert!(seq.finished());
    seq.seek(Duration::from_secs(100));
    assert_eq!(seq.position(), Duration::from_secs(3));
}

#[test]
fn finished_once_played_to_the_end() {
    let mut seq = sequencer(Arc::new(load(&SmfBuilder::new().track(one_note(0).end(960)), LoopMarkers::None)), 1000);
    seq.next_events(1000, |_, _| {});
    assert!(!seq.finished(), "every event is out, but the file goes on");
    seq.next_events(1500, |_, _| {});
    assert!(seq.finished());
    assert_eq!(seq.position(), Duration::from_millis(1500));
}

// Loading.

#[test]
fn format_0_loads_and_format_2_is_unsupported() {
    assert!(Smf::from_bytes(&SmfBuilder::new().format(0).track(one_note(0)).build(), LoopMarkers::None).is_ok());
    let format2 = SmfBuilder::new().format(2).track(one_note(0)).build();
    assert!(matches!(
        Smf::from_bytes(&format2, LoopMarkers::None),
        Err(Error::Unsupported(crate::error::Unsupported::MidiFormat2))
    ));
    assert!(invalid(&SmfBuilder::new().format(3).track(one_note(0)).build()));
}

#[test]
fn headers_and_chunks_out_of_the_ordinary() {
    // A longer header, a chunk of another type, fewer tracks than said.
    let builder = SmfBuilder::new().header_size(10).chunk(*b"XFIH", vec![1, 2, 3]).track(one_note(0)).track_count(3);
    assert_eq!(events(&load(&builder, LoopMarkers::None)).len(), 2);
    assert!(invalid(&SmfBuilder::new().track_count(0).build()));
    assert!(invalid(&SmfBuilder::new().track_count(1).build()));
    assert!(invalid(&SmfBuilder::new().header_size(5).track(one_note(0)).build()));
    assert!(invalid(b"RIFF\0\0\0\x06\0\x01\0\x01\x01\xE0"));
}

#[test]
fn malformed_events_fail() {
    let fails = |track: Track| invalid(&SmfBuilder::new().track(track).build());
    assert!(fails(Track::new().raw(&[0x00, 0x3C, 0x64])), "data with no running status");
    assert!(fails(Track::new().raw(&[0xFF, 0xFF, 0xFF, 0xFF, 0x7F, 0x90, 0x3C, 0x64])), "a delta of five bytes");
    assert!(fails(Track::new().raw(&[0x00, 0x90, 0x3C, 0xE4])), "a data byte over 127");
    assert!(fails(Track::new().raw(&[0x00, 0x90, 0x3C])), "an event cut short");
    assert!(fails(Track::new().raw(&[0x00, 0xF0, 0xFF, 0xFF, 0xFF, 0x7F])), "a length past the end");
    assert!(fails(Track::new().raw(&[0x00, 0xFF, 0x01])), "a meta event cut short");
}

#[test]
fn a_chunk_shorter_than_it_says_plays_what_it_has() {
    let builder = SmfBuilder::new().track_sized(one_note(0).raw(&[]), 1 << 30);
    assert_eq!(events(&load(&builder, LoopMarkers::None)).len(), 2);
}

#[test]
fn sysex_events() {
    let long = [0x7D; 40];
    let track = Track::new()
        .sysex(0, &[0x7E, 0x7F, 0x09, 0x01, 0xF7])
        .sysex(0, &[long.as_slice(), &[0xF7]].concat())
        .sysex(0, &[0x41, 0x10])
        .sysex_f7(10, &[0x42, 0x12])
        .sysex_f7(10, &[0x40, 0xF7])
        .sysex_f7(0, &[0xF0, 0x43, 0xF7])
        .sysex_f7(0, &[0xF8])
        .end(0);
    let smf = load(&SmfBuilder::new().track(track), LoopMarkers::None);
    let sysex = |data: &[u8]| Event::SysEx(SysEx::new(data).unwrap());
    assert_eq!(
        events(&smf),
        [sysex(&[0x7E, 0x7F, 0x09, 0x01]), sysex(&[0x41, 0x10, 0x42, 0x12, 0x40]), sysex(&[0x43])]
    );
    // A divided message is an event when it ends.
    assert_eq!(smf.events[1].time, 20 * 500_000);
}

/// A reader giving a byte at a time.
struct OneByte<'a>(&'a [u8]);

impl Read for OneByte<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.0.is_empty() || buf.is_empty() {
            return Ok(0);
        }
        buf[0] = self.0[0];
        self.0 = &self.0[1..];
        Ok(1)
    }
}

#[test]
fn short_reads_load_the_same() {
    let bytes = state_file().track(one_note(5)).build();
    let whole = Smf::from_bytes(&bytes, LoopMarkers::None).unwrap();
    let short = Smf::read(OneByte(&bytes), LoopMarkers::None).unwrap();
    assert_eq!(events(&whole), events(&short));
    assert_eq!(whole.duration(), short.duration());
}

#[test]
fn any_cut_or_changed_byte_loads_or_fails_without_panicking() {
    let bytes = state_file().track(one_note(5).sysex(0, &[1, 2, 0xF7])).build();
    for len in 0..bytes.len() {
        let _ = Smf::from_bytes(&bytes[..len], LoopMarkers::RpgMaker);
    }
    for i in 0..bytes.len() {
        for b in [0x00, 0x7F, 0x80, 0xF0, 0xF7, 0xFF] {
            let mut changed = bytes.clone();
            changed[i] = b;
            if let Ok(smf) = Smf::from_bytes(&changed, LoopMarkers::FinalFantasy) {
                let mut seq = sequencer(Arc::new(smf), 44100);
                seq.set_looping(true);
                seq.next_events(441_000, |_, _| {});
            }
        }
    }
}

// Sequencer::render

/// A file of bursts: 300 controller changes and 40 notes at each of a few
/// ticks, so that a small queue fills many times over within one frame.
fn bursts() -> Arc<Smf> {
    let mut t = Track::new();
    for burst in 0..4u8 {
        let delta = if burst == 0 { 0 } else { 97 };
        t = t.cc(delta, 0, 7, 0);
        for i in 0..300u32 {
            t = t.cc(0, (i % 16) as u8, [7, 10, 1, 11][i as usize % 4], (i * 7 % 128) as u8);
        }
        for k in 0..40u8 {
            t = t.note_on(0, k % 16, 40 + k + burst, 100);
        }
    }
    let smf = load(&SmfBuilder::new().track(t.end(960)), LoopMarkers::None);
    Arc::new(smf)
}

fn synth(capacity: usize) -> crate::Synth {
    use crate::testkit::sf2::{SampleSpec, Sf2, Zone, sine};
    let sf = Sf2::single(SampleSpec::new("s", sine(4000, 37.3, 20000)).looped(10, 3739), Zone::new()).load();
    let mut c = crate::synth::Config::new(44100);
    c.event_capacity = capacity;
    crate::Synth::new(Arc::new(sf), c).unwrap()
}

fn render_all(seq: &mut Sequencer, synth: &mut crate::Synth, chunks: &[usize], frames: usize) -> Vec<f32> {
    let mut out = Vec::new();
    let (mut k, mut done) = (0, 0);
    while done < frames {
        let n = chunks[k % chunks.len()].min(frames - done);
        done += n;
        let (mut l, mut r) = (vec![0.0; n], vec![0.0; n]);
        assert_eq!(seq.render(synth, &mut l, &mut r), n);
        out.extend(l.iter().zip(&r).flat_map(|(a, b)| [*a, *b]));
        k += 1;
    }
    out
}

/// A queue far smaller than the bursts plays every event at its frame: the
/// output is the same as through a queue that holds them all.
#[test]
fn render_plays_bursts_on_time_through_a_small_queue() {
    let frames = 44100;
    let reference = {
        let mut seq = sequencer(bursts(), 44100);
        render_all(&mut seq, &mut synth(65536), &[frames], frames)
    };
    assert!(reference.iter().any(|x| *x != 0.0));
    for chunks in [&[frames][..], &[64], &[1, 999, 33]] {
        let mut seq = sequencer(bursts(), 44100);
        let mut s = synth(16);
        assert_eq!(render_all(&mut seq, &mut s, chunks, frames), reference, "{chunks:?}");
        assert!(seq.finished() || seq.frame >= frames as u64);
    }
}

/// When the queue stays full of someone else's events for later frames, the
/// sequencer's events play late rather than not at all.
#[test]
fn render_plays_late_when_the_queue_is_kept_full() {
    let smf = Arc::new(load(&SmfBuilder::new().track(one_note(10)), LoopMarkers::None));
    let mut seq = sequencer(smf, 44100);
    let mut s = synth(16);
    for _ in 0..16 {
        s.schedule(1000, cc(1, 1, 0)).unwrap();
    }
    let (mut l, mut r) = (vec![0.0; 2000], vec![0.0; 2000]);
    assert_eq!(seq.render(&mut s, &mut l, &mut r), 2000);
    assert!(s.active_voices() > 0);
}

/// `{:?}` of a file and of a sequencer is a summary, not the events; of a
/// system exclusive message, its bytes.
#[test]
fn debug_is_a_summary() {
    let mut t = Track::new();
    for _ in 0..1000 {
        t = t.note_on(10, 0, 60, 100).note_off(10, 0, 60);
    }
    let smf = load(&SmfBuilder::new().track(t.end(0)), LoopMarkers::None);
    let text = format!("{smf:?}");
    assert!(text.contains("events: 2000") && text.len() < 200, "{text}");
    let seq = Sequencer::with_sample_rate(Arc::new(smf), 44100).unwrap();
    let text = format!("{seq:?}");
    assert!(text.contains("finished: false") && text.len() < 400, "{text}");
    assert_eq!(format!("{:?}", SysEx::new(&[0x41, 0x10]).unwrap()), "SysEx([65, 16])");
}

// Sample rates.

#[test]
fn sample_rates_are_a_synthesizers() {
    let smf = Arc::new(load(&SmfBuilder::new().track(one_note(960)), LoopMarkers::None));
    for rate in [0, 1, 7999, 192_001] {
        assert!(
            matches!(
                Sequencer::with_sample_rate(smf.clone(), rate),
                Err(Error::Config(crate::error::Setting::SampleRate))
            ),
            "{rate}"
        );
    }
    for rate in [8000, 192_000] {
        assert_eq!(Sequencer::with_sample_rate(smf.clone(), rate).unwrap().sample_rate(), rate);
    }
    assert_eq!(Sequencer::new(smf, &synth(16)).sample_rate(), 44100);
}

/// A sequencer made for another rate plays at the synthesizer's, from the
/// time it has played.
#[test]
fn render_plays_at_the_synthesizers_rate() {
    let smf = Arc::new(load(&SmfBuilder::new().track(one_note(960).end(0)), LoopMarkers::None));
    let mut seq = Sequencer::with_sample_rate(smf, 48000).unwrap();
    let mut s = synth(16);
    render_all(&mut seq, &mut s, &[4096], 44100);
    assert_eq!(seq.sample_rate(), 44100);
    assert_eq!(s.active_voices(), 0);
    assert_eq!(seq.position(), Duration::from_secs(1));
    render_all(&mut seq, &mut s, &[1], 1);
    assert_eq!(s.active_voices(), 1, "the note at 1 s plays at frame 44100");
}

// Jumps reach the state played from the start.

/// 10 frames a tick, at 960 ticks a second.
const RATE: u32 = 9600;

/// The time of `ticks`, at 960 ticks a second.
fn at_ticks(ticks: u64) -> Duration {
    Duration::from_nanos((ticks * 1_000_000_000 + 480) / 960)
}

/// Presets for the banks and programs the files choose.
fn chase_font() -> Arc<crate::SoundFont> {
    use crate::soundfont::GeneratorType as G;
    use crate::testkit::sf2::{SampleSpec, Sf2, Zone};
    let mut b = Sf2::new();
    let s = b.sample(SampleSpec::new("dc", vec![16384; 2000]).looped(100, 1900));
    let i = b.instrument("i", [Zone::new().sample(s).set(G::SampleModes, 1)]);
    for (bank, program) in [(0, 0), (0, 5), (0, 9), (1, 0), (5, 5), (120, 0), (128, 0), (128, 5), (129, 9)] {
        b.preset(&format!("{bank}:{program}"), bank, program, [Zone::new().instrument(i)]);
    }
    Arc::new(b.load())
}

fn chase_synth() -> crate::Synth {
    let mut c = crate::synth::Config::new(RATE);
    c.effects = false;
    c.max_voices = 16;
    crate::Synth::new(chase_font(), c).unwrap()
}

fn render_frames(seq: &mut Sequencer, synth: &mut crate::Synth, frames: u64) {
    let (mut l, mut r) = (vec![0.0; 4096], vec![0.0; 4096]);
    let mut left = frames;
    while left > 0 {
        let n = left.min(4096) as usize;
        seq.render(synth, &mut l[..n], &mut r[..n]);
        left -= n as u64;
    }
}

struct Lcg(u64);

impl Lcg {
    fn below(&mut self, n: u32) -> u32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 33) % n as u64) as u32
    }

    fn pick<T: Copy>(&mut self, xs: &[T]) -> T {
        xs[self.below(xs.len() as u32) as usize]
    }
}

/// A GS data set with its checksum, as a track's sysex event.
fn gs_bytes(address: [u8; 3], data: u8) -> Vec<u8> {
    let sum = address.iter().map(|b| u32::from(*b)).sum::<u32>() + u32::from(data);
    let checksum = ((0x80 - (sum & 0x7F)) & 0x7F) as u8;
    vec![0x41, 0x10, 0x42, 0x12, address[0], address[1], address[2], data, checksum, 0xF7]
}

fn random_sysex(r: &mut Lcg) -> Vec<u8> {
    match r.below(10) {
        0 => gs_bytes([0x40, 0x00, 0x7F], 0x00),
        1 => vec![0x7E, 0x7F, 0x09, 0x01, 0xF7],
        2 => vec![0x7E, 0x7F, 0x09, 0x03, 0xF7],
        // GM System Off, XG System On, "exit GS mode", a wrong checksum.
        3 => vec![0x7E, 0x7F, 0x09, 0x02, 0xF7],
        4 => vec![0x43, 0x10, 0x4C, 0x00, 0x00, 0x7E, 0x00, 0xF7],
        5 => gs_bytes([0x40, 0x00, 0x7F], 0x7F),
        6 => vec![0x41, 0x10, 0x42, 0x12, 0x40, 0x11, 0x15, 0x01, 0x00, 0xF7],
        // Use for rhythm part, for parts playing channels 9, 0, 1 and 10.
        _ => gs_bytes([0x40, 0x10 | r.pick(&[0, 1, 2, 0xA]), 0x15], r.pick(&[0, 1, 2])),
    }
}

/// A controller and a value, the values being those that do something.
fn random_cc(r: &mut Lcg) -> (u8, u8) {
    let controller =
        r.pick(&[0, 0, 32, 1, 2, 4, 7, 10, 11, 64, 66, 71, 76, 91, 6, 38, 98, 99, 100, 101, 121, 120, 123]);
    let value = match controller {
        0 => r.pick(&[0, 1, 5, 120, 121, 127]),
        32 => r.pick(&[0, 1, 5]),
        99 => r.pick(&[120, 120, 1, 127, 3]),
        98 => r.pick(&[52, 48, 8, 9, 10, 100, 101, 102, 127, 3]),
        101 => r.pick(&[0, 0, 127, 1]),
        100 => r.pick(&[0, 1, 2, 5, 127]),
        120 | 121 | 123 => 0,
        _ => r.below(128) as u8,
    };
    (controller, value)
}

/// `count` random events on a few channels: controllers, programs, bank
/// selects, RPNs and NRPNs alone and in groups, pitch bend, pressures,
/// notes, CC121, resets and rhythm parts.
fn random_track(seed: u64, count: usize) -> Track {
    random_events(Track::new(), seed, count)
}

fn random_events(mut t: Track, seed: u64, count: usize) -> Track {
    let mut r = Lcg(seed);
    for _ in 0..count {
        let delta = r.pick(&[0, 0, 1, 3, 20]);
        let ch = r.pick(&[0, 1, 9, 10]);
        t = match r.below(22) {
            0..=7 => {
                let (controller, value) = random_cc(&mut r);
                t.cc(delta, ch, controller, value)
            }
            8 => t.program(delta, ch, r.pick(&[0, 5, 9])),
            9 => t.pitch_bend(delta, ch, r.below(16384) as u16),
            10 => t.channel_pressure(delta, ch, r.below(128) as u8),
            11 => t.poly_pressure(delta, ch, r.pick(&[60, 61]), r.below(128) as u8),
            12 | 13 => t.note_on(delta, ch, 60 + r.below(3) as u8, 100),
            14 => t.note_off(delta, ch, 60 + r.below(3) as u8),
            15 | 16 => t.sysex(delta, &random_sysex(&mut r)),
            _ => {
                // A parameter: selected, then its data, in either order.
                let select: &[(u8, u8)] = match r.below(4) {
                    0 => &[(99, 120), (98, r.pick(&[52, 48, 100]))],
                    1 => &[(99, 120), (98, 100), (98, 8)],
                    2 => &[(101, 0), (100, r.pick(&[0, 1, 2, 5]))],
                    _ => &[(99, 1), (98, r.pick(&[8, 9, 32]))],
                };
                let mut t = t.cc(delta, ch, select[0].0, select[0].1);
                for &(controller, value) in &select[1..] {
                    t = t.cc(0, ch, controller, value);
                }
                let (msb, lsb) = (r.below(128) as u8, r.below(128) as u8);
                match r.below(3) {
                    0 => t.cc(0, ch, 38, lsb).cc(0, ch, 6, msb),
                    1 => t.cc(0, ch, 6, msb).cc(0, ch, 38, lsb),
                    _ => t.cc(0, ch, 6, msb),
                }
            }
        };
    }
    t
}

/// The ticks of a track's events, and where it ends.
fn ticks_of(smf: &Smf) -> u64 {
    smf.end / 500_000
}

/// The ticks of the reset messages of `smf`.
fn reset_ticks(smf: &Smf) -> Vec<u64> {
    let resets = smf.events.iter().filter(|e| {
        e.status == 0xF0
            && matches!(
                crate::synth::classify_sysex(smf.sysex[e.sysex as usize].data()),
                crate::synth::SysExKind::Reset(_)
            )
    });
    resets.map(|e| e.time / 500_000).collect()
}

/// The channel the random files leave alone, for the host.
const HOST: u8 = 15;

/// What a host plays on its own channel, `HOST`, while a file plays on the
/// others: a few random changes to its state.
fn host_changes(synth: &mut crate::Synth, r: &mut Lcg) {
    for _ in 0..4 {
        let event = match r.below(5) {
            0 => Event::ProgramChange { channel: HOST, program: r.pick(&[5, 9]) },
            1 => Event::PitchBend { channel: HOST, value: r.below(16384) as u16 },
            2 => cc(HOST, 0, 1),
            _ => {
                let (controller, value) = random_cc(r);
                cc(HOST, controller, value)
            }
        };
        synth.send(event);
    }
}

/// The channels' state but the host's, and the host's.
fn split_host<T>(states: (BankStyle, Vec<T>)) -> ((BankStyle, Vec<T>), T) {
    let (style, mut channels) = states;
    let host = channels.remove(HOST as usize);
    ((style, channels), host)
}

/// Seeking to a time gives the synthesizer's channels the state they have
/// when played from the start to there, whatever played before the seek;
/// the channel the file does not use keeps what the host set on it.
#[test]
fn seeking_reaches_the_state_played_from_the_start() {
    for seed in 0..160 {
        let smf = Arc::new(load(&SmfBuilder::new().track(random_track(seed, 160).end(5)), LoopMarkers::None));
        assert_eq!(smf.channels & 1 << HOST, 0);
        let end = ticks_of(&smf);
        let mut r = Lcg(seed + 1000);
        let mut targets: Vec<u64> = (0..10).map(|_| r.below(end as u32 + 1) as u64).collect();
        // The ticks of reset messages, which a seek there does not play.
        targets.extend(reset_ticks(&smf));
        targets.extend([0, end]);
        targets.sort();
        targets.dedup();

        // From the start, stopping at each target: past its events.
        let mut synth = chase_synth();
        let mut seq = Sequencer::new(smf.clone(), &synth);
        let mut played = Vec::new();
        let mut frame = 0;
        for &k in &targets {
            render_frames(&mut seq, &mut synth, 10 * k + 1 - frame);
            frame = 10 * k + 1;
            played.push(synth.channel_states());
        }

        // Seeking, in another order, after playing it all.
        let mut synth = chase_synth();
        let mut seq = Sequencer::new(smf.clone(), &synth);
        render_frames(&mut seq, &mut synth, 10 * end + 100);
        let mut order: Vec<usize> = (0..targets.len()).collect();
        for i in (1..order.len()).rev() {
            order.swap(i, r.below(i as u32 + 1) as usize);
        }
        for i in order {
            host_changes(&mut synth, &mut r);
            let host = split_host(synth.channel_states()).1;
            seq.seek(at_ticks(targets[i]));
            render_frames(&mut seq, &mut synth, 1);
            let (file, host_after) = split_host(synth.channel_states());
            assert_eq!(file, split_host(played[i].clone()).0, "seed {seed}, seek to tick {}", targets[i]);
            assert_eq!(host_after, host, "seed {seed}, seek to tick {}: the host's channel", targets[i]);
        }
    }
}

/// Going round a loop, the channels have the state they had the first time
/// round, whatever the loop changed: with markers and without, with a reset
/// message at the loop's start or not. The channel the file does not use
/// keeps what the host set on it.
#[test]
fn going_round_reaches_the_state_of_the_first_time_round() {
    for seed in 0..160 {
        // Half the files have a random system exclusive message (a reset four
        // times in ten) at the start of the loop, between other events of its
        // tick.
        let at_start = |t: Track| {
            if seed % 4 < 2 {
                let mut r = Lcg(seed + 3000);
                t.cc(0, 0, 7, 20).program(0, 1, 5).sysex(0, &random_sysex(&mut r)).cc(0, 0, 10, 30).program(0, 9, 9)
            } else {
                t
            }
        };
        let mut track = random_track(seed, 120);
        let markers = if seed % 2 == 0 {
            track = at_start(track.cc(1, 0, 111, 0));
            LoopMarkers::RpgMaker
        } else {
            track = random_events(at_start(Track::new()), seed, 120);
            LoopMarkers::None
        };
        let track = random_events(track, seed + 500, 80);
        let smf = Arc::new(load(&SmfBuilder::new().track(track.end(5)), markers));
        assert_eq!(smf.channels & 1 << HOST, 0);
        let l = smf.looped.unwrap();
        let (start, end) = (l.start / 500_000, l.end / 500_000);
        let mut r = Lcg(seed + 2000);
        let mut points: Vec<u64> = (0..6).map(|_| start + r.below((end - start) as u32) as u64).collect();
        points.push(start);
        points.sort();
        points.dedup();

        let mut synth = chase_synth();
        let mut seq = Sequencer::new(smf.clone(), &synth);
        seq.set_looping(true);
        let mut frame = 0;
        let mut first = Vec::new();
        for round in 0..3 {
            for (i, &k) in points.iter().enumerate() {
                let to = 10 * (k + round * (end - start)) + 1;
                if round == 0 {
                    render_frames(&mut seq, &mut synth, to - frame);
                    first.push(split_host(synth.channel_states()).0);
                } else if i == 0 {
                    // Up to going round, where the host changes its channel,
                    // and then the frame of going round.
                    render_frames(&mut seq, &mut synth, to - 1 - frame);
                    host_changes(&mut synth, &mut r);
                    let host = split_host(synth.channel_states()).1;
                    render_frames(&mut seq, &mut synth, 1);
                    let (file, host_after) = split_host(synth.channel_states());
                    assert_eq!(file, first[i], "seed {seed}, round {round}, tick {k}");
                    assert_eq!(host_after, host, "seed {seed}, round {round}: the host's channel");
                } else {
                    render_frames(&mut seq, &mut synth, to - frame);
                    assert_eq!(split_host(synth.channel_states()).0, first[i], "seed {seed}, round {round}, tick {k}");
                }
                frame = to;
            }
        }
    }
}

/// A file whose loop turns the volume down, pans left, changes the program
/// and the bend range after its first note.
fn changes_in_the_loop() -> Arc<Smf> {
    let track = Track::new()
        .note_on(0, 0, 60, 100)
        .note_off(96, 0, 60)
        .cc(864, 0, 7, 10)
        .cc(0, 0, 10, 0)
        .program(0, 0, 5)
        .cc(0, 0, 101, 0)
        .cc(0, 0, 100, 0)
        .cc(0, 0, 6, 12)
        .end(1920);
    Arc::new(load(&SmfBuilder::new().track(track), LoopMarkers::None))
}

/// Go version's and the first version's bug: what the loop set after its
/// start stayed set the next time round.
#[test]
fn going_round_undoes_what_the_loop_set() {
    let smf = changes_in_the_loop();
    let mut synth = chase_synth();
    let mut seq = Sequencer::new(smf, &synth);
    seq.set_looping(true);
    let round = 3 * RATE as usize;
    let (mut l, mut r) = (vec![0.0; round], vec![0.0; round]);
    seq.render(&mut synth, &mut l[..1], &mut r[..1]);
    let first = synth.channel_states();
    seq.render(&mut synth, &mut l[1..], &mut r[1..]);
    let (mut l2, mut r2) = (vec![0.0; round], vec![0.0; round]);
    seq.render(&mut synth, &mut l2[..1], &mut r2[..1]);
    assert_eq!(synth.channel_states(), first);
    let c = &first.1[0];
    assert_eq!((c.cc[7], c.cc[10], c.program, c.bend_range), (100, 64, 0, 2.0));
    seq.render(&mut synth, &mut l2[1..], &mut r2[1..]);
    // The note sounds the same each time round.
    let note = 50..RATE as usize / 10;
    assert!(l[note.clone()].iter().any(|x| *x != 0.0));
    assert_eq!(l[note.clone()], l2[note.clone()]);
    assert_eq!(r[note.clone()], r2[note]);
}

#[test]
fn seeking_back_undoes_what_was_set_after() {
    let smf = changes_in_the_loop();
    let mut synth = chase_synth();
    let mut seq = Sequencer::new(smf, &synth);
    render_frames(&mut seq, &mut synth, 1);
    let start = synth.channel_states();
    render_frames(&mut seq, &mut synth, 2 * RATE as u64);
    assert_ne!(synth.channel_states(), start);
    seq.seek(Duration::ZERO);
    render_frames(&mut seq, &mut synth, 1);
    assert_eq!(synth.channel_states(), start);
}

/// The state handed out by a seek to `time`, less the release.
fn seek_state(smf: Smf, time: Duration) -> Vec<Event> {
    let released = release_events(smf.channels).len();
    let mut seq = sequencer(Arc::new(smf), 1000);
    seq.seek(time);
    let mut out = Vec::new();
    seq.next_events(1, |_, e| out.push(e));
    out.drain(..released);
    out
}

/// The first version's bug: a seek sent a SoundFont NRPN's MSB before its
/// LSB, which the NRPN selection had cleared, so the low bits were lost.
#[test]
fn seeking_keeps_the_low_bits_of_soundfont_nrpns() {
    // Fine tune (52) by +100 cents: 8292 is MSB 64, LSB 100.
    let track = Track::new()
        .cc(0, 0, 99, 120)
        .cc(0, 0, 98, 52)
        .cc(0, 0, 38, 100)
        .cc(0, 0, 6, 64)
        .note_on(960, 0, 60, 100)
        .note_off(960, 0, 60)
        .end(960);
    let builder = SmfBuilder::new().track(track);
    assert_eq!(
        seek_state(load(&builder, LoopMarkers::None), Duration::from_millis(500)),
        [cc(0, 99, 120), cc(0, 98, 52), cc(0, 38, 100), cc(0, 6, 64)]
    );
    let smf = Arc::new(load(&builder, LoopMarkers::None));
    let mut synth = chase_synth();
    let mut seq = Sequencer::new(smf.clone(), &synth);
    render_frames(&mut seq, &mut synth, RATE as u64 / 2 + 1);
    let played = synth.channel_states();
    assert_eq!(played.1[0].gen_offsets[52], 100.0);
    let mut synth = chase_synth();
    let mut seq = Sequencer::new(smf, &synth);
    seq.seek(Duration::from_millis(500));
    render_frames(&mut seq, &mut synth, 1);
    assert_eq!(synth.channel_states(), played);
}

/// The first version took CC121 to reset only what GM recommends; the
/// synthesizer resets all but what RP-015 keeps. A seek does not send
/// back what CC121 reset.
#[test]
fn the_chase_resets_all_controllers_as_the_synthesizer_does() {
    let track = Track::new().cc(0, 0, 2, 5).cc(0, 0, 4, 9).cc(0, 0, 7, 50).cc(0, 0, 91, 30).cc(0, 0, 121, 0).end(960);
    let smf = load(&SmfBuilder::new().track(track), LoopMarkers::None);
    assert_eq!(seek_state(smf, Duration::from_millis(500)), [cc(0, 121, 0), cc(0, 7, 50), cc(0, 91, 30)]);
}

/// XG System On does nothing to the synthesizer, so nothing to a seek;
/// "exit GS mode" resets as the synthesizer does.
#[test]
fn the_chase_judges_system_exclusive_messages_as_the_synthesizer_does() {
    let xg = [0x43, 0x10, 0x4C, 0x00, 0x00, 0x7E, 0x00, 0xF7];
    let track = Track::new().cc(0, 0, 7, 50).sysex(0, &xg).cc(0, 0, 10, 20).end(960);
    let smf = load(&SmfBuilder::new().track(track), LoopMarkers::None);
    let xg_event = Event::SysEx(SysEx::new(&xg[..7]).unwrap());
    assert_eq!(seek_state(smf, Duration::from_millis(500)), [xg_event, cc(0, 7, 50), cc(0, 10, 20)]);

    let exit = gs_bytes([0x40, 0x00, 0x7F], 0x7F);
    let track = Track::new().cc(0, 0, 7, 50).sysex(0, &exit).cc(0, 0, 0, 1).end(960);
    let smf = load(&SmfBuilder::new().track(track), LoopMarkers::None);
    let exit_event = Event::SysEx(SysEx::new(&exit[..9]).unwrap());
    assert_eq!(seek_state(smf, Duration::from_millis(500)), [exit_event, cc(0, 0, 1)]);
}

/// A rhythm part resets its channel's program: the program before it is
/// not sent back after it, nor are the controllers it reset.
#[test]
fn the_chase_keeps_rhythm_parts_in_their_place() {
    let rhythm = gs_bytes([0x40, 0x11, 0x15], 1);
    let track = Track::new().cc(0, 0, 0, 1).program(0, 0, 5).cc(0, 0, 1, 90).cc(0, 0, 7, 50).sysex(0, &rhythm).end(960);
    let smf = load(&SmfBuilder::new().track(track), LoopMarkers::None);
    let rhythm_event = Event::SysEx(SysEx::new(&rhythm[..9]).unwrap());
    // The bank select stays, before the rhythm part: CC121 keeps it.
    assert_eq!(seek_state(smf, Duration::from_millis(500)), [cc(0, 0, 1), rhythm_event, cc(0, 7, 50)]);
}

// Reset messages where a jump lands, and the channels a file uses.

const GS_RESET: [u8; 10] = [0x41, 0x10, 0x42, 0x12, 0x40, 0x00, 0x7F, 0x00, 0x41, 0xF7];

/// A file whose loop starts at 1 s with, at that tick, CC7 and a note, a GS
/// reset, then CC10 and another note; it ends at 2 s.
fn reset_at_the_loop_start() -> Smf {
    let track = Track::new()
        .program(0, 0, 5)
        .cc(960, 0, 111, 0)
        .cc(0, 0, 7, 20)
        .note_on(0, 0, 60, 100)
        .sysex(0, &GS_RESET)
        .cc(0, 0, 10, 30)
        .note_on(0, 0, 62, 100)
        .end(960);
    load(&SmfBuilder::new().track(track), LoopMarkers::RpgMaker)
}

/// Going round, the reset message at the loop's start is handed out as the
/// jump's reset, in place of the reset of the channels; what its tick has
/// before it is undone by it, and left out; what follows it plays. A seek
/// there does the same.
#[test]
fn a_reset_at_the_jump_target_takes_the_place_of_the_reset_of_the_channels() {
    let reset = Event::SysEx(SysEx::new(&GS_RESET[..9]).unwrap());
    let after = [reset, cc(0, 10, 30), Event::NoteOn { channel: 0, key: 62, velocity: 100 }];
    let out = play(reset_at_the_loop_start(), 1000, true, 3500);
    let at = |frame| out.iter().filter(|&&(f, _)| f == frame).map(|&(_, e)| e).collect::<Vec<_>>();
    // From the start, the tick plays as it is.
    let mut first = vec![cc(0, 7, 20), Event::NoteOn { channel: 0, key: 60, velocity: 100 }];
    first.extend(after);
    assert_eq!(at(1000), first);
    let mut round = release_events(1);
    round.extend(after);
    assert_eq!(at(2000), round);
    assert_eq!(at(3000), round);

    let mut seq = sequencer(Arc::new(reset_at_the_loop_start()), 1000);
    seq.next_events(300, |_, _| {});
    seq.seek(Duration::from_secs(1));
    let mut sought = Vec::new();
    seq.next_events(301, |_, e| sought.push(e));
    assert_eq!(sought, round);
}

/// Through a synthesizer, going round or seeking to a tick with resets
/// leaves the channels as the first time round, and the bank select mode as
/// the last reset of the tick set it.
#[test]
fn a_reset_where_a_jump_lands_gives_the_state_it_gives_from_the_start() {
    let gm_on = [0x7E, 0x7F, 0x09, 0x01, 0xF7];
    let track = Track::new()
        .cc(0, 1, 7, 50)
        .cc(960, 0, 111, 0)
        .cc(0, 1, 10, 20)
        .sysex(0, &GS_RESET)
        .cc(0, 1, 0, 5)
        .sysex(0, &gm_on)
        .cc(0, 1, 91, 70)
        .sysex(480, &GS_RESET)
        .end(480);
    let smf = Arc::new(load(&SmfBuilder::new().track(track), LoopMarkers::RpgMaker));
    let mut synth = chase_synth();
    let mut seq = Sequencer::new(smf, &synth);
    seq.set_looping(true);
    render_frames(&mut seq, &mut synth, 9601);
    let first = synth.channel_states();
    assert_eq!(first.0, BankStyle::Gm);
    assert_eq!((first.1[1].cc[7], first.1[1].cc[10], first.1[1].cc[91]), (100, 64, 70));
    render_frames(&mut seq, &mut synth, 4800);
    assert_eq!(synth.channel_states().0, BankStyle::Gs, "the reset within the loop plays");
    render_frames(&mut seq, &mut synth, 4800);
    assert_eq!(synth.channel_states(), first, "going round");
    seq.seek(Duration::from_secs(1));
    render_frames(&mut seq, &mut synth, 1);
    assert_eq!(synth.channel_states(), first, "seeking");
}

/// A file starting with a GS reset, looped whole: going round does not stop
/// the notes still sounding nor clear the reverb. It plays exactly as the
/// same file without the reset.
#[test]
fn a_reset_at_the_loop_start_cuts_no_notes_and_clears_no_effects() {
    use crate::soundfont::GeneratorType as G;
    use crate::testkit::sf2::{SampleSpec, Sf2, Zone, sine};
    let sound = SampleSpec::new("s", sine(4410, 100.0, 16000)).looped(0, 4410);
    // A release of about 2 s.
    let sf = Arc::new(Sf2::single(sound, Zone::new().set(G::SampleModes, 1).set(G::ReleaseVolEnv, 1200)).load());
    let file = |reset: bool| {
        let mut t = Track::new();
        if reset {
            // What the tick has before the reset, the reset undoes.
            t = t.cc(0, 0, 7, 20).sysex(0, &GS_RESET);
        }
        let t = t.cc(0, 0, 91, 127).note_on(0, 0, 60, 100).note_off(480, 0, 60).end(480);
        Arc::new(load(&SmfBuilder::new().track(t), LoopMarkers::None))
    };
    let mut outputs = Vec::new();
    for reset in [false, true] {
        let mut synth = crate::Synth::new(sf.clone(), crate::synth::Config::new(44100)).unwrap();
        let mut seq = Sequencer::new(file(reset), &synth);
        seq.set_looping(true);
        let mut out = render_all(&mut seq, &mut synth, &[64], 44100 + 320);
        // The first note's release, and the second note.
        assert_eq!(synth.active_voices(), 2, "reset: {reset}");
        out.extend(render_all(&mut seq, &mut synth, &[64], 3 * 44100));
        outputs.push(out);
    }
    assert!(outputs[0].iter().any(|x| *x != 0.0));
    assert!(outputs[0] == outputs[1], "the reset changes what plays");
}

/// The channels a file uses are those of its channel messages and its GS
/// rhythm parts; loop markers, taken out, are not.
#[test]
fn the_channels_a_file_uses() {
    let rhythm = gs_bytes([0x40, 0x1A, 0x15], 1);
    let track = Track::new()
        .note_on(0, 2, 60, 100)
        .pitch_bend(0, 4, 0)
        .sysex(0, &rhythm)
        .sysex(0, &GS_RESET)
        .cc(0, 7, 111, 0)
        .end(960);
    let smf = load(&SmfBuilder::new().track(track), LoopMarkers::RpgMaker);
    assert_eq!(smf.channels, 1 << 2 | 1 << 4 | 1 << 10);
    let smf = load(&SmfBuilder::new().track(Track::new().sysex(0, &GS_RESET).end(960)), LoopMarkers::None);
    assert_eq!(smf.channels, 0);
}

/// A host playing on a channel the file does not use: jumps neither
/// release its notes nor reset its channel. The file's own channels are
/// released and reset.
#[test]
fn jumps_leave_the_channels_the_file_does_not_use_alone() {
    let track = Track::new().cc(0, 0, 7, 90).note_on(0, 0, 60, 100).note_off(96, 0, 60).end(864);
    let smf = Arc::new(load(&SmfBuilder::new().track(track), LoopMarkers::None));
    let mut synth = chase_synth();
    for event in [
        Event::ProgramChange { channel: HOST, program: 5 },
        cc(HOST, 7, 33),
        cc(HOST, 10, 10),
        cc(HOST, 101, 0),
        cc(HOST, 100, 0),
        cc(HOST, 6, 7),
        Event::PitchBend { channel: HOST, value: 1000 },
        cc(HOST, 64, 127),
        Event::NoteOn { channel: HOST, key: 60, velocity: 100 },
    ] {
        synth.send(event);
    }
    let host = synth.channel_states().1[HOST as usize].clone();
    let mut seq = Sequencer::new(smf, &synth);
    seq.set_looping(true);
    // Round three times, then halfway into the fourth, where the file's
    // note is over.
    render_frames(&mut seq, &mut synth, 3 * 9600 + 4800);
    assert_eq!(synth.active_voices(), 1, "the host's note sounds on");
    seq.seek(Duration::from_millis(500));
    render_frames(&mut seq, &mut synth, 100);
    assert_eq!(synth.active_voices(), 1, "the host's note sounds on");
    let states = synth.channel_states();
    assert_eq!(states.1[HOST as usize], host);
    assert_eq!(states.1[0].cc[7], 90);
    // The other channels the file does not use are as they were.
    let fresh = chase_synth().channel_states();
    for ch in (1..16).filter(|&c| c != HOST as usize) {
        assert_eq!(states.1[ch], fresh.1[ch], "channel {ch}");
    }
}

/// Frames the synthesizer renders without the sequencer pause the music:
/// what follows is not late, nor crowded at the start of a render.
#[test]
fn rendering_the_synthesizer_alone_pauses_the_music() {
    let track = Track::new().note_on(480, 0, 60, 100).note_off(480, 0, 60).end(0);
    let smf = Arc::new(load(&SmfBuilder::new().track(track), LoopMarkers::None));
    let play = |pause: usize| {
        let mut synth = chase_synth();
        let mut seq = Sequencer::new(smf.clone(), &synth);
        let (mut l, mut r) = (vec![0.0; RATE as usize], vec![0.0; RATE as usize]);
        seq.render(&mut synth, &mut l[..960], &mut r[..960]);
        synth.render(&mut l[..pause], &mut r[..pause]);
        for chunk in (0..RATE as usize).step_by(512) {
            let end = (chunk + 512).min(RATE as usize);
            seq.render(&mut synth, &mut l[chunk..end], &mut r[chunk..end]);
        }
        l
    };
    // The note at 0.5 s, 960 frames of which were played, starts at the same
    // frame of what follows whether or not the synthesizer played alone.
    let (straight, paused) = (play(0), play(1920));
    assert!(straight.iter().any(|x| *x != 0.0));
    assert_eq!(paused, straight);
}

/// However many events a file has at once, a render call takes as many as
/// the synthesizer's work budget allows, and the rest follow, late, in
/// order.
#[test]
fn a_crowd_of_events_at_once_plays_late_in_order() {
    let mut track = Track::new().running_status(true);
    for i in 0..200_000u32 {
        track = track.cc(0, 0, 7, (i % 128) as u8);
    }
    let track = track.cc(0, 0, 7, 99).note_on(0, 0, 60, 100).end(0);
    let smf = Arc::new(load(&SmfBuilder::new().track(track), LoopMarkers::None));
    let mut synth = chase_synth();
    let mut seq = Sequencer::new(smf, &synth);
    let (mut l, mut r) = (vec![0.0; 64], vec![0.0; 64]);
    seq.render(&mut synth, &mut l, &mut r);
    assert!(seq.index < 50_000, "{} events in one render of 64 frames", seq.index);
    // An eighth of the queue is left to the host.
    assert!(synth.queue_room() >= synth.config().event_capacity / 8);
    while !seq.finished() {
        seq.render(&mut synth, &mut l, &mut r);
    }
    render_frames(&mut seq, &mut synth, 64);
    assert_eq!(synth.channel_states().1[0].cc[7], 99);
    assert_eq!(synth.active_voices(), 1);
}
