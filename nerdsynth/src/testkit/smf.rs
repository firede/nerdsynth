//! Building Standard MIDI Files in memory, valid or not.
//!
//! ```
//! use nerdsynth::testkit::smf::{SmfBuilder, Track};
//!
//! let bytes = SmfBuilder::new()
//!     .track(Track::new().tempo(0, 500_000).note_on(0, 0, 60, 100).note_off(480, 0, 60).end(0))
//!     .build();
//! ```

/// Appends `value` as a variable-length quantity, as many bytes as it
/// takes: over four for values from 2^28, which readers refuse.
pub fn write_vlq(out: &mut Vec<u8>, value: u64) {
    let mut groups = vec![(value & 0x7F) as u8];
    let mut rest = value >> 7;
    while rest > 0 {
        groups.push((rest & 0x7F) as u8 | 0x80);
        rest >>= 7;
    }
    out.extend(groups.iter().rev());
}

/// The bytes of a track chunk's events.
#[derive(Clone, Debug, Default)]
pub struct Track {
    bytes: Vec<u8>,
    running_status: bool,
    status: Option<u8>,
}

impl Track {
    pub fn new() -> Track {
        Track::default()
    }

    /// Whether channel messages leave out their status when it is that of
    /// the last channel message. Meta and system exclusive events keep it.
    pub fn running_status(mut self, on: bool) -> Track {
        self.running_status = on;
        self
    }

    /// A channel message: `status` and its data bytes, which are not
    /// checked.
    pub fn channel(mut self, delta: u64, status: u8, data: &[u8]) -> Track {
        write_vlq(&mut self.bytes, delta);
        if !(self.running_status && self.status == Some(status)) {
            self.bytes.push(status);
        }
        self.status = Some(status);
        self.bytes.extend_from_slice(data);
        self
    }

    pub fn note_on(self, delta: u64, channel: u8, key: u8, velocity: u8) -> Track {
        self.channel(delta, 0x90 | channel, &[key, velocity])
    }

    pub fn note_off(self, delta: u64, channel: u8, key: u8) -> Track {
        self.channel(delta, 0x80 | channel, &[key, 0])
    }

    pub fn poly_pressure(self, delta: u64, channel: u8, key: u8, pressure: u8) -> Track {
        self.channel(delta, 0xA0 | channel, &[key, pressure])
    }

    pub fn cc(self, delta: u64, channel: u8, controller: u8, value: u8) -> Track {
        self.channel(delta, 0xB0 | channel, &[controller, value])
    }

    pub fn program(self, delta: u64, channel: u8, program: u8) -> Track {
        self.channel(delta, 0xC0 | channel, &[program])
    }

    pub fn channel_pressure(self, delta: u64, channel: u8, pressure: u8) -> Track {
        self.channel(delta, 0xD0 | channel, &[pressure])
    }

    /// `value` from 0 to 16383.
    pub fn pitch_bend(self, delta: u64, channel: u8, value: u16) -> Track {
        self.channel(delta, 0xE0 | channel, &[(value & 0x7F) as u8, (value >> 7 & 0x7F) as u8])
    }

    /// A meta event of `kind`.
    pub fn meta(mut self, delta: u64, kind: u8, data: &[u8]) -> Track {
        write_vlq(&mut self.bytes, delta);
        self.bytes.extend_from_slice(&[0xFF, kind]);
        write_vlq(&mut self.bytes, data.len() as u64);
        self.bytes.extend_from_slice(data);
        self
    }

    /// A tempo, in microseconds per quarter note (24 bits).
    pub fn tempo(self, delta: u64, tempo: u32) -> Track {
        let [_, a, b, c] = tempo.to_be_bytes();
        self.meta(delta, 0x51, &[a, b, c])
    }

    /// End of Track.
    pub fn end(self, delta: u64) -> Track {
        self.meta(delta, 0x2F, &[])
    }

    /// An F0 event of `data`, the bytes after F0: a whole message ends with
    /// F7, the first packet of a divided one does not.
    pub fn sysex(self, delta: u64, data: &[u8]) -> Track {
        self.sysex_event(delta, 0xF0, data)
    }

    /// An F7 event of `data`: a packet going on with a divided message, or
    /// an escape.
    pub fn sysex_f7(self, delta: u64, data: &[u8]) -> Track {
        self.sysex_event(delta, 0xF7, data)
    }

    fn sysex_event(mut self, delta: u64, status: u8, data: &[u8]) -> Track {
        write_vlq(&mut self.bytes, delta);
        self.bytes.push(status);
        write_vlq(&mut self.bytes, data.len() as u64);
        self.bytes.extend_from_slice(data);
        self
    }

    /// Bytes as they are, valid or not.
    pub fn raw(mut self, bytes: &[u8]) -> Track {
        self.bytes.extend_from_slice(bytes);
        self
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

/// A Standard MIDI File: its header, and chunks.
#[derive(Clone, Debug)]
pub struct SmfBuilder {
    format: u16,
    track_count: Option<u16>,
    division: u16,
    header_size: u32,
    chunks: Vec<([u8; 4], Option<u32>, Vec<u8>)>,
}

impl Default for SmfBuilder {
    fn default() -> SmfBuilder {
        SmfBuilder::new()
    }
}

impl SmfBuilder {
    /// Format 1, 480 ticks per quarter note, no tracks.
    pub fn new() -> SmfBuilder {
        SmfBuilder { format: 1, track_count: None, division: 480, header_size: 6, chunks: Vec::new() }
    }

    pub fn format(mut self, format: u16) -> SmfBuilder {
        self.format = format;
        self
    }

    /// The division as it is in the header.
    pub fn division(mut self, division: u16) -> SmfBuilder {
        self.division = division;
        self
    }

    /// An SMPTE division: `frames_per_second` (24, 25, 29 for 30
    /// drop-frame, 30) and ticks per frame.
    pub fn smpte(self, frames_per_second: u8, ticks_per_frame: u8) -> SmfBuilder {
        let high = (-(frames_per_second as i8)) as u8;
        self.division(u16::from_be_bytes([high, ticks_per_frame]))
    }

    /// The track count the header says, instead of the tracks' number.
    pub fn track_count(mut self, count: u16) -> SmfBuilder {
        self.track_count = Some(count);
        self
    }

    /// The size the header chunk says, 6 or more; bytes past 6 are zeros.
    pub fn header_size(mut self, size: u32) -> SmfBuilder {
        self.header_size = size;
        self
    }

    pub fn track(self, track: Track) -> SmfBuilder {
        self.chunk(*b"MTrk", track.bytes)
    }

    /// A track whose chunk says it is `size` bytes, whatever it is.
    pub fn track_sized(mut self, track: Track, size: u32) -> SmfBuilder {
        self.chunks.push((*b"MTrk", Some(size), track.bytes));
        self
    }

    /// A chunk of any type.
    pub fn chunk(mut self, id: [u8; 4], body: Vec<u8>) -> SmfBuilder {
        self.chunks.push((id, None, body));
        self
    }

    pub fn build(&self) -> Vec<u8> {
        let tracks = self.chunks.iter().filter(|(id, _, _)| id == b"MTrk").count() as u16;
        let mut out = Vec::new();
        out.extend_from_slice(b"MThd");
        out.extend_from_slice(&self.header_size.to_be_bytes());
        out.extend_from_slice(&self.format.to_be_bytes());
        out.extend_from_slice(&self.track_count.unwrap_or(tracks).to_be_bytes());
        out.extend_from_slice(&self.division.to_be_bytes());
        out.resize(out.len() + self.header_size.saturating_sub(6) as usize, 0);
        for (id, size, body) in &self.chunks {
            out.extend_from_slice(id);
            out.extend_from_slice(&size.unwrap_or(body.len() as u32).to_be_bytes());
            out.extend_from_slice(body);
        }
        out
    }
}
