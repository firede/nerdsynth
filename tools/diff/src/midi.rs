//! MIDI messages of the test cases, and writing them as a Standard MIDI File.
//!
//! Times are in milliseconds: the file has 500 ticks per quarter note at
//! 120 bpm, so one tick is one millisecond.

/// Ticks per quarter note of the written files.
pub const PPQ: u16 = 500;
/// Microseconds per quarter note (120 bpm).
pub const TEMPO: u32 = 500_000;

/// FluidSynth renders in blocks of this many frames, and its MIDI player
/// only acts at block starts.
pub const FLUID_BLOCK: u64 = 64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Msg {
    NoteOn {
        ch: u8,
        key: u8,
        vel: u8,
    },
    NoteOff {
        ch: u8,
        key: u8,
    },
    Cc {
        ch: u8,
        cc: u8,
        val: u8,
    },
    Program {
        ch: u8,
        program: u8,
    },
    /// 0 to 16383, 8192 the center.
    Bend {
        ch: u8,
        value: u16,
    },
    ChannelPressure {
        ch: u8,
        value: u8,
    },
    PolyPressure {
        ch: u8,
        key: u8,
        value: u8,
    },
    /// The whole message, from 0xF0 to 0xF7.
    SysEx(Vec<u8>),
}

impl Msg {
    /// The message's bytes, with its status byte.
    pub fn bytes(&self) -> Vec<u8> {
        match *self {
            Msg::NoteOn { ch, key, vel } => vec![0x90 | ch, key, vel],
            Msg::NoteOff { ch, key } => vec![0x80 | ch, key, 64],
            Msg::Cc { ch, cc, val } => vec![0xB0 | ch, cc, val],
            Msg::Program { ch, program } => vec![0xC0 | ch, program],
            Msg::Bend { ch, value } => vec![0xE0 | ch, (value & 0x7F) as u8, (value >> 7 & 0x7F) as u8],
            Msg::ChannelPressure { ch, value } => vec![0xD0 | ch, value],
            Msg::PolyPressure { ch, key, value } => vec![0xA0 | ch, key, value],
            Msg::SysEx(ref data) => data.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Timed {
    pub ms: u32,
    pub msg: Msg,
}

/// Sorts events by time, keeping the order of events of the same time.
pub fn sorted(events: &[Timed]) -> Vec<Timed> {
    let mut v = events.to_vec();
    v.sort_by_key(|e| e.ms);
    v
}

/// A format 0 SMF of `events`, ending at `end_ms`.
pub fn write_smf(events: &[Timed], end_ms: u32) -> Vec<u8> {
    let mut track = Vec::new();
    let mut last = 0u32;
    let mut delta = |track: &mut Vec<u8>, ms: u32| {
        varlen(track, ms - last);
        last = ms;
        last
    };
    delta(&mut track, 0);
    track.extend_from_slice(&[0xFF, 0x51, 0x03, (TEMPO >> 16) as u8, (TEMPO >> 8) as u8, TEMPO as u8]);
    let mut at = 0;
    for e in sorted(events) {
        at = delta(&mut track, e.ms);
        let bytes = e.msg.bytes();
        if bytes[0] == 0xF0 {
            track.push(0xF0);
            varlen(&mut track, (bytes.len() - 1) as u32);
            track.extend_from_slice(&bytes[1..]);
        } else {
            track.extend_from_slice(&bytes);
        }
    }
    delta(&mut track, end_ms.max(at));
    track.extend_from_slice(&[0xFF, 0x2F, 0x00]);

    let mut smf = Vec::with_capacity(track.len() + 22);
    smf.extend_from_slice(b"MThd");
    smf.extend_from_slice(&6u32.to_be_bytes());
    smf.extend_from_slice(&0u16.to_be_bytes());
    smf.extend_from_slice(&1u16.to_be_bytes());
    smf.extend_from_slice(&PPQ.to_be_bytes());
    smf.extend_from_slice(b"MTrk");
    smf.extend_from_slice(&(track.len() as u32).to_be_bytes());
    smf.extend_from_slice(&track);
    smf
}

fn varlen(out: &mut Vec<u8>, mut v: u32) {
    let mut buf = [0u8; 5];
    let mut n = 0;
    loop {
        buf[n] = (v & 0x7F) as u8;
        n += 1;
        v >>= 7;
        if v == 0 {
            break;
        }
    }
    for i in (0..n).rev() {
        out.push(buf[i] | if i > 0 { 0x80 } else { 0 });
    }
}

/// The frame at which FluidSynth (fast rendering, with 64-frame periods)
/// plays an event of time `ms`.
///
/// Its MIDI player runs at the start of every 64-frame block `k`, with the
/// time `msec = (long)(1000 * 64k / rate)`, and plays the events whose tick
/// is at most `msec / deltatime + 0.5`; with one tick per millisecond,
/// deltatime is exactly 1.0. So an event goes out at the first block whose
/// `msec` is at least its time. What it does to the voices (a note on, a
/// controller's effect) is queued for the audio side, which takes the queue
/// at the start of the next render call: one block later, since every call
/// renders one block. The other renderers are given these frames, so that
/// every renderer plays every event at the same frame.
pub fn fluid_frame(ms: u32, rate: u32) -> u64 {
    let msec = |k: u64| (1000.0 * (k * FLUID_BLOCK) as f64 / rate as f64) as u64;
    let mut k = (ms as u64 * rate as u64 / (1000 * FLUID_BLOCK)).saturating_sub(1);
    while msec(k) < ms as u64 {
        k += 1;
    }
    (k + 1) * FLUID_BLOCK
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varlen_encodes() {
        let mut v = Vec::new();
        varlen(&mut v, 0);
        varlen(&mut v, 0x7F);
        varlen(&mut v, 0x80);
        varlen(&mut v, 0x0FFF_FFFF);
        assert_eq!(v, [0x00, 0x7F, 0x81, 0x00, 0xFF, 0xFF, 0xFF, 0x7F]);
    }

    #[test]
    fn fluid_frames() {
        // Each is one block after the block the player sends the event in.
        assert_eq!(fluid_frame(0, 44100), 64);
        // Block 1 starts at 1.45 ms, whose msec is 1.
        assert_eq!(fluid_frame(1, 44100), 128);
        assert_eq!(fluid_frame(2, 44100), 192);
        // 1000 ms is frame 44100, in block 689 (msec 999), so block 690.
        assert_eq!(fluid_frame(1000, 44100), 691 * 64);
        // At 48 kHz, every 4 ms is exactly a block start.
        assert_eq!(fluid_frame(4, 48000), 256);
    }
}
