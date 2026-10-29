//! Errors loading SoundFonts and MIDI files and making synthesizers.

use std::fmt;

/// An error loading a SoundFont or a MIDI file, or making a synthesizer.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// Reading failed.
    Io(std::io::Error),
    /// The data is not a valid file of its format.
    Invalid(InvalidFile),
    /// The file is valid but uses something this library does not support.
    Unsupported(Unsupported),
    /// A setting is out of its range.
    Config(Setting),
}

/// A kind of file this library reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Format {
    /// A SoundFont 2 file.
    SoundFont,
    /// A Standard MIDI File.
    Midi,
}

/// Why a file is not valid: its format, and a description of what is wrong
/// and where, for people (its wording is no part of the interface).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvalidFile {
    format: Format,
    message: String,
}

impl InvalidFile {
    pub(crate) fn new(format: Format, message: impl Into<String>) -> InvalidFile {
        InvalidFile { format, message: message.into() }
    }

    /// The format the file is not valid as.
    pub fn format(&self) -> Format {
        self.format
    }
}

impl fmt::Display for InvalidFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let format = match self.format {
            Format::SoundFont => "SoundFont",
            Format::Midi => "MIDI file",
        };
        write!(f, "invalid {format}: {}", self.message)
    }
}

/// What a valid file uses that this library does not support.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Unsupported {
    /// Compressed (Ogg Vorbis) samples: SoundFont 3.
    CompressedSamples,
    /// A format 2 MIDI file: independent sequences.
    MidiFormat2,
}

impl fmt::Display for Unsupported {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Unsupported::CompressedSamples => "compressed samples (SoundFont 3)",
            Unsupported::MidiFormat2 => "format 2 MIDI files (independent sequences)",
        })
    }
}

/// A setting of [`Config`](crate::Config), or the sample rate of a
/// [`Sequencer`](crate::Sequencer).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Setting {
    /// The sample rate: 8000 to 192000 Hz.
    SampleRate,
    /// [`Config::max_voices`](crate::Config::max_voices): 1 to 4096.
    MaxVoices,
    /// [`Config::event_capacity`](crate::Config::event_capacity): 16 to
    /// 65536.
    EventCapacity,
    /// [`Config::min_note_length`](crate::Config::min_note_length): 1 s at
    /// most.
    MinNoteLength,
    /// [`Config::gain`](crate::Config::gain): 0 to 1000.
    Gain,
}

impl fmt::Display for Setting {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Setting::SampleRate => "sample_rate is not within 8000 to 192000",
            Setting::MaxVoices => "max_voices is not within 1 to 4096",
            Setting::EventCapacity => "event_capacity is not within 16 to 65536",
            Setting::MinNoteLength => "min_note_length is longer than 1 s",
            Setting::Gain => "gain is not within 0 to 1000",
        })
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "read failed: {e}"),
            Error::Invalid(e) => e.fmt(f),
            Error::Unsupported(u) => write!(f, "unsupported: {u}"),
            Error::Config(s) => write!(f, "invalid setting: {s}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}
