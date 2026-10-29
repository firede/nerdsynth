//! What each MIDI event does: notes, controllers, programs, RPNs and NRPNs,
//! and the GM, GM2 and GS system exclusive messages. After FluidSynth 2.6's
//! `src/synth/fluid_synth.c` and `fluid_chan.c`.

use std::sync::Arc;

use super::channel::{BankStyle, DRUM_BANK, SOSTENUTO, SUSTAIN};
use super::queue::Queued;
use super::voice::Status;
use super::{Synth, budget};
use crate::Event;
use crate::event::SysEx;
use crate::soundfont::GeneratorType as G;

/// FluidSynth's scale of each generator's SoundFont NRPN (§9.6): the data,
/// less 8192, times this (`fluid_gen_info[].nrpn_scale`).
pub(crate) const NRPN_SCALE: [u8; 61] = [
    1, 1, 1, 1, 1, 2, 2, 2, 2, 1, 2, 2, 1, 1, 0, 1, 1, 1, 0, 0, 0, 2, 4, 2, 4, 2, 2, 2, 2, 1, 2, 1, 1, 2, 2, 2, 2, 1,
    2, 1, 1, 0, 0, 0, 0, 1, 0, 1, 1, 0, 1, 1, 1, 0, 0, 0, 1, 0, 0, 0, 0,
];

/// The device IDs a system exclusive message may name: FluidSynth's default
/// `synth.device-id` (16) and "all" (127).
fn device_ok(id: u8) -> bool {
    id == 0x10 || id == 0x7F
}

/// What a system exclusive message does to the synthesizer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SysExKind {
    /// GM or GM2 System On, GS Reset or "exit GS mode": every channel resets
    /// and bank selects follow the system.
    Reset(BankStyle),
    /// GS "use for rhythm part" for the part playing `channel`: it becomes a
    /// drum channel or a melodic one, if bank selects follow GS.
    RhythmPart { channel: u8, drum: bool },
    /// Nothing.
    Other,
}

/// What the system exclusive message of `d`, the bytes between 0xF0 and
/// 0xF7, does: the one judgement shared by the synthesizer and a
/// sequencer's chase.
pub(crate) fn classify_sysex(d: &[u8]) -> SysExKind {
    if d.len() < 4 || !device_ok(d[1]) {
        return SysExKind::Other;
    }
    match (d[0], d[2], d[3]) {
        // Universal non-real-time, General MIDI: GM 1 or GM 2 System On.
        (0x7E, 0x09, 0x01) => SysExKind::Reset(BankStyle::Gm),
        (0x7E, 0x09, 0x03) => SysExKind::Reset(BankStyle::Gm2),
        // Roland, GS, DT1.
        (0x41, 0x42, 0x12) => gs_data_set(d),
        _ => SysExKind::Other,
    }
}

/// A Roland GS data set (DT1): address, data and checksum.
fn gs_data_set(d: &[u8]) -> SysExKind {
    if d.len() < 9 {
        return SysExKind::Other;
    }
    let address = u32::from(d[4]) << 16 | u32::from(d[5]) << 8 | u32::from(d[6]);
    let body = &d[4..d.len() - 1];
    let sum = body.iter().fold(0u32, |s, b| s + u32::from(*b));
    if (0x80 - (sum & 0x7F)) & 0x7F != u32::from(d[d.len() - 1]) {
        return SysExKind::Other;
    }
    let data = &d[7..d.len() - 1];
    if address == 0x40_007F {
        // GS Reset (0) or "exit GS mode" (0x7F).
        return match data {
            [0x00] => SysExKind::Reset(BankStyle::Gs),
            [0x7F] => SysExKind::Reset(BankStyle::Gm),
            _ => SysExKind::Other,
        };
    }
    if address & 0xFF_F0FF == 0x40_1015 {
        // Use for rhythm part: 0 off, 1 or 2 a drum part.
        let [mode] = data else { return SysExKind::Other };
        if *mode > 2 {
            return SysExKind::Other;
        }
        let part = (address >> 8 & 0x0F) as u8;
        let channel = if part == 0 {
            9
        } else if part < 10 {
            part - 1
        } else {
            part
        };
        return SysExKind::RhythmPart { channel, drum: *mode != 0 };
    }
    SysExKind::Other
}

impl Synth {
    /// Handles a queued item now.
    pub(super) fn handle_queued(&mut self, item: Queued) {
        self.budget.spend(budget::EVENT);
        match item {
            Queued::Midi(event) => self.handle(event),
            // A sequencer's jump: its channels and bank selects as a reset
            // leaves them, the voices and the effects going on.
            Queued::ResetChannels(style, channels) => {
                self.budget.spend(budget::RESET);
                self.bank_style = style;
                self.reset_channels_of(channels);
            }
        }
    }

    /// Handles an event now.
    pub(super) fn handle(&mut self, event: Event) {
        match event {
            Event::NoteOff { channel, key } if channel < 16 => self.note_off(channel, key.min(127)),
            Event::NoteOn { channel, key, velocity } if channel < 16 => {
                if velocity == 0 {
                    self.note_off(channel, key.min(127));
                } else {
                    self.note_on(channel, key.min(127), velocity.min(127));
                }
            }
            Event::PolyPressure { channel, key, pressure } if channel < 16 => {
                self.channels[channel as usize].key_pressure[key.min(127) as usize] = pressure.min(127);
            }
            Event::ControlChange { channel, controller, value } if channel < 16 => {
                self.control_change(channel, controller.min(127), value.min(127));
            }
            Event::ProgramChange { channel, program } if channel < 16 => {
                self.program_change(channel as usize, program.min(127));
            }
            Event::ChannelPressure { channel, pressure } if channel < 16 => {
                let c = &mut self.channels[channel as usize];
                c.channel_pressure = pressure.min(127);
                c.touch();
            }
            Event::PitchBend { channel, value } if channel < 16 => {
                let c = &mut self.channels[channel as usize];
                c.pitch_bend = value.min(16383);
                c.touch();
            }
            Event::SysEx(sysex) => self.sysex(&sysex),
            _ => {}
        }
    }

    fn note_on(&mut self, channel: u8, key: u8, velocity: u8) {
        let ch = channel as usize;
        let Some(preset) = self.channels[ch].preset else { return };
        // A note on releases the voices of the same key on the channel, as a
        // note off would (FluidSynth's
        // fluid_synth_release_voice_on_same_note_LOCAL).
        for i in 0..self.voices.len() {
            let v = &self.voices[i];
            if v.is_live() && v.channel == channel && v.key == key && v.status != Status::Released {
                self.note_off_voice(i);
            }
        }
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        let sound_font = Arc::clone(&self.sound_font);
        let data_len = sound_font.sample_data().len();
        let max_voices = self.config.max_voices;
        self.budget
            .spend(budget::NOTE_ON_SLOT * self.voices.len() as i64 + budget::PAIR * sound_font.pairs(preset) as i64);
        self.alloc.begin(&self.voices);
        // No more regions than voices: the others would only take the
        // first ones' slots, and a note on's work stays bounded.
        let mut started = 0;
        for region in sound_font.regions(preset, key, velocity) {
            if started == max_voices {
                break;
            }
            let slot = self.alloc.slot(&mut self.voices, max_voices, self.now, self.ctx.sample_rate, &self.channels);
            let v = &mut self.voices[slot];
            let ok = v.start(&region, data_len, channel, key, velocity, id, preset, self.now, &self.ctx);
            self.alloc.used(slot, ok);
            if ok {
                started += 1;
                self.budget.spend(budget::VOICE + budget::VOICE_MODULATOR * v.modulators() as i64);
                if v.exclusive_class != 0 {
                    self.alloc.classes.push(v.exclusive_class);
                }
            }
        }
        // E-05: an exclusive class stops the other notes of its class on
        // the channel and preset, and only those of other note ons.
        let classes = &mut self.alloc.classes;
        if !classes.is_empty() {
            classes.sort_unstable();
            classes.dedup();
            let c = &self.channels[ch];
            for v in self.voices.iter_mut() {
                if v.channel == channel
                    && v.preset == preset
                    && v.id != id
                    && v.exclusive_class != 0
                    && classes.binary_search(&v.exclusive_class).is_ok()
                {
                    if v.is_live() {
                        self.budget.spend(budget::CUT + budget::CUT_MODULATOR * v.modulators() as i64);
                    }
                    v.cut(c, &self.ctx);
                }
            }
        }
    }

    fn note_off(&mut self, channel: u8, key: u8) {
        self.spend_scan();
        for i in 0..self.voices.len() {
            let v = &self.voices[i];
            if v.is_live() && v.channel == channel && v.key == key && v.status == Status::On {
                self.note_off_voice(i);
            }
        }
    }

    /// A note off for a voice: held by the sostenuto pedal if it was
    /// pressed after the note started, by the hold pedal if down, else
    /// released (FluidSynth's fluid_voice_noteoff).
    fn note_off_voice(&mut self, i: usize) {
        let v = &mut self.voices[i];
        let c = &self.channels[v.channel as usize & 15];
        if c.sostenuto() && v.id < c.sostenuto_order {
            v.status = Status::Sostenuto;
        } else if c.sustained() {
            v.status = Status::Sustained;
        } else {
            v.release(&self.ctx);
        }
    }

    /// The work of going through the voice slots.
    fn spend_scan(&mut self) {
        self.budget.spend(budget::SLOT * self.voices.len() as i64);
    }

    /// Releases the voices of `channel` in `status`.
    fn release_where(&mut self, channel: u8, status: Status) {
        self.spend_scan();
        for v in self.voices.iter_mut() {
            if v.is_live() && v.channel == channel && v.status == status {
                v.release(&self.ctx);
            }
        }
    }

    fn all_notes_off(&mut self, channel: u8) {
        self.spend_scan();
        for i in 0..self.voices.len() {
            let v = &self.voices[i];
            if v.is_live() && v.channel == channel && v.status != Status::Released {
                self.note_off_voice(i);
            }
        }
    }

    fn all_sound_off(&mut self, channel: Option<u8>) {
        self.spend_scan();
        for v in self.voices.iter_mut() {
            if channel.is_none_or(|c| v.channel == c) {
                v.fade();
            }
        }
    }

    fn control_change(&mut self, channel: u8, controller: u8, value: u8) {
        let ch = channel as usize;
        let style = self.bank_style;
        let c = &mut self.channels[ch];
        let previous = c.cc[controller as usize];
        c.cc[controller as usize] = value;
        match controller {
            0 => match style {
                BankStyle::Gs => c.bank = if c.drum { DRUM_BANK + u16::from(value) } else { value.into() },
                BankStyle::Gm => {}
                BankStyle::Gm2 => match value {
                    120 => {
                        c.drum = true;
                        c.bank = DRUM_BANK;
                    }
                    121 => {
                        c.drum = false;
                        c.bank = c.cc[32].into();
                    }
                    _ => {}
                },
            },
            32 => {
                if style == BankStyle::Gm2 && !c.drum {
                    c.bank = value.into();
                }
            }
            6 | 38 => self.data_entry(ch, controller),
            98 => {
                // NRPN LSB.
                c.cc[6] = 0;
                c.cc[38] = 0;
                if c.cc[99] == 120 {
                    match value {
                        100 => c.nrpn_select += 100,
                        101 => c.nrpn_select += 1000,
                        102 => c.nrpn_select += 10000,
                        v if v < 100 => c.nrpn_select += u32::from(v),
                        _ => {}
                    }
                    c.nrpn_select = c.nrpn_select.min(100_000);
                }
                c.nrpn_active = true;
            }
            99 => {
                // NRPN MSB.
                c.cc[98] = 0;
                c.cc[6] = 0;
                c.cc[38] = 0;
                c.nrpn_select = 0;
                c.nrpn_active = true;
            }
            100 | 101 => {
                // E-14: the RPN stays selected through NRPNs, which are kept
                // in CC98 and CC99.
                c.nrpn_active = false;
                c.cc[6] = 0;
                c.cc[38] = 0;
            }
            SUSTAIN => {
                if value < 64 {
                    self.release_where(channel, Status::Sustained);
                }
            }
            SOSTENUTO => {
                if value < 64 {
                    self.release_where(channel, Status::Sostenuto);
                } else if previous < 64 {
                    // Only when pressed: the notes sounding then are held,
                    // not those started while it is down (FluidSynth takes
                    // them again on every value of 64 or more).
                    c.sostenuto_order = self.next_id;
                }
            }
            120 => self.all_sound_off(Some(channel)),
            121 => {
                c.init_controllers(true);
                self.release_where(channel, Status::Sustained);
                self.release_where(channel, Status::Sostenuto);
            }
            // All Notes Off, and the mode messages (Omni Off, Omni On, Mono,
            // Poly), which imply it.
            123..=127 => self.all_notes_off(channel),
            _ => {}
        }
        self.channels[ch].touch();
    }

    /// Data entry (CC6, CC38) for the RPN or NRPN selected.
    fn data_entry(&mut self, ch: usize, controller: u8) {
        let style = self.bank_style;
        let c = &mut self.channels[ch];
        let (msb, lsb) = (c.cc[6], c.cc[38]);
        let data = i32::from(msb) << 7 | i32::from(lsb);
        if c.nrpn_active {
            if c.cc[99] == 120 && c.cc[98] < 100 {
                // SoundFont NRPNs (§9.6): processed on the MSB, after the LSB.
                if controller == 38 {
                    return;
                }
                let g = c.nrpn_select as usize;
                if let Some(&scale) = NRPN_SCALE.get(g) {
                    c.gen_offsets[g] = f64::from((data - 8192).clamp(-8192, 8192)) * f64::from(scale);
                }
                c.nrpn_select = 0;
            } else if c.cc[99] == 1 && style == BankStyle::Gs {
                // GS NRPNs for the vibrato, mapped to the sound controllers
                // as FluidSynth does.
                let cc = match c.cc[98] {
                    8 => 76,
                    9 => 77,
                    10 => 78,
                    _ => return,
                };
                c.cc[cc] = msb;
            }
        } else if c.cc[101] == 0 {
            match c.cc[100] {
                0 => c.bend_range = f64::from(msb) + f64::from(lsb) / 100.0,
                1 => c.gen_offsets[G::FineTune as usize] = f64::from(data - 8192) * (100.0 / 8192.0),
                2 => c.gen_offsets[G::CoarseTune as usize] = f64::from(msb) - 64.0,
                5 if !c.drum => c.mod_depth_range = f64::from(msb) * 100.0 + f64::from(lsb) * 100.0 / 128.0,
                _ => {}
            }
        }
    }

    /// Selects the preset of the channel's bank and `program`, falling back
    /// as FluidSynth does (E-12): on a drum channel to bank 128 and then to
    /// 128:0, on others to bank 0 and then to 0:0.
    pub(super) fn program_change(&mut self, ch: usize, program: u8) {
        let sf = &self.sound_font;
        let c = &mut self.channels[ch];
        let fallback = if c.drum { DRUM_BANK } else { 0 };
        c.program = program;
        c.preset = sf
            .find_preset(c.bank, program)
            .or_else(|| sf.find_preset(fallback, program))
            .or_else(|| sf.find_preset(fallback, 0));
        c.touch();
    }

    /// Every channel to its initial state, its program 0.
    pub(super) fn reset_channels(&mut self) {
        self.reset_channels_of(u16::MAX);
    }

    /// The channels of `mask`, bit `n` for channel `n`, to their initial
    /// state, their program 0.
    fn reset_channels_of(&mut self, mask: u16) {
        for i in (0..16).filter(|i| mask >> i & 1 != 0) {
            self.channels[i].reset(i);
            self.program_change(i, 0);
        }
    }

    /// GM, GM2 or GS System On: every sound stops, every channel resets, the
    /// effects clear, and bank selects follow the system (E-15).
    fn system_reset(&mut self, style: BankStyle) {
        self.all_sound_off(None);
        self.bank_style = style;
        self.reset_channels();
        self.budget.spend(budget::RESET);
        // FluidSynth clears the delay lines, keeping the modulation going.
        // Lines already silent are left as they are, as when skipped.
        if !self.reverb.is_silent() {
            self.reverb.clear_lines();
            self.budget.spend(budget::CLEAR_EFFECTS);
        }
        if !self.chorus.is_silent() {
            self.chorus.clear_lines();
            self.budget.spend(budget::CLEAR_EFFECTS);
        }
    }

    fn sysex(&mut self, sysex: &SysEx) {
        match classify_sysex(sysex.data()) {
            SysExKind::Reset(style) => self.system_reset(style),
            SysExKind::RhythmPart { channel, drum } if self.bank_style == BankStyle::Gs => {
                let ch = channel as usize;
                self.channels[ch].drum = drum;
                self.control_change(channel, 121, 0);
                self.channels[ch].bank = if drum { DRUM_BANK } else { 0 };
                self.program_change(ch, 0);
            }
            _ => {}
        }
    }
}
