//! Reading a Standard MIDI File into one list of events in time.

use std::io::{self, Read};

use super::{Entry, Loop, LoopMarkers, Smf};
use crate::Error;
use crate::error::{Format, InvalidFile, Unsupported};
use crate::event::SysEx;

/// The longest file loaded, in seconds.
pub(super) const MAX_SECONDS: u64 = 24 * 60 * 60;

/// The tempo before any tempo event: 120 beats per minute.
const DEFAULT_TEMPO: u64 = 500_000;

// The statuses of raw events that are not channel messages. Loop markers
// sort before every other event of their tick.
const LOOP_START: u8 = 0x00;
const LOOP_END: u8 = 0x01;
const TEMPO: u8 = 0x02;
pub(super) const SYSEX: u8 = 0xF0;

/// An event of a track, in ticks: a channel message (its status and data),
/// a system exclusive message (`value` its index), a tempo (`value` in
/// microseconds per quarter note) or a loop marker.
#[derive(Clone, Copy)]
struct Raw {
    tick: u64,
    status: u8,
    d1: u8,
    d2: u8,
    value: u32,
}

/// How ticks turn into time, in units of `1 / den` second.
struct Timing {
    den: u64,
    /// Units a tick takes regardless of tempo (SMPTE), or 0 to take the
    /// tempo, in microseconds per quarter note, as the units of a tick
    /// (ticks per quarter note).
    per_tick: u64,
}

impl Timing {
    fn new(division: u16) -> Result<Timing, Error> {
        if division & 0x8000 == 0 {
            if division == 0 {
                return Err(invalid("the division is 0 ticks per quarter note"));
            }
            return Ok(Timing { den: 1_000_000 * division as u64, per_tick: 0 });
        }
        let ticks_per_frame = (division & 0xFF) as u64;
        if ticks_per_frame == 0 {
            return Err(invalid("the SMPTE division has 0 ticks per frame"));
        }
        let fps = -(((division >> 8) as u8 as i8) as i64);
        match fps {
            24 | 25 | 30 => Ok(Timing { den: fps as u64 * ticks_per_frame, per_tick: 1 }),
            // 30 drop-frame: 30000 / 1001 frames per second.
            29 => Ok(Timing { den: 30_000 * ticks_per_frame, per_tick: 1001 }),
            _ => Err(invalid(format!("the SMPTE division has {fps} frames per second"))),
        }
    }

    fn units(&self, ticks: u64, tempo: u64) -> u128 {
        let per_tick = if self.per_tick == 0 { tempo } else { self.per_tick };
        ticks as u128 * per_tick as u128
    }
}

fn invalid(message: impl Into<String>) -> Error {
    Error::Invalid(InvalidFile::new(Format::Midi, message))
}

/// Reads into `buf` until it is full or the reader ends, and gives how many
/// bytes it read.
fn read_full<R: Read>(reader: &mut R, buf: &mut [u8]) -> Result<usize, Error> {
    let mut filled = 0;
    while filled < buf.len() {
        match reader.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(filled)
}

pub(super) fn read<R: Read>(mut reader: R, markers: LoopMarkers) -> Result<Smf, Error> {
    let mut head = [0; 8];
    if read_full(&mut reader, &mut head)? < 8 {
        return Err(invalid("the file ends in its header"));
    }
    if &head[..4] != b"MThd" {
        return Err(invalid("the file does not start with an MThd chunk"));
    }
    let size = u32::from_be_bytes([head[4], head[5], head[6], head[7]]);
    if size < 6 {
        return Err(invalid(format!("the MThd chunk is {size} bytes, less than 6")));
    }
    let mut header = [0; 6];
    if read_full(&mut reader, &mut header)? < 6 {
        return Err(invalid("the file ends in its header"));
    }
    io::copy(&mut (&mut reader).take(size as u64 - 6), &mut io::sink())?;
    let format = u16::from_be_bytes([header[0], header[1]]);
    let track_count = u16::from_be_bytes([header[2], header[3]]);
    let timing = Timing::new(u16::from_be_bytes([header[4], header[5]]))?;
    match format {
        0 | 1 => {}
        2 => return Err(Error::Unsupported(Unsupported::MidiFormat2)),
        _ => return Err(invalid(format!("the format is {format}"))),
    }
    if track_count == 0 {
        return Err(invalid("the file has no track"));
    }

    let mut raws = Vec::new();
    let mut sysex = Vec::new();
    let mut end_tick = 0;
    let mut body = Vec::new();
    let mut found = 0;
    while found < track_count {
        let n = read_full(&mut reader, &mut head)?;
        // A file with fewer tracks than its header says plays the tracks it
        // has.
        if n == 0 && found > 0 {
            break;
        }
        if n < 8 {
            return Err(invalid(format!("the file ends before track {found}")));
        }
        let size = u32::from_be_bytes([head[4], head[5], head[6], head[7]]) as u64;
        if &head[..4] != b"MTrk" {
            // Chunks of other types are skipped, as the format asks.
            io::copy(&mut (&mut reader).take(size), &mut io::sink())?;
            continue;
        }
        body.clear();
        // Reading grows the buffer with the data there is, not with the size
        // the chunk claims.
        (&mut reader).take(size).read_to_end(&mut body)?;
        let track = Track { number: found, markers, raws: &mut raws, sysex: &mut sysex };
        end_tick = end_tick.max(track.read(&body)?);
        found += 1;
    }

    // Tracks are each in order already, so this merges them. The sort is
    // stable: within a tick, events keep the order of their tracks, and of
    // the track; loop markers go first.
    raws.sort_by_key(|r| (r.tick, r.status > LOOP_END));
    merge(&raws, sysex, end_tick, &timing, markers)
}

/// Turns ticks into time by the tempo map, and finds the loop.
fn merge(raws: &[Raw], sysex: Vec<SysEx>, end_tick: u64, timing: &Timing, markers: LoopMarkers) -> Result<Smf, Error> {
    let limit = MAX_SECONDS as u128 * timing.den as u128;
    let too_long = || invalid(format!("the file is longer than {} hours", MAX_SECONDS / 3600));
    let mut events = Vec::with_capacity(raws.len());
    let mut tempo = DEFAULT_TEMPO;
    let mut tick = 0;
    let mut time: u128 = 0;
    let mut loop_start: Option<(u64, usize)> = None;
    let mut loop_end: Option<(u64, usize)> = None;
    for raw in raws {
        time += timing.units(raw.tick - tick, tempo);
        tick = raw.tick;
        if time > limit {
            return Err(too_long());
        }
        let here = (time as u64, events.len());
        match raw.status {
            LOOP_START => {
                if loop_start.is_none() {
                    loop_start = Some(here);
                    // An end before the start ends no loop.
                    loop_end = None;
                }
            }
            LOOP_END => {
                if loop_end.is_none() {
                    loop_end = Some(here);
                }
            }
            TEMPO => tempo = raw.value as u64,
            status => events.push(Entry { time: time as u64, status, d1: raw.d1, d2: raw.d2, sysex: raw.value }),
        }
    }
    time += timing.units(end_tick - tick, tempo);
    if time > limit {
        return Err(too_long());
    }
    let end = time as u64;

    // A loop shorter than a millisecond is no loop: a sequencer would go
    // round it for nothing. Then the markers are ignored.
    let long_enough = |from: u64, to: u64| to > from && (to - from) as u128 * 1000 >= timing.den as u128;
    let (start, start_index) = loop_start.unwrap_or((0, 0));
    let (marked_end, end_index) = loop_end.unwrap_or((end, events.len()));
    let bounds = if markers != LoopMarkers::None && long_enough(start, marked_end) {
        Some((start, start_index, marked_end, end_index))
    } else if long_enough(0, end) {
        Some((0, 0, end, events.len()))
    } else {
        None
    };
    let looped = bounds.map(|(start, start_index, end, end_index)| Loop { start, start_index, end, end_index });
    let channels = super::chase::channels(&events, &sysex);
    Ok(Smf { den: timing.den, events, sysex, end, looped, channels })
}

/// A system exclusive message sent in packets: an F0 event without a final
/// F7, and F7 events going on with it.
struct Divided {
    data: [u8; SysEx::CAPACITY],
    len: usize,
    too_long: bool,
}

impl Divided {
    fn push(&mut self, bytes: &[u8]) {
        if self.too_long || self.len + bytes.len() > SysEx::CAPACITY {
            self.too_long = true;
            return;
        }
        self.data[self.len..self.len + bytes.len()].copy_from_slice(bytes);
        self.len += bytes.len();
    }
}

struct Track<'a> {
    number: u16,
    markers: LoopMarkers,
    raws: &'a mut Vec<Raw>,
    sysex: &'a mut Vec<SysEx>,
}

/// Reads through the bytes of a track.
struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl Cursor<'_> {
    fn byte(&mut self) -> Option<u8> {
        let b = *self.bytes.get(self.pos)?;
        self.pos += 1;
        Some(b)
    }

    /// A variable-length quantity: at most four bytes, seven bits each.
    fn vlq(&mut self) -> Option<Result<u32, ()>> {
        let mut value = 0;
        for _ in 0..4 {
            let b = self.byte()?;
            value = value << 7 | (b & 0x7F) as u32;
            if b & 0x80 == 0 {
                return Some(Ok(value));
            }
        }
        Some(Err(()))
    }

    fn take(&mut self, len: u32) -> Option<&[u8]> {
        let end = self.pos.checked_add(len as usize).filter(|&end| end <= self.bytes.len())?;
        let data = &self.bytes[self.pos..end];
        self.pos = end;
        Some(data)
    }
}

impl Track<'_> {
    /// Reads the events of a track, and gives the tick it ends at: its End
    /// of Track event, or else the end of its chunk.
    fn read(mut self, bytes: &[u8]) -> Result<u64, Error> {
        let mut cur = Cursor { bytes, pos: 0 };
        let mut tick: u64 = 0;
        let mut running = 0;
        let mut divided: Option<Divided> = None;
        while cur.pos < bytes.len() {
            let at = cur.pos;
            let short = || self.error(at, "the event goes past the end of the track");
            let delta = cur.vlq().ok_or_else(short)?.map_err(|()| self.error(at, "a delta time is over 4 bytes"))?;
            tick = tick.saturating_add(delta as u64);
            let first = cur.byte().ok_or_else(short)?;
            match first {
                0x00..=0x7F => {
                    if running == 0 {
                        return Err(self.error(at, "a data byte with no running status"));
                    }
                    self.channel(&mut cur, tick, running, Some(first), at)?;
                }
                0x80..=0xEF => {
                    // System exclusive and meta events keep the running
                    // status, as most readers do, though the format cancels
                    // it.
                    running = first;
                    self.channel(&mut cur, tick, first, None, at)?;
                }
                0xF0 | 0xF7 => {
                    let len = cur.vlq().ok_or_else(short)?.map_err(|()| self.error(at, "a length is over 4 bytes"))?;
                    let data = cur.take(len).ok_or_else(short)?;
                    self.sysex(tick, first, data, &mut divided);
                }
                0xFF => {
                    let kind = cur.byte().ok_or_else(short)?;
                    let len = cur.vlq().ok_or_else(short)?.map_err(|()| self.error(at, "a length is over 4 bytes"))?;
                    let data = cur.take(len).ok_or_else(short)?;
                    match (kind, data) {
                        // Events after the end of the track are ignored.
                        (0x2F, _) => return Ok(tick),
                        (0x51, &[a, b, c]) => {
                            let tempo = u32::from_be_bytes([0, a, b, c]);
                            // A tempo of 0 would stop time.
                            if tempo > 0 {
                                self.raws.push(Raw { tick, status: TEMPO, d1: 0, d2: 0, value: tempo });
                            }
                        }
                        _ => {}
                    }
                }
                _ => {
                    return Err(self.error(at, &format!("the status {first:#04X} (system common or real-time)")));
                }
            }
        }
        Ok(tick)
    }

    fn error(&self, at: usize, message: &str) -> Error {
        invalid(format!("track {}, byte {at}: {message}", self.number))
    }

    fn channel(&mut self, cur: &mut Cursor, tick: u64, status: u8, first: Option<u8>, at: usize) -> Result<(), Error> {
        let mut data = || {
            let b = cur.byte().ok_or_else(|| self.error(at, "the event goes past the end of the track"))?;
            if b & 0x80 != 0 {
                return Err(self.error(at, "a data byte has its top bit set"));
            }
            Ok(b)
        };
        let d1 = match first {
            Some(b) => b,
            None => data()?,
        };
        let d2 = if matches!(status & 0xF0, 0xC0 | 0xD0) { 0 } else { data()? };
        let marker = if status & 0xF0 == 0xB0 {
            match (self.markers, d1) {
                (LoopMarkers::RpgMaker, 111)
                | (LoopMarkers::IncredibleMachine, 110)
                | (LoopMarkers::FinalFantasy, 116) => Some(LOOP_START),
                (LoopMarkers::IncredibleMachine, 111) | (LoopMarkers::FinalFantasy, 117) => Some(LOOP_END),
                _ => None,
            }
        } else {
            None
        };
        let status = marker.unwrap_or(status);
        self.raws.push(Raw { tick, status, d1, d2, value: 0 });
        Ok(())
    }

    /// An F0 event, or an F7 one: a packet going on with a divided message,
    /// or an escape. Messages longer than [`SysEx::CAPACITY`] are dropped,
    /// and so are escapes other than a whole message.
    fn sysex(&mut self, tick: u64, status: u8, data: &[u8], divided: &mut Option<Divided>) {
        let message = match (status, divided.take()) {
            (0xF0, _) => match data.strip_suffix(&[0xF7]) {
                Some(body) => SysEx::new(body),
                None => {
                    let mut d = Divided { data: [0; SysEx::CAPACITY], len: 0, too_long: false };
                    d.push(data);
                    *divided = Some(d);
                    None
                }
            },
            (_, Some(mut d)) => match data.strip_suffix(&[0xF7]) {
                Some(body) => {
                    d.push(body);
                    if d.too_long { None } else { SysEx::new(&d.data[..d.len]) }
                }
                None => {
                    d.push(data);
                    *divided = Some(d);
                    None
                }
            },
            (_, None) => data.strip_prefix(&[0xF0]).and_then(|d| d.strip_suffix(&[0xF7])).and_then(SysEx::new),
        };
        if let Some(message) = message {
            self.raws.push(Raw { tick, status: SYSEX, d1: 0, d2: 0, value: self.sysex.len() as u32 });
            self.sysex.push(message);
        }
    }
}
