//! MIDI events a synthesizer plays.

/// A MIDI event for one of the 16 channels, or a system exclusive message.
///
/// Data out of MIDI's range (a key of 200) is taken as the nearest in range
/// by the synthesizer, the same way for every event, so that a note off
/// always finds the note its note on played. Events for channels beyond 15
/// are ignored.
///
/// More kinds of events may be added; a `match` on it needs a `_` arm.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Event {
    /// Releases a key.
    NoteOff {
        /// 0 to 15.
        channel: u8,
        /// 0 to 127, 60 being middle C.
        key: u8,
    },
    /// Presses a key.
    NoteOn {
        /// 0 to 15.
        channel: u8,
        /// 0 to 127, 60 being middle C.
        key: u8,
        /// 0 to 127; 0 makes it a note off.
        velocity: u8,
    },
    /// Pressure on a key held (polyphonic aftertouch).
    PolyPressure {
        /// 0 to 15.
        channel: u8,
        /// 0 to 127, 60 being middle C.
        key: u8,
        /// 0 to 127.
        pressure: u8,
    },
    /// Sets a controller, or is a channel mode message (120 to 127).
    ControlChange {
        /// 0 to 15.
        channel: u8,
        /// 0 to 127.
        controller: u8,
        /// 0 to 127.
        value: u8,
    },
    /// Selects a preset, in the bank the bank select controllers chose.
    ProgramChange {
        /// 0 to 15.
        channel: u8,
        /// 0 to 127.
        program: u8,
    },
    /// Pressure on all the keys of a channel (channel aftertouch).
    ChannelPressure {
        /// 0 to 15.
        channel: u8,
        /// 0 to 127.
        pressure: u8,
    },
    /// Bends the pitch of a channel.
    PitchBend {
        /// 0 to 15.
        channel: u8,
        /// 0 to 16383, 8192 being the center.
        value: u16,
    },
    /// A system exclusive message, such as a GM or GS reset.
    SysEx(SysEx),
}

/// A system exclusive message, kept inline so that events can be queued
/// without allocating: the bytes between 0xF0 and 0xF7, up to
/// [`SysEx::CAPACITY`] of them.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct SysEx {
    len: u8,
    data: [u8; SysEx::CAPACITY],
}

impl SysEx {
    /// The longest message kept; GS and GM messages are far shorter. A
    /// later version may raise it, never lower it.
    pub const CAPACITY: usize = 32;

    /// The message of `data`, the bytes between 0xF0 and 0xF7, or `None`
    /// when longer than [`SysEx::CAPACITY`].
    pub fn new(data: &[u8]) -> Option<SysEx> {
        if data.len() > Self::CAPACITY {
            return None;
        }
        let mut sysex = SysEx { len: data.len() as u8, data: [0; Self::CAPACITY] };
        sysex.data[..data.len()].copy_from_slice(data);
        Some(sysex)
    }

    /// The bytes between 0xF0 and 0xF7.
    pub fn data(&self) -> &[u8] {
        &self.data[..self.len as usize]
    }
}

impl std::fmt::Debug for SysEx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("SysEx").field(&self.data()).finish()
    }
}

impl Event {
    /// Parses a complete MIDI message: a channel message with its status
    /// byte, or a system exclusive message from 0xF0 to 0xF7. Other
    /// messages, and incomplete ones, give `None`.
    pub fn from_midi(bytes: &[u8]) -> Option<Event> {
        let (&status, data) = bytes.split_first()?;
        if status == 0xF0 {
            let body = data.strip_suffix(&[0xF7])?;
            return SysEx::new(body).map(Event::SysEx);
        }
        let channel = status & 0x0F;
        let d = |i: usize| data.get(i).copied().filter(|b| b & 0x80 == 0);
        Some(match status & 0xF0 {
            0x80 => Event::NoteOff { channel, key: d(0)? },
            0x90 => Event::NoteOn { channel, key: d(0)?, velocity: d(1)? },
            0xA0 => Event::PolyPressure { channel, key: d(0)?, pressure: d(1)? },
            0xB0 => Event::ControlChange { channel, controller: d(0)?, value: d(1)? },
            0xC0 => Event::ProgramChange { channel, program: d(0)? },
            0xD0 => Event::ChannelPressure { channel, pressure: d(0)? },
            0xE0 => Event::PitchBend { channel, value: d(0)? as u16 | (d(1)? as u16) << 7 },
            _ => return None,
        })
    }
}
