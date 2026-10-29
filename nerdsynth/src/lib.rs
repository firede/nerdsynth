//! NerdSynth: a SoundFont 2 synthesizer.
//!
//! - [`SoundFont`] loads a SoundFont 2 file. It is immutable once loaded and
//!   meant to be shared between synthesizers in an [`Arc`](std::sync::Arc).
//! - [`Synth`] plays it: send it [`Event`]s, now or at an exact frame, and
//!   call [`Synth::render`] for the sound. Rendering never allocates, locks
//!   or panics, so it can run on an audio thread.
//! - [`Smf`] loads a Standard MIDI File and [`Sequencer`] plays it through a
//!   synthesizer, or hands its events to any host frame by frame.
//!
//! # Examples
//!
//! Playing a note:
//!
//! ```no_run
//! use std::sync::Arc;
//! use nerdsynth::{Config, Event, SoundFont, Synth};
//!
//! # fn main() -> Result<(), nerdsynth::Error> {
//! let sound_font = Arc::new(SoundFont::read(std::fs::File::open("piano.sf2")?)?);
//! let mut synth = Synth::new(sound_font, Config::new(44100))?;
//! synth.send(Event::NoteOn { channel: 0, key: 60, velocity: 100 });
//! // Released half a second in.
//! synth.schedule(22050, Event::NoteOff { channel: 0, key: 60 }).unwrap();
//!
//! let (mut left, mut right) = (vec![0.0; 512], vec![0.0; 512]);
//! for _ in 0..100 {
//!     synth.render(&mut left, &mut right);
//!     // Hand `left` and `right` to the audio device.
//! }
//! # Ok(())
//! # }
//! ```
//!
//! Playing a MIDI file:
//!
//! ```no_run
//! use std::sync::Arc;
//! use nerdsynth::midi::LoopMarkers;
//! use nerdsynth::{Config, Sequencer, Smf, SoundFont, Synth};
//!
//! # fn main() -> Result<(), nerdsynth::Error> {
//! let sound_font = Arc::new(SoundFont::read(std::fs::File::open("gm.sf2")?)?);
//! let smf = Arc::new(Smf::read(std::fs::File::open("song.mid")?, LoopMarkers::None)?);
//! let mut synth = Synth::new(sound_font, Config::new(48000))?;
//! let mut sequencer = Sequencer::new(smf, &synth);
//!
//! let (mut left, mut right) = (vec![0.0; 512], vec![0.0; 512]);
//! // On after the end, while the last notes and the reverb die away.
//! while !sequencer.finished() || !synth.is_silent() {
//!     sequencer.render(&mut synth, &mut left, &mut right);
//!     // Hand `left` and `right` to the audio device.
//! }
//! # Ok(())
//! # }
//! ```
//!
//! `SoundFont::read`, `Smf::read`, `Synth::new` and `Sequencer::new`
//! allocate and take time in proportion to the file: call them off the
//! audio thread, and move the synthesizer and the sequencer to it.

#![warn(missing_docs)]

pub mod error;
pub mod event;
pub mod midi;
pub mod soundfont;
pub mod synth;

#[cfg(any(test, feature = "testkit"))]
#[doc(hidden)]
pub mod testkit;

pub use error::Error;
pub use event::Event;
pub use midi::{Sequencer, Smf};
pub use soundfont::SoundFont;
pub use synth::{Config, QueueFull, Synth};

// Made on one thread and handed to the audio thread; shared read-only.
const _: () = {
    const fn send_sync<T: Send + Sync>() {}
    send_sync::<Synth>();
    send_sync::<Sequencer>();
    send_sync::<SoundFont>();
    send_sync::<Smf>();
    send_sync::<Config>();
    send_sync::<Event>();
    send_sync::<Error>();
};
