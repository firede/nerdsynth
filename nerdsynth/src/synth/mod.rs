//! The synthesizer.
//!
//! # How it renders
//!
//! Time is a frame clock. Events are queued for frames and take effect
//! exactly there. Each voice computes its control values (envelopes, LFOs,
//! modulators, filter cutoff) at the start of each segment of 64 frames,
//! for the segment's end, and moves its gains linearly across it, so that
//! nothing clicks and the cost of control is per block, not per frame. The
//! filter glides to its new cutoff as FluidSynth's does, recomputing its
//! coefficients each frame while it glides. A voice's segments follow on from
//! its start; an event that changes the voice or its channel cuts its
//! segment at the event's frame, and leaves the other voices alone, so the
//! cost of an event is that of the voices it touches. Segments never
//! depend on the lengths `render` is called with: the output does not
//! depend on how it is asked for.
//!
//! Events spend a work budget earned by the frames rendered (see
//! `budget`): when a stream of events would cost more than a share of the
//! time, they wait and play late, so that rendering keeps up.
//!
//! The behaviour follows the SoundFont 2.04 specification and, where it says
//! nothing, FluidSynth 2.6; `spec/decisions.md` lists where it departs from
//! both.

mod alloc;
mod budget;
mod channel;
mod dsp;
mod effects;
mod events;
mod modulate;
mod queue;
mod voice;

#[cfg(test)]
mod tests;

use std::sync::Arc;
use std::time::Duration;

use crate::error::Setting;
use crate::{Error, Event, SoundFont};
use alloc::Allocator;
use budget::Budget;
pub(crate) use channel::{BankStyle, Channel, DRUM_BANK};
use effects::{Chorus, Reverb};
pub(crate) use events::{NRPN_SCALE, SysExKind, classify_sysex};
use queue::{Queue, Queued};
pub(crate) use voice::zone_plays;
use voice::{Context, Voice};

/// The most frames a segment lasts: the control rate.
const BLOCK: usize = 64;

/// How samples are read between their points.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum Interpolation {
    /// Straight lines between points: cheaper, duller.
    Linear,
    /// Four-point cubic, as FluidSynth does by default.
    #[default]
    Cubic,
}

/// The velocity-to-filter-cutoff default modulator (SoundFont §8.4.2), whose
/// form implementations disagree on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum VelocityToFilter {
    /// None, as FluidSynth 2.4–2.6 actually play: FluidSynth defines the
    /// modulator with a velocity switch but evaluates it, and any modulator
    /// of a file identical to it, as 0. SpessaSynth has none either.
    #[default]
    Off,
    /// FluidSynth's definition: -2400 cents times (128 - velocity) / 128,
    /// for velocities of 64 and above only
    /// (the switch is FluidSynth's change to the specification's form).
    Switched,
    /// The specification's (2.01): -2400 cents times (128 - velocity) / 128,
    /// for every velocity.
    Linear,
}

/// The settings of a [`Synth`].
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Config {
    /// In Hz, 8000 to 192000.
    pub sample_rate: u32,
    /// The voices that can sound at a time, 1 to 4096; 256 by default. A
    /// voice taken for a new one fades out over 2 ms, alongside. A note on
    /// starts at most this many voices, whatever its regions.
    pub max_voices: usize,
    /// The events that can wait in the queue, 16 to 65536; 4096 by default.
    pub event_capacity: usize,
    /// Whether the reverb and the chorus run; true by default.
    pub effects: bool,
    /// Cubic by default.
    pub interpolation: Interpolation,
    /// The velocity-to-filter default modulator; [`VelocityToFilter::Off`]
    /// by default, as FluidSynth plays.
    pub velocity_to_filter: VelocityToFilter,
    /// Whether CC74 (brightness) moves the filter cutoff by up to ±9600
    /// cents, as SpessaSynth's default modulator; false by default, as
    /// FluidSynth and the specification.
    pub cc74_to_filter: bool,
    /// The shortest a note sounds, 0 to 1 s; 10 ms by default, as
    /// FluidSynth. A note released sooner, by a note off or otherwise, goes
    /// on until then, so that notes as short as no time (drums, often) are
    /// heard.
    pub min_note_length: Duration,
    /// What the output is multiplied by, 0 to 1000; 1 by default. At 1 the
    /// level is FluidSynth's at a gain of 1, five times its default of 0.2;
    /// a full General MIDI arrangement goes well past full scale.
    pub gain: f32,
}

impl Config {
    /// The default settings at `sample_rate`.
    pub fn new(sample_rate: u32) -> Config {
        Config {
            sample_rate,
            max_voices: 256,
            event_capacity: 4096,
            effects: true,
            interpolation: Interpolation::Cubic,
            velocity_to_filter: VelocityToFilter::Off,
            cc74_to_filter: false,
            min_note_length: Duration::from_millis(10),
            gain: 1.0,
        }
    }

    fn check(&self) -> Result<(), Error> {
        let checks = [
            (Setting::SampleRate, (8000..=192_000).contains(&self.sample_rate)),
            (Setting::MaxVoices, (1..=4096).contains(&self.max_voices)),
            (Setting::EventCapacity, (16..=65536).contains(&self.event_capacity)),
            (Setting::MinNoteLength, self.min_note_length <= Duration::from_secs(1)),
            (Setting::Gain, (0.0..=1000.0).contains(&self.gain)),
        ];
        match checks.into_iter().find(|&(_, ok)| !ok) {
            Some((setting, _)) => Err(Error::Config(setting)),
            None => Ok(()),
        }
    }
}

/// The event queue is full; the event was not queued.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QueueFull;

impl std::fmt::Display for QueueFull {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the event queue is full")
    }
}

impl std::error::Error for QueueFull {}

/// A SoundFont synthesizer of 16 MIDI channels, channel 9 (the tenth)
/// playing drums.
///
/// It keeps a frame clock, starting at 0 and counting every frame
/// rendered. Events take effect at the frame they are scheduled for, exactly.
/// Nothing it does while rendering or handling events allocates, locks or
/// panics, and no sequence of events makes it output NaN or infinity. It
/// does not limit the output: a SoundFont can make it far louder than full
/// scale (a modulation LFO to volume alone can add 96 dB, as in FluidSynth).
///
/// # Work
///
/// Events cost work: a note on, the regions it goes through and the voices
/// it starts; any event, the voice slots it goes through and the voices
/// whose control it makes recompute. So that no stream of events, however
/// dense or costly, makes rendering fall behind, events spend a budget
/// earned by the frames rendered, enough for events to take about a quarter
/// of the time on a recent desktop machine and two milliseconds of work at
/// once. Once it is spent, the events due wait, in order, and play at the
/// first multiple of 64 frames it allows, late. Music plays far within it
/// (a chord of 256 notes takes about one millisecond at the default
/// polyphony); a file or a stream of events that does not is heard
/// dragging rather than stalling the audio. A note on goes through every
/// voice slot, twice [`Config::max_voices`], so at a high polyphony fewer
/// notes fit at once: at 4096, a chord of about 60.
///
/// An event always plays whole: a note on costs at most going through its
/// preset's regions (2^20 at most in a SoundFont) and starting
/// [`Config::max_voices`] voices.
pub struct Synth {
    sound_font: Arc<SoundFont>,
    config: Config,
    now: u64,
    channels: Box<[Channel; 16]>,
    /// Twice the polyphony: room for the voices fading out after being
    /// taken.
    voices: Box<[Voice]>,
    alloc: Allocator,
    queue: Queue,
    budget: Budget,
    /// Events due wait for the budget: they play at a multiple of a block.
    waiting: bool,
    /// The work of segments cut by events since the last multiple of a
    /// block, spent there, so that the budget between multiples does not
    /// depend on where renders end.
    cut_owed: i64,
    bank_style: BankStyle,
    /// The id of the next note on.
    next_id: u64,
    ctx: Context,
    buffers: Box<Buffers>,
    reverb: Reverb,
    chorus: Chorus,
}

/// Scratch buffers of one segment.
struct Buffers {
    voice: [f32; BLOCK],
    reverb: [f32; BLOCK],
    chorus: [f32; BLOCK],
}

impl Synth {
    /// A synthesizer playing `sound_font`. Fails if a setting is out of its
    /// range. All the memory it needs is allocated here.
    pub fn new(sound_font: Arc<SoundFont>, config: Config) -> Result<Synth, Error> {
        config.check()?;
        dsp::conv::concave(0.0); // Builds the curve tables now, not while rendering.
        let sample_rate = f64::from(config.sample_rate);
        let ctx = Context {
            sample_rate,
            interpolation: config.interpolation,
            velocity_to_filter: config.velocity_to_filter,
            cc74_to_filter: config.cc74_to_filter,
            sincos: dsp::filter::SinCos::new(sample_rate),
            min_note_frames: (config.min_note_length.as_secs_f64() * sample_rate) as u64,
        };
        let channels: [Channel; 16] = std::array::from_fn(Channel::new);
        let mut synth = Synth {
            voices: (0..config.max_voices * 2).map(|_| Voice::new()).collect(),
            alloc: Allocator::new(config.max_voices * 2, config.max_voices),
            queue: Queue::new(config.event_capacity),
            budget: Budget::new(config.sample_rate),
            waiting: false,
            cut_owed: 0,
            sound_font,
            now: 0,
            channels: Box::new(channels),
            bank_style: BankStyle::Gs,
            next_id: 1,
            ctx,
            buffers: Box::new(Buffers { voice: [0.0; BLOCK], reverb: [0.0; BLOCK], chorus: [0.0; BLOCK] }),
            reverb: Reverb::new(sample_rate),
            chorus: Chorus::new(sample_rate),
            config,
        };
        synth.reset();
        Ok(synth)
    }

    /// The SoundFont it plays.
    pub fn sound_font(&self) -> &Arc<SoundFont> {
        &self.sound_font
    }

    /// The settings it was made with.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// The frame the next frame rendered is.
    pub fn now(&self) -> u64 {
        self.now
    }

    /// Plays `event` now, at the next frame rendered, after the events queued
    /// for this frame or before. If the work budget is spent (see
    /// [Work](Self#work)), it waits in the queue as a scheduled event would;
    /// if the queue is full too, the oldest event due plays now, whatever
    /// the budget, to make room for it.
    pub fn send(&mut self, event: Event) {
        self.handle_due();
        if !self.budget.ready() || self.queue.next_frame().is_some_and(|f| f <= self.now) {
            if self.queue.push(self.now, Queued::Midi(event)) {
                return;
            }
            // Full: the oldest event due plays now, whatever the budget, to
            // make room; at most one more event's work.
            if let Some(item) = self.queue.pop_due(self.now) {
                self.handle_queued(item);
                let queued = self.queue.push(self.now, Queued::Midi(event));
                debug_assert!(queued);
                return;
            }
        }
        // Nothing waits before it.
        self.handle_queued(Queued::Midi(event));
    }

    /// Plays `event` at `frame`; events of past frames play at the next
    /// frame rendered, and events of the same frame in the order queued.
    /// Fails, without allocating, when the queue holds
    /// [`Config::event_capacity`] events.
    pub fn schedule(&mut self, frame: u64, event: Event) -> Result<(), QueueFull> {
        if self.queue.push(frame, Queued::Midi(event)) { Ok(()) } else { Err(QueueFull) }
    }

    /// Sets the channels of `channels` (bit `n` for channel `n`) to their
    /// initial state at `frame`, as [`reset`](Self::reset) does, and bank
    /// selects to follow `style`, leaving the voices and the effects alone:
    /// a sequencer's jump. Queued as [`schedule`](Self::schedule) queues an
    /// event.
    pub(crate) fn schedule_reset(&mut self, frame: u64, style: BankStyle, channels: u16) -> Result<(), QueueFull> {
        if self.queue.push(frame, Queued::ResetChannels(style, channels)) { Ok(()) } else { Err(QueueFull) }
    }

    /// How bank selects choose banks now.
    pub(crate) fn bank_style(&self) -> BankStyle {
        self.bank_style
    }

    /// Each channel's state, for comparing in tests: everything events set
    /// but the mode messages' controllers (120–127) and the sostenuto
    /// pedal's note order; and the bank style.
    #[cfg(test)]
    pub(crate) fn channel_states(&self) -> (BankStyle, Vec<channel::State>) {
        (self.bank_style, self.channels.iter().map(Channel::state).collect())
    }

    /// Renders `min(left.len(), right.len())` frames, and returns how many.
    /// The output is written, not added.
    pub fn render(&mut self, left: &mut [f32], right: &mut [f32]) -> usize {
        let n = left.len().min(right.len());
        let (left, right) = (&mut left[..n], &mut right[..n]);
        left.fill(0.0);
        right.fill(0.0);
        let mut done = 0;
        while done < n {
            self.handle_due();
            // Up to the next event or the next multiple of a block, whichever
            // first. Events waiting for the budget wait for a block: a chunk
            // ended for them would cost more than the frames earn.
            let grid = (self.now / BLOCK as u64 + 1) * BLOCK as u64;
            let end = match self.queue.next_frame() {
                Some(f) if f > self.now && f >= self.now + self.budget.frames_until_ready() => f,
                Some(f) if f > self.now && f < grid => {
                    // It will find the budget spent: it waits for the block.
                    self.waiting = true;
                    grid
                }
                _ => grid,
            };
            let chunk = ((end.min(grid) - self.now) as usize).min(n - done);
            self.render_chunk(&mut left[done..done + chunk], &mut right[done..done + chunk]);
            self.now += chunk as u64;
            done += chunk;
            if self.now == end && end < grid {
                // A chunk more for an event: work, counted at its end
                // whatever the lengths asked for.
                self.budget.spend(budget::CHUNK + budget::CHUNK_SLOT * self.voices.len() as i64);
            }
            if self.now == grid {
                self.budget.spend(std::mem::take(&mut self.cut_owed));
            }
        }
        let gain = self.config.gain;
        for x in left.iter_mut().chain(right.iter_mut()) {
            *x *= gain;
            if !x.is_finite() {
                *x = 0.0;
            }
        }
        n
    }

    /// Renders `out.len() / 2` frames into `out`, left and right
    /// interleaved, and returns how many.
    pub fn render_interleaved(&mut self, out: &mut [f32]) -> usize {
        let frames = out.len() / 2;
        let mut left = [0.0f32; BLOCK];
        let mut right = [0.0f32; BLOCK];
        let mut done = 0;
        while done < frames {
            let n = (frames - done).min(BLOCK);
            self.render(&mut left[..n], &mut right[..n]);
            for (i, frame) in out[2 * done..2 * (done + n)].chunks_exact_mut(2).enumerate() {
                frame[0] = left[i];
                frame[1] = right[i];
            }
            done += n;
        }
        // An odd sample left over: no frame of its own.
        out[2 * frames..].fill(0.0);
        frames
    }

    /// The voices sounding, including those fading out after being taken
    /// for a new note or stopped by All Sound Off.
    pub fn active_voices(&self) -> usize {
        self.voices.iter().filter(|v| !v.is_free()).count()
    }

    /// Whether nothing sounds: no voice, the effects' tails have died away,
    /// and no event due waits for the work budget. Events queued for later
    /// frames are not counted: a host that stops rendering when silent
    /// should render on until they have played.
    pub fn is_silent(&self) -> bool {
        self.voices.iter().all(Voice::is_free)
            && (!self.config.effects || (self.reverb.is_silent() && self.chorus.is_silent()))
            && self.queue.next_frame().is_none_or(|f| f > self.now)
    }

    /// Stops every voice at once, clears the queue and the effects, and
    /// sets every channel to its initial state. The clock goes on.
    pub fn reset(&mut self) {
        for v in self.voices.iter_mut() {
            v.stop();
        }
        self.queue.clear();
        self.budget.fill();
        self.waiting = false;
        self.cut_owed = 0;
        self.reverb.reset();
        self.chorus.reset();
        self.bank_style = BankStyle::Gs;
        self.reset_channels();
    }

    /// Handles the events due at the current frame now, rather than at the
    /// next render, making room in the queue, as far as the budget goes;
    /// true if there were any.
    pub(crate) fn handle_due_events(&mut self) -> bool {
        self.handle_due()
    }

    /// The room left in the queue.
    pub(crate) fn queue_room(&self) -> usize {
        self.config.event_capacity - self.queue.len()
    }

    /// The frames to render before the budget lets an event play: 0 if it
    /// does now, else to the first multiple of a block where it does.
    pub(crate) fn frames_until_ready(&self) -> u64 {
        match self.budget.frames_until_ready() {
            0 if self.waiting => self.now.next_multiple_of(BLOCK as u64) - self.now,
            0 => 0,
            n => (self.now + n).next_multiple_of(BLOCK as u64) - self.now,
        }
    }

    /// The frames to render before the next event queued is due: 0 if
    /// none is queued or one is due.
    pub(crate) fn frames_to_next_event(&self) -> u64 {
        self.queue.next_frame().map_or(0, |f| f.saturating_sub(self.now))
    }

    /// Handles the events due at the current frame, as far as the budget
    /// goes; true if there were any.
    fn handle_due(&mut self) -> bool {
        // Waiting events play at a multiple of a block, the frame a render
        // does not end a chunk at for them, so that when they play does not
        // depend on the lengths asked for.
        if self.waiting && self.now % BLOCK as u64 != 0 {
            return false;
        }
        let mut any = false;
        loop {
            if self.queue.next_frame().is_none_or(|f| f > self.now) {
                self.waiting = false;
                break;
            }
            if !self.budget.ready() {
                self.waiting = true;
                break;
            }
            if let Some(item) = self.queue.pop_due(self.now) {
                self.handle_queued(item);
                any = true;
            }
        }
        any
    }

    /// Renders `left.len()` frames, at most a block, with no event within;
    /// each voice begins its segments as it needs.
    fn render_chunk(&mut self, left: &mut [f32], right: &mut [f32]) {
        let len = left.len();
        let data =
            dsp::oscillator::Data { points: self.sound_font.sample_data(), low: self.sound_font.sample_data24() };
        let b = &mut *self.buffers;
        b.reverb[..len].fill(0.0);
        b.chorus[..len].fill(0.0);
        let mut cut = 0;
        for v in self.voices.iter_mut() {
            let ch = &self.channels[v.channel as usize & 15];
            let mut at = 0;
            while at < len && !v.is_free() {
                if v.needs_begin(ch) {
                    if v.segment_left() > 0 {
                        // Cut short by an event: work for the budget.
                        cut += budget::CUT + budget::CUT_MODULATOR * v.modulators() as i64;
                    }
                    v.begin(BLOCK, ch, &self.ctx);
                    if v.is_free() {
                        break;
                    }
                }
                let end = at + v.segment_left().min(len - at);
                v.render(
                    end - at,
                    data,
                    &self.ctx,
                    &mut b.voice,
                    [&mut left[at..end], &mut right[at..end], &mut b.reverb[at..end], &mut b.chorus[at..end]],
                );
                at = end;
            }
        }
        self.cut_owed += cut;
        self.budget.earn(len);
        if self.config.effects {
            // A silent effect with no input only moves its modulation on.
            let quiet = |x: &[f32]| x.iter().all(|v| *v == 0.0);
            if self.reverb.is_silent() && quiet(&b.reverb[..len]) {
                self.reverb.skip(len);
            } else {
                self.reverb.process(&b.reverb[..len], left, right);
            }
            if self.chorus.is_silent() && quiet(&b.chorus[..len]) {
                self.chorus.skip(len);
            } else {
                self.chorus.process(&b.chorus[..len], left, right);
            }
        }
    }
}

/// A summary: the SoundFont's, the settings, the clock and the voices
/// sounding.
impl std::fmt::Debug for Synth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Synth")
            .field("sound_font", &self.sound_font)
            .field("config", &self.config)
            .field("now", &self.now)
            .field("active_voices", &self.active_voices())
            .finish()
    }
}
