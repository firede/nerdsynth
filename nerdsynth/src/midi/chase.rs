//! The state a file has set at a point: what a sequencer hands out when it
//! jumps there, so that what follows plays as it would have.
//!
//! A chase follows the events on a copy of each channel's state, the
//! synthesizer's own [`Channel`], doing what the synthesizer does: the same
//! reset for CC121, the same judgement of system exclusive messages. What
//! it hands out, after a reset of the channels the file uses, is for each
//! channel:
//!
//! - its structural events (bank selects, programs, the selection of RPNs
//!   and NRPNs and their data entry, CC121, GS rhythm parts) in their
//!   order, leaving out those whose every effect later ones undo before
//!   anything reads it;
//! - then the controllers, pitch bend and pressures where they differ from
//!   what those events leave.
//!
//! Replayed on reset channels, they give the channels the state the file
//! had, exactly: the order of what reads what is kept, so that, say, a
//! SoundFont NRPN still finds its LSB, and a rhythm part still undoes the
//! program before it.

use crate::Event;
use crate::event::SysEx;
use crate::synth::{BankStyle, Channel, NRPN_SCALE, SysExKind, classify_sysex};

use super::{Entry, Smf};

/// What a sequencer hands out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Item {
    Event(Event),
    /// The file's channels to their initial state. Bank selects follow the
    /// style the file's last reset message set, given with the message, or,
    /// if `None`, the sequencer's starting style.
    Reset(Option<(BankStyle, SysEx)>),
}

fn is_reset(entry: &Entry, sysex: &[SysEx]) -> bool {
    entry.status == 0xF0 && matches!(classify_sysex(sysex[entry.sysex as usize].data()), SysExKind::Reset(_))
}

/// Where playing goes on after a jump to `time`, `index` being its first
/// event at or after `time`: past the last reset message at `time`, if any.
/// The jump's reset of the channels takes the place of that message, and
/// the chase to there takes in the mode it sets; the events of the tick
/// before it are undone by it. Played again, the message would stop every
/// voice and clear the effects.
pub(super) fn resume_at(smf: &Smf, index: usize, time: u64) -> usize {
    let tick = smf.events[index..].iter().take_while(|e| e.time == time);
    tick.enumerate().filter(|(_, e)| is_reset(e, &smf.sysex)).last().map_or(index, |(i, _)| index + i + 1)
}

/// The channels `events` use, bit `n` for channel `n`: those of channel
/// messages and of GS rhythm parts.
pub(super) fn channels(events: &[Entry], sysex: &[SysEx]) -> u16 {
    events.iter().fold(0, |mask, e| match e.status {
        0xF0 => match classify_sysex(sysex[e.sysex as usize].data()) {
            SysExKind::RhythmPart { channel, .. } => mask | 1 << channel,
            _ => mask,
        },
        status => mask | 1 << (status & 0x0F),
    })
}

/// The state at `index`: the items setting the state the events of `smf`
/// before `index` set, from channels reset with bank selects following
/// `style`.
pub(super) fn state_at(smf: &Smf, index: usize, style: BankStyle) -> Vec<Item> {
    let mut chase = Chase::new(style);
    for entry in &smf.events[..index] {
        chase.feed(&entry.event(&smf.sysex));
    }
    chase.finish()
}

/// Cells of a channel's state that structural events read or write, as
/// bits.
type Cells = u128;

const CC0: Cells = 1 << 0;
const CC32: Cells = 1 << 1;
const CC6: Cells = 1 << 2;
const CC38: Cells = 1 << 3;
const CC98: Cells = 1 << 4;
const CC99: Cells = 1 << 5;
const CC100: Cells = 1 << 6;
const CC101: Cells = 1 << 7;
/// Whether data entry goes to an NRPN.
const ACTIVE: Cells = 1 << 8;
/// The SoundFont NRPN selected.
const SELECT: Cells = 1 << 9;
const BANK: Cells = 1 << 10;
const DRUM: Cells = 1 << 11;
/// The program and the preset.
const PROGRAM: Cells = 1 << 12;
const BEND_RANGE: Cells = 1 << 13;
const MOD_DEPTH: Cells = 1 << 14;
/// The generator offsets set by RPNs and NRPNs, one cell each.
const GENERATORS: Cells = ((1 << NRPN_SCALE.len()) - 1) << 15;

const fn generator(g: usize) -> Cells {
    1 << (15 + g)
}

/// What CC121 resets of the cells.
const RESET_ALL: Cells = CC6 | CC38 | CC98 | CC99 | CC100 | CC101 | GENERATORS;

/// Controllers with effects beyond their value: bank select, data entry,
/// the parameter numbers, Reset All Controllers.
fn structural(controller: u8) -> bool {
    matches!(controller, 0 | 32 | 6 | 38 | 98..=101 | 121)
}

/// A structural event, and the cells it wrote and read when the file sent
/// it.
#[derive(Clone, Copy)]
struct Logged {
    event: Event,
    writes: Cells,
    reads: Cells,
    keep: bool,
}

/// What a chase knows of a channel.
#[derive(Clone)]
struct Part {
    state: Channel,
    log: Vec<Logged>,
    /// The log's length when last pruned.
    pruned: usize,
}

impl Part {
    fn new(index: usize) -> Part {
        Part { state: Channel::new(index), log: Vec::new(), pruned: 0 }
    }

    fn push(&mut self, event: Event, (writes, reads): (Cells, Cells)) {
        self.log.push(Logged { event, writes, reads, keep: true });
        if self.log.len() > 2 * self.pruned + 32 {
            self.prune();
        }
    }

    /// Leaves out the events that write nothing read later: going back from
    /// the end, where every cell counts, an event counts if it writes a cell
    /// that counts; the cells it writes then count no more before it, and
    /// those it reads do.
    fn prune(&mut self) {
        let mut live = Cells::MAX;
        for e in self.log.iter_mut().rev() {
            e.keep = e.writes & live != 0;
            if e.keep {
                live = (live & !e.writes) | e.reads;
            }
        }
        self.log.retain(|e| e.keep);
        self.pruned = self.log.len();
    }

    /// Appends the events that set this channel's state on a reset one.
    fn emit(&self, channel: u8, style: BankStyle, out: &mut Vec<Item>) {
        let mut replay = Channel::new(channel as usize);
        for e in &self.log {
            out.push(Item::Event(e.event));
            apply(&mut replay, style, &e.event);
        }
        let s = &self.state;
        for controller in (0..120u8).filter(|&c| !structural(c)) {
            let value = s.cc[controller as usize];
            if replay.cc[controller as usize] != value {
                out.push(Item::Event(Event::ControlChange { channel, controller, value }));
                replay.cc[controller as usize] = value;
            }
        }
        if replay.pitch_bend != s.pitch_bend {
            out.push(Item::Event(Event::PitchBend { channel, value: s.pitch_bend }));
            replay.pitch_bend = s.pitch_bend;
        }
        if replay.channel_pressure != s.channel_pressure {
            out.push(Item::Event(Event::ChannelPressure { channel, pressure: s.channel_pressure }));
            replay.channel_pressure = s.channel_pressure;
        }
        for key in 0..128u8 {
            let pressure = s.key_pressure[key as usize];
            if replay.key_pressure[key as usize] != pressure {
                out.push(Item::Event(Event::PolyPressure { channel, key, pressure }));
                replay.key_pressure[key as usize] = pressure;
            }
        }
        debug_assert!(same(&replay, s), "the pruned events of channel {channel} do not give its state");
    }
}

/// Whether two channels have the same state, but the preset (which a chase
/// does not know) and the controllers of mode messages.
fn same(a: &Channel, b: &Channel) -> bool {
    a.cc[..120] == b.cc[..120]
        && (a.drum, a.bank, a.program) == (b.drum, b.bank, b.program)
        && (a.pitch_bend, a.channel_pressure) == (b.pitch_bend, b.channel_pressure)
        && a.key_pressure == b.key_pressure
        && (a.bend_range, a.mod_depth_range) == (b.bend_range, b.mod_depth_range)
        && a.gen_offsets == b.gen_offsets
        && (a.nrpn_active, a.nrpn_select) == (b.nrpn_active, b.nrpn_select)
}

/// Applies a structural event to `c` as the synthesizer does, bank selects
/// following `style`, and returns the cells it wrote and those it read.
fn apply(c: &mut Channel, style: BankStyle, event: &Event) -> (Cells, Cells) {
    match *event {
        Event::ControlChange { controller, value, .. } => control(c, style, controller, value),
        Event::ProgramChange { program, .. } => {
            // The preset is found from the bank, or the drum bank.
            c.program = program;
            (PROGRAM, BANK | DRUM)
        }
        Event::SysEx(sysex) => match classify_sysex(sysex.data()) {
            SysExKind::RhythmPart { drum, .. } => {
                c.drum = drum;
                c.cc[121] = 0;
                c.init_controllers(true);
                c.bank = if drum { crate::synth::DRUM_BANK } else { 0 };
                c.program = 0;
                (DRUM | BANK | PROGRAM | RESET_ALL, 0)
            }
            _ => (0, 0),
        },
        _ => (0, 0),
    }
}

/// A structural controller change, as `Synth::control_change` does it.
fn control(c: &mut Channel, style: BankStyle, controller: u8, value: u8) -> (Cells, Cells) {
    c.cc[controller as usize] = value;
    match controller {
        0 => match style {
            BankStyle::Gs => {
                c.bank = if c.drum { crate::synth::DRUM_BANK + u16::from(value) } else { value.into() };
                (CC0 | BANK, DRUM)
            }
            BankStyle::Gm => (CC0, 0),
            BankStyle::Gm2 => match value {
                120 => {
                    c.drum = true;
                    c.bank = crate::synth::DRUM_BANK;
                    (CC0 | DRUM | BANK, 0)
                }
                121 => {
                    c.drum = false;
                    c.bank = c.cc[32].into();
                    (CC0 | DRUM | BANK, CC32)
                }
                _ => (CC0, 0),
            },
        },
        32 => match style {
            BankStyle::Gm2 if !c.drum => {
                c.bank = value.into();
                (CC32 | BANK, DRUM)
            }
            BankStyle::Gm2 => (CC32, DRUM),
            _ => (CC32, 0),
        },
        6 | 38 => data_entry(c, style, controller),
        98 => {
            c.cc[6] = 0;
            c.cc[38] = 0;
            c.nrpn_active = true;
            if c.cc[99] != 120 {
                return (CC98 | CC6 | CC38 | ACTIVE, CC99);
            }
            match value {
                100 => c.nrpn_select += 100,
                101 => c.nrpn_select += 1000,
                102 => c.nrpn_select += 10000,
                v if v < 100 => c.nrpn_select += u32::from(v),
                _ => {}
            }
            c.nrpn_select = c.nrpn_select.min(100_000);
            (CC98 | CC6 | CC38 | ACTIVE | SELECT, CC99 | SELECT)
        }
        99 => {
            c.cc[98] = 0;
            c.cc[6] = 0;
            c.cc[38] = 0;
            c.nrpn_select = 0;
            c.nrpn_active = true;
            (CC99 | CC98 | CC6 | CC38 | SELECT | ACTIVE, 0)
        }
        100 | 101 => {
            c.nrpn_active = false;
            c.cc[6] = 0;
            c.cc[38] = 0;
            (if controller == 100 { CC100 } else { CC101 } | ACTIVE | CC6 | CC38, 0)
        }
        121 => {
            c.init_controllers(true);
            (RESET_ALL, 0)
        }
        _ => (0, 0),
    }
}

/// Data entry, as `Synth::data_entry` does it. What decides where the data
/// goes is read, as well as the data.
fn data_entry(c: &mut Channel, style: BankStyle, controller: u8) -> (Cells, Cells) {
    let (own, other) = if controller == 6 { (CC6, CC38) } else { (CC38, CC6) };
    let (msb, lsb) = (c.cc[6], c.cc[38]);
    let data = i32::from(msb) << 7 | i32::from(lsb);
    if c.nrpn_active {
        let decide = ACTIVE | CC99 | CC98;
        if c.cc[99] == 120 && c.cc[98] < 100 {
            // SoundFont NRPNs: on the MSB, with the LSB then.
            if controller == 38 {
                return (own, decide);
            }
            let g = c.nrpn_select as usize;
            let (mut writes, mut reads) = (own | SELECT, decide | SELECT);
            if let Some(&scale) = NRPN_SCALE.get(g) {
                c.gen_offsets[g] = f64::from((data - 8192).clamp(-8192, 8192)) * f64::from(scale);
                writes |= generator(g);
                reads |= CC38;
            }
            c.nrpn_select = 0;
            (writes, reads)
        } else {
            if c.cc[99] == 1 && style == BankStyle::Gs {
                // GS vibrato NRPNs set controllers, which are sent after.
                let target = match c.cc[98] {
                    8 => Some(76),
                    9 => Some(77),
                    10 => Some(78),
                    _ => None,
                };
                if let Some(cc) = target {
                    c.cc[cc] = msb;
                }
            }
            (own, decide)
        }
    } else if c.cc[101] == 0 {
        let decide = ACTIVE | CC101 | CC100;
        match c.cc[100] {
            0 => {
                c.bend_range = f64::from(msb) + f64::from(lsb) / 100.0;
                (own | BEND_RANGE, decide | other)
            }
            1 => {
                c.gen_offsets[52] = f64::from(data - 8192) * (100.0 / 8192.0);
                (own | generator(52), decide | other)
            }
            2 => {
                c.gen_offsets[51] = f64::from(msb) - 64.0;
                (own | generator(51), if controller == 38 { decide | CC6 } else { decide })
            }
            5 if !c.drum => {
                c.mod_depth_range = f64::from(msb) * 100.0 + f64::from(lsb) * 100.0 / 128.0;
                (own | MOD_DEPTH, decide | other | DRUM)
            }
            5 => (own, decide | DRUM),
            _ => (own, decide),
        }
    } else {
        (own, ACTIVE | CC101)
    }
}

/// The state events have set since the last reset message, if any.
#[derive(Clone)]
struct Chase {
    style: BankStyle,
    /// The last reset message, and the style it set.
    reset: Option<(BankStyle, SysEx)>,
    /// System exclusive messages that do nothing to the synthesizer, in
    /// order, for other hosts.
    sysex: Vec<SysEx>,
    parts: Vec<Part>,
}

impl Chase {
    fn new(style: BankStyle) -> Chase {
        Chase { style, reset: None, sysex: Vec::new(), parts: (0..16).map(Part::new).collect() }
    }

    fn feed(&mut self, event: &Event) {
        let channel = match *event {
            Event::SysEx(sysex) => {
                match classify_sysex(sysex.data()) {
                    SysExKind::Reset(style) => {
                        // It undoes what came before it.
                        *self = Chase::new(style);
                        self.reset = Some((style, sysex));
                    }
                    SysExKind::RhythmPart { channel, .. } if self.style == BankStyle::Gs => {
                        let part = &mut self.parts[channel as usize];
                        let cells = apply(&mut part.state, self.style, event);
                        part.push(*event, cells);
                    }
                    _ => self.sysex.push(sysex),
                }
                return;
            }
            Event::NoteOn { .. } | Event::NoteOff { .. } => return,
            Event::ControlChange { channel, .. }
            | Event::ProgramChange { channel, .. }
            | Event::PitchBend { channel, .. }
            | Event::ChannelPressure { channel, .. }
            | Event::PolyPressure { channel, .. } => channel,
        };
        let style = self.style;
        let Some(part) = self.parts.get_mut(channel as usize) else { return };
        let c = &mut part.state;
        // Data out of range is taken as the synthesizer takes it.
        match *event {
            Event::ControlChange { controller, value, .. } => {
                let (controller, value) = (controller.min(127), value.min(127));
                if structural(controller) {
                    let cells = control(c, style, controller, value);
                    part.push(Event::ControlChange { channel, controller, value }, cells);
                } else if controller < 120 {
                    c.cc[controller as usize] = value;
                }
                // The other mode messages set no state.
            }
            Event::ProgramChange { program, .. } => {
                let event = Event::ProgramChange { channel, program: program.min(127) };
                let cells = apply(c, style, &event);
                part.push(event, cells);
            }
            Event::PitchBend { value, .. } => c.pitch_bend = value.min(16383),
            Event::ChannelPressure { pressure, .. } => c.channel_pressure = pressure.min(127),
            Event::PolyPressure { key, pressure, .. } => c.key_pressure[key.min(127) as usize] = pressure.min(127),
            _ => {}
        }
    }

    /// The items setting the state: the reset, the system exclusive
    /// messages in their order, then each channel's events.
    fn finish(mut self) -> Vec<Item> {
        let mut out = vec![Item::Reset(self.reset)];
        out.extend(self.sysex.iter().map(|&sysex| Item::Event(Event::SysEx(sysex))));
        for (channel, part) in self.parts.iter_mut().enumerate() {
            part.prune();
            part.emit(channel as u8, self.style, &mut out);
        }
        out
    }
}
