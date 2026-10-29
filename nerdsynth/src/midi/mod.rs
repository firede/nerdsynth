//! Standard MIDI Files, and playing them.
//!
//! # Time
//!
//! [`Smf`] keeps the time of each event exactly, as a whole number of units
//! of `1 / d` second, `d` depending on the file's division: a million times
//! the ticks per quarter note (tempos being in microseconds per quarter
//! note), or the SMPTE frames per second times the ticks per frame. A
//! [`Sequencer`] hands an event out at its time times the sample rate,
//! rounded to the nearest frame. Loops add their exact length each time
//! round, so playing a loop a thousand times puts the events where a file a
//! thousand times as long would have them: rounding never builds up.

use std::io::Read;
use std::sync::Arc;
use std::time::Duration;

use crate::error::Setting;
use crate::event::SysEx;
use crate::synth::BankStyle;
use crate::{Error, Event, Synth};

mod chase;
mod parse;
#[cfg(test)]
mod tests;

use chase::Item;

/// Which controllers mark a loop in a MIDI file. The controllers marking
/// it are taken out of the events.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum LoopMarkers {
    /// None: a looping sequencer loops the whole file.
    #[default]
    None,
    /// CC111 marks the start, as RPG Maker does; the end of the file ends
    /// the loop.
    RpgMaker,
    /// CC110 and CC111 mark the start and the end (The Incredible Machine).
    IncredibleMachine,
    /// CC116 and CC117 mark the start and the end (Final Fantasy).
    FinalFantasy,
}

/// An event of the merged list: a channel message, its status and data, or
/// a system exclusive message (status 0xF0) at index `sysex`.
#[derive(Clone, Copy, Debug)]
struct Entry {
    time: u64,
    status: u8,
    d1: u8,
    d2: u8,
    sysex: u32,
}

impl Entry {
    fn event(&self, sysex: &[SysEx]) -> Event {
        let channel = self.status & 0x0F;
        let (key, d1, d2) = (self.d1, self.d1, self.d2);
        match self.status & 0xF0 {
            0x80 => Event::NoteOff { channel, key },
            0x90 => Event::NoteOn { channel, key, velocity: d2 },
            0xA0 => Event::PolyPressure { channel, key, pressure: d2 },
            0xB0 => Event::ControlChange { channel, controller: d1, value: d2 },
            0xC0 => Event::ProgramChange { channel, program: d1 },
            0xD0 => Event::ChannelPressure { channel, pressure: d1 },
            0xE0 => Event::PitchBend { channel, value: d1 as u16 | (d2 as u16) << 7 },
            _ => Event::SysEx(sysex[self.sysex as usize]),
        }
    }
}

/// The loop a looping sequencer plays: from `start` to `end`, the events
/// from `start_index` to `end_index`.
#[derive(Clone, Copy, Debug)]
struct Loop {
    start: u64,
    start_index: usize,
    end: u64,
    end_index: usize,
}

/// A Standard MIDI File (format 0 or 1), its tracks merged into one list of
/// events in time.
///
/// Events of the same tick keep the order of their tracks, and within a
/// track their own order; loop markers go before them, so that a loop
/// starting at a tick plays every event of that tick, and one ending at a
/// tick plays none. Tempo events set the time and are not events; other
/// meta events are dropped. System exclusive messages, in F0 events and in
/// F7 packets, are events if they fit in [`SysEx`].
///
/// Loading fails without panicking on any data, and fails for files longer
/// than 24 hours.
///
/// Its [`Debug`](std::fmt::Debug) output is a summary, not the events.
#[derive(Clone)]
pub struct Smf {
    /// Units of time a second.
    den: u64,
    events: Vec<Entry>,
    sysex: Vec<SysEx>,
    /// The end of the last track.
    end: u64,
    /// The loop, unless shorter than a millisecond: between the markers if
    /// the file has them, else the whole file.
    looped: Option<Loop>,
    /// The channels the file uses, bit `n` for channel `n`: those of its
    /// channel messages and of its GS rhythm parts. Jumps release and reset
    /// these only.
    channels: u16,
}

impl Smf {
    /// Loads a file, reading `reader` up to the end of its last track,
    /// taking the controllers of `markers` as marking its loop.
    pub fn read<R: Read>(reader: R, markers: LoopMarkers) -> Result<Smf, Error> {
        parse::read(reader, markers)
    }

    /// Loads a file from memory; see [`read`](Self::read).
    pub fn from_bytes(bytes: &[u8], markers: LoopMarkers) -> Result<Smf, Error> {
        Smf::read(bytes, markers)
    }

    /// The time from the start to the end of the last track. Notes still
    /// sound after it, for as long as their release and the effects last.
    pub fn duration(&self) -> Duration {
        self.to_duration(self.end as u128)
    }

    fn to_duration(&self, units: u128) -> Duration {
        let den = self.den as u128;
        Duration::from_nanos(((units * 1_000_000_000 + den / 2) / den) as u64)
    }
}

/// The events of each of `channels` (bit `n` for channel `n`) that release
/// every note, whatever the pedals: sustain and sostenuto off, All Notes
/// Off, then Reset All Controllers.
fn release(channels: u16, mut f: impl FnMut(Event)) {
    for channel in (0..16).filter(|c| channels >> c & 1 != 0) {
        for controller in [64, 66, 123, 121] {
            f(Event::ControlChange { channel, controller, value: 0 });
        }
    }
}

/// Plays an [`Smf`]: hands out its events frame by frame, at a sample rate.
///
/// # Jumps
///
/// Going back to the start of a loop and [`seek`](Self::seek) are jumps.
/// At a jump it hands out, at the frame it jumps at, for the channels the
/// file uses (those it sends channel messages or GS rhythm parts to):
///
/// 1. CC64 0 and CC66 0 (pedals up), CC123 (All Notes Off) and CC121
///    (Reset All Controllers), releasing their notes;
/// 2. the reset of those channels: [`render`](Self::render) sets each to
///    its initial state, as [`Synth::reset`] does but with the voices and
///    the effects going on, and bank selects to follow the mode the file's
///    last GM, GM2 or GS reset message up to the target set, or else the
///    mode the sequencer started from. [`next_events`](Self::next_events)
///    hands out that reset message, if any: no MIDI event resets a
///    channel's volume, pan, program or bend range;
/// 3. the events that set the state the file had at the target: system
///    exclusive messages, programs and bank selects, controllers, RPNs and
///    NRPNs in their order, pitch bend and pressures.
///
/// What follows then plays on a synthesizer as it would have played from
/// the start, whatever was set after the target before the jump. The other
/// channels are left alone, for a host playing other things on them,
/// though bank selects follow the file's mode on every channel.
///
/// A reset message at the very tick jumped to is not played again: the
/// reset of the channels takes its place, with the mode it sets, and the
/// events of that tick before it, which it would undo, are left out. So a
/// file starting with a GS reset, looped whole, does not cut the notes
/// still sounding and clear the reverb each time round. Played from the
/// start, the message does what it does, to every channel.
pub struct Sequencer {
    smf: Arc<Smf>,
    sample_rate: u32,
    /// The bank select mode it started from, which a jump to before any
    /// reset message of the file goes back to.
    style: BankStyle,
    looping: bool,
    /// The next event.
    index: usize,
    /// Frames handed out: those before it.
    frame: u64,
    /// Where file time plays: a time `t` of the file in its units plays at
    /// `t · sample_rate + offset`, in units of `1 / (den · sample_rate)`
    /// second from the start of playing.
    offset: i128,
    /// What to hand out before the next event of the file, with the frames:
    /// a seek's, and going round the loop's. Its capacity is reserved for
    /// going round, so that playing never allocates.
    pending: Vec<(u64, Item)>,
    /// The first of `pending` not yet handed out.
    pending_at: usize,
    /// The state the file has set at the start of the loop.
    loop_state: Vec<Item>,
    /// The event going round the loop goes on from: past any reset message
    /// at the start of the loop.
    loop_resume: usize,
}

impl Sequencer {
    /// A sequencer at the start of `smf`, not looping, to play through
    /// `synth`: at its sample rate, starting from the way its bank selects
    /// choose banks now. It allocates, and takes a time in proportion to
    /// the file's events (it works out the state at the start of the loop):
    /// make it off the audio thread.
    pub fn new(smf: Arc<Smf>, synth: &Synth) -> Sequencer {
        Sequencer::make(smf, synth.config().sample_rate, synth.bank_style())
    }

    /// A sequencer at the start of `smf`, not looping, handing out its
    /// events at `sample_rate` for a host playing them elsewhere, as for a
    /// new [`Synth`] (bank selects as GS has them). Fails unless the rate
    /// is within 8000 to 192000 Hz, as a synthesizer's must be.
    pub fn with_sample_rate(smf: Arc<Smf>, sample_rate: u32) -> Result<Sequencer, Error> {
        if !(8000..=192_000).contains(&sample_rate) {
            return Err(Error::Config(Setting::SampleRate));
        }
        Ok(Sequencer::make(smf, sample_rate, BankStyle::Gs))
    }

    /// Any sample rate above 0 works; the public constructors keep to a
    /// synthesizer's, where a loop of at least a millisecond is at least 8
    /// frames long.
    fn make(smf: Arc<Smf>, sample_rate: u32, style: BankStyle) -> Sequencer {
        let loop_resume = smf.looped.map_or(0, |l| chase::resume_at(&smf, l.start_index, l.start));
        let loop_state = smf.looped.map_or_else(Vec::new, |_| chase::state_at(&smf, loop_resume, style));
        let room = 64 + loop_state.len();
        Sequencer {
            smf,
            sample_rate,
            style,
            looping: false,
            index: 0,
            frame: 0,
            offset: 0,
            pending: Vec::with_capacity(room),
            pending_at: 0,
            loop_state,
            loop_resume,
        }
    }

    /// The file it plays.
    pub fn smf(&self) -> &Arc<Smf> {
        &self.smf
    }

    /// The sample rate it hands events out at. [`render`](Self::render)
    /// changes it to the synthesizer's.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Whether it plays the loop again and again: between the loop markers
    /// if the file has them, else the whole file. Each time it goes back it
    /// jumps (see [Jumps](Self#jumps)) to the start of the loop.
    ///
    /// A loop shorter than a millisecond is not played: with markers that
    /// close, it loops the whole file, and a file that short plays once.
    pub fn set_looping(&mut self, looping: bool) {
        self.looping = looping;
    }

    fn active_loop(&self) -> Option<&Loop> {
        if self.looping { self.smf.looped.as_ref() } else { None }
    }

    fn rate(&self) -> i128 {
        self.sample_rate as i128
    }

    /// The frame of `x`, in units of `1 / (den · sample_rate)` second:
    /// rounded to the nearest, halves up.
    fn frame_of(&self, x: i128) -> u64 {
        let den = self.smf.den as i128;
        if x <= 0 { 0 } else { u64::try_from((2 * x + den) / (2 * den)).unwrap_or(u64::MAX) }
    }

    /// Goes on at `rate` from the time it has played.
    fn retime(&mut self, rate: u32) {
        let den = self.smf.den as i128;
        let (from, to) = (self.rate(), rate as i128);
        // The time played, in units of 1 / (den · from) second, then of
        // 1 / (den · to), rounded.
        let played = self.frame as i128 * den - self.offset;
        let played = (2 * played * to + from).div_euclid(2 * from);
        self.offset = self.frame as i128 * den - played;
        self.sample_rate = rate;
    }

    /// Calls `f` with each event before `until`, a frame counted from the
    /// start of playing, and the frame it plays at, in order; the next call
    /// goes on from there.
    ///
    /// Going back to the start of a loop, it hands out, at the frame of the
    /// loop's end, the events of a jump (see [Jumps](Self#jumps)) to the
    /// start of the loop. The time past the end goes on from the start, so
    /// loops keep time.
    ///
    /// A loop is at least a millisecond long, so the work of a call grows
    /// with the frames asked for and the events within them.
    pub fn next_events(&mut self, until: u64, mut f: impl FnMut(u64, Event)) {
        self.deliver(until, |frame, item| {
            match item {
                Item::Event(event) => f(frame, event),
                Item::Reset(Some((_, sysex))) => f(frame, Event::SysEx(sysex)),
                Item::Reset(None) => {}
            }
            true
        });
    }

    /// Hands out what is before `until` as [`next_events`](Self::next_events)
    /// does, as long as `f` takes it. When `f` refuses an item, the item is
    /// kept for the next call, the sequencer stops at its frame, and that
    /// frame is returned; else `until` is.
    fn deliver(&mut self, until: u64, mut f: impl FnMut(u64, Item) -> bool) -> u64 {
        if until <= self.frame {
            return self.frame;
        }
        let smf = Arc::clone(&self.smf);
        let rate = self.rate();
        loop {
            if self.pending_at < self.pending.len() {
                let mut taken = 0;
                let mut refused = None;
                for &(frame, item) in &self.pending[self.pending_at..] {
                    let frame = frame.max(self.frame);
                    if frame >= until {
                        refused = Some(until);
                        break;
                    }
                    if !f(frame, item) {
                        refused = Some(frame);
                        break;
                    }
                    taken += 1;
                }
                self.pending_at += taken;
                if self.pending_at == self.pending.len() {
                    self.pending.clear();
                    self.pending_at = 0;
                }
                if let Some(frame) = refused {
                    self.frame = frame;
                    return frame;
                }
            }
            let looped = if self.looping { smf.looped.as_ref() } else { None };
            let limit = looped.map_or(smf.events.len(), |l| l.end_index);
            if self.index < limit {
                let entry = &smf.events[self.index];
                let frame = self.frame_of(entry.time as i128 * rate + self.offset);
                if frame >= until {
                    break;
                }
                let frame = frame.max(self.frame);
                if !f(frame, Item::Event(entry.event(&smf.sysex))) {
                    self.frame = frame;
                    return frame;
                }
                self.index += 1;
                continue;
            }
            let Some(l) = looped else { break };
            let end = l.end as i128 * rate + self.offset;
            let now = self.frame as i128 * smf.den as i128;
            let at = if end < now {
                // Looping was turned on past the end of the loop: go back
                // now.
                self.offset = now - l.start as i128 * rate;
                self.frame
            } else {
                let at = self.frame_of(end);
                if at >= until {
                    break;
                }
                self.offset += (l.end - l.start) as i128 * rate;
                at
            };
            // Handed out from `pending`, so that a refusal can resume.
            release(smf.channels, |event| self.pending.push((at, Item::Event(event))));
            self.pending.extend(self.loop_state.iter().map(|&item| (at, item)));
            self.index = self.loop_resume;
        }
        self.frame = until;
        until
    }

    /// Schedules the events of the next frames on `synth` and renders them:
    /// as many frames as `synth.render` renders. The file goes on from where
    /// it was at the synthesizer's current frame: frames the synthesizer
    /// rendered without the sequencer are a pause in the music.
    ///
    /// Events are scheduled until the synthesizer's queue is full but for an
    /// eighth, left to the host's own events; the frames before the first
    /// event left out are rendered, which makes room, and so on, so that
    /// nothing allocates and no event plays late unless the synthesizer's
    /// work budget is spent (see
    /// [`Synth`](crate::Synth#work)): then the events wait, in order, until
    /// it allows. If the queue stays full of events for later frames
    /// (scheduled by someone else), an event plays a frame late for each
    /// frame it waits. The work of a call is thus bounded by the frames it
    /// renders, the queue's capacity and the budget, whatever the file.
    ///
    /// It plays at the synthesizer's sample rate: if the sequencer was made
    /// for another, it goes on from the time it has played at the
    /// synthesizer's.
    pub fn render(&mut self, synth: &mut Synth, left: &mut [f32], right: &mut [f32]) -> usize {
        let rate = synth.config().sample_rate;
        if rate != self.sample_rate {
            self.retime(rate);
        }
        let frames = left.len().min(right.len());
        // The sequencer's frame now is the synthesizer's, anew each time:
        // frames the synthesizer rendered without the sequencer (sound
        // effects while the music is paused) are a pause.
        let shift = i128::from(synth.now()) - i128::from(self.frame);
        let (style, channels) = (self.style, self.smf.channels);
        // An eighth of the queue left to the host's own events.
        let reserve = synth.config().event_capacity / 8;
        let mut done = 0;
        while done < frames {
            let from = self.frame;
            let target = from + (frames - done) as u64;
            let mut full = false;
            let reached = self.deliver(target, |frame, item| {
                let at = (i128::from(frame) + shift).clamp(0, i128::from(u64::MAX)) as u64;
                let ok = synth.queue_room() > reserve
                    && match item {
                        Item::Event(event) => synth.schedule(at, event).is_ok(),
                        Item::Reset(reset) => {
                            synth.schedule_reset(at, reset.map_or(style, |(s, _)| s), channels).is_ok()
                        }
                    };
                full |= !ok;
                ok
            });
            let mut n = (reached - from) as usize;
            if n == 0 && full {
                // The queue is full with events of this very frame: let the
                // synthesizer take them now, and go on queueing.
                if synth.handle_due_events() {
                    continue;
                }
                // Full of events waiting for the budget, or of events for
                // later frames: render until the synthesizer can take one.
                n = (synth.frames_until_ready().max(synth.frames_to_next_event()).max(1) as usize).min(frames - done);
                self.frame += n as u64;
            }
            let rendered = synth.render(&mut left[done..done + n], &mut right[done..done + n]);
            done += rendered;
        }
        frames
    }

    /// The time played, within the file: within the loop when looping.
    pub fn position(&self) -> Duration {
        let units = (self.frame as i128 * self.smf.den as i128 - self.offset).max(0) / self.rate();
        self.smf.to_duration((units as u128).min(self.smf.end as u128))
    }

    /// Goes to `time` in the file, or, when looping past the end of the
    /// loop, to where it would be going round the loop: the next call to
    /// [`next_events`](Self::next_events) or [`render`](Self::render) first
    /// hands out, at its first frame, the events of a jump there (see
    /// [Jumps](Self#jumps)). Notes before `time` are not played.
    ///
    /// It allocates, and takes a time in proportion to the events before
    /// `time`: it is not for an audio thread.
    pub fn seek(&mut self, time: Duration) {
        let smf = Arc::clone(&self.smf);
        let den = smf.den as u128;
        let nanos = time.as_nanos().min((parse::MAX_SECONDS as u128 + 1) * 1_000_000_000);
        let mut t = (nanos * den + 500_000_000) / 1_000_000_000;
        if let Some(l) = self.active_loop() {
            let (start, end) = (l.start as u128, l.end as u128);
            if t >= end {
                t = start + (t - start) % (end - start);
            }
        }
        let t = t.min(smf.end as u128) as u64;
        let index = chase::resume_at(&smf, smf.events.partition_point(|e| e.time < t), t);
        self.pending.clear();
        self.pending_at = 0;
        let at = self.frame;
        release(smf.channels, |event| self.pending.push((at, Item::Event(event))));
        let state = chase::state_at(&smf, index, self.style);
        self.pending.extend(state.into_iter().map(|item| (at, item)));
        self.index = index;
        self.offset = self.frame as i128 * den as i128 - t as i128 * self.rate();
    }

    /// Whether it has handed out every event and played to the end of the
    /// file, and does not loop. Events it scheduled may still wait in the
    /// synthesizer's queue: see [`Synth::is_silent`](crate::Synth::is_silent).
    pub fn finished(&self) -> bool {
        self.active_loop().is_none()
            && self.pending_at >= self.pending.len()
            && self.index >= self.smf.events.len()
            && self.frame_of(self.smf.end as i128 * self.rate() + self.offset) <= self.frame
    }
}

impl std::fmt::Debug for Smf {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let looped = self.looped.as_ref().map(|l| self.to_duration(l.start as u128)..self.to_duration(l.end as u128));
        f.debug_struct("Smf")
            .field("events", &self.events.len())
            .field("sysex", &self.sysex.len())
            .field("duration", &self.duration())
            .field("loop", &looped)
            .finish()
    }
}

impl std::fmt::Debug for Sequencer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sequencer")
            .field("smf", &self.smf)
            .field("sample_rate", &self.sample_rate)
            .field("looping", &self.looping)
            .field("position", &self.position())
            .field("finished", &self.finished())
            .finish()
    }
}

impl Clone for Sequencer {
    /// A sequencer in the same place, with the same room reserved for going
    /// round the loop, so that the clone does not allocate either.
    fn clone(&self) -> Sequencer {
        let mut pending = Vec::with_capacity(self.pending.capacity());
        pending.extend_from_slice(&self.pending);
        Sequencer {
            smf: Arc::clone(&self.smf),
            sample_rate: self.sample_rate,
            style: self.style,
            looping: self.looping,
            index: self.index,
            frame: self.frame,
            offset: self.offset,
            pending,
            pending_at: self.pending_at,
            loop_state: self.loop_state.clone(),
            loop_resume: self.loop_resume,
        }
    }
}
