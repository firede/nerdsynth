//! The state of a MIDI channel: controllers, pitch bend, pressures, RPNs,
//! the program and bank, and the generator offsets set by RPNs and NRPNs.
//! The voices read it through [`Channel::version`], which changes whenever
//! anything a modulator may read changes.

use crate::soundfont::GENERATOR_COUNT;

/// How bank select messages choose a bank (FluidSynth's
/// `synth.midi-bank-select`), set by the GM, GM2 and GS reset messages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BankStyle {
    /// CC0 is the bank, CC32 is ignored; on a drum channel the bank is 128
    /// plus CC0. The default.
    Gs,
    /// Bank selects are ignored.
    Gm,
    /// CC0 120 makes a drum channel, 121 a melodic one; CC32 is the bank of a
    /// melodic channel.
    Gm2,
}

/// The SoundFont bank of drum kits.
pub(crate) const DRUM_BANK: u16 = 128;

#[derive(Clone, Debug)]
pub(crate) struct Channel {
    pub(crate) cc: [u8; 128],
    pub(crate) drum: bool,
    /// The bank the next program change selects from.
    pub(crate) bank: u16,
    pub(crate) program: u8,
    /// The preset playing, an index into the SoundFont's presets.
    pub(crate) preset: Option<usize>,
    pub(crate) pitch_bend: u16,
    pub(crate) channel_pressure: u8,
    pub(crate) key_pressure: [u8; 128],
    /// RPN 0, in semitones (the LSB in cents).
    pub(crate) bend_range: f64,
    /// RPN 5, in cents.
    pub(crate) mod_depth_range: f64,
    /// Offsets to the voices' generators: RPN 1 and 2 (fine and coarse
    /// tune) and SoundFont NRPNs.
    pub(crate) gen_offsets: [f64; GENERATOR_COUNT],
    /// Whether data entry goes to an NRPN rather than an RPN.
    pub(crate) nrpn_active: bool,
    /// The SoundFont NRPN being selected (§9.6).
    pub(crate) nrpn_select: u32,
    /// The note id the sostenuto pedal was pressed at: notes before it are
    /// held.
    pub(crate) sostenuto_order: u64,
    pub(crate) version: u64,
}

pub(crate) const SUSTAIN: u8 = 64;
pub(crate) const SOSTENUTO: u8 = 66;

impl Channel {
    pub(crate) fn new(index: usize) -> Channel {
        let mut c = Channel {
            cc: [0; 128],
            drum: false,
            bank: 0,
            program: 0,
            preset: None,
            pitch_bend: 8192,
            channel_pressure: 0,
            key_pressure: [0; 128],
            bend_range: 2.0,
            mod_depth_range: 50.0,
            gen_offsets: [0.0; GENERATOR_COUNT],
            nrpn_active: false,
            nrpn_select: 0,
            sostenuto_order: 0,
            version: 0,
        };
        c.reset(index);
        c
    }

    /// The channel's state after a system reset (FluidSynth's
    /// `fluid_channel_reset`); the caller chooses the preset.
    pub(crate) fn reset(&mut self, index: usize) {
        self.drum = index == 9;
        self.bank = if self.drum { DRUM_BANK } else { 0 };
        self.program = 0;
        self.preset = None;
        self.nrpn_select = 0;
        self.nrpn_active = false;
        self.sostenuto_order = 0;
        self.init_controllers(false);
    }

    /// FluidSynth's `fluid_channel_init_ctrl`. With `reset_all_controllers`,
    /// the response to CC121 as in MIDI RP-015: volume, pan, balance, bank,
    /// the effect depths and sound controllers, the bend range and the
    /// modulation depth range keep their values.
    pub(crate) fn init_controllers(&mut self, reset_all_controllers: bool) {
        self.channel_pressure = 0;
        self.pitch_bend = 8192;
        self.gen_offsets = [0.0; GENERATOR_COUNT];
        if reset_all_controllers {
            for i in 0..120 {
                let keep = matches!(i, 0 | 32 | 7 | 39 | 10 | 42 | 8 | 40 | 70..=79 | 91..=95);
                if !keep {
                    self.cc[i] = 0;
                }
            }
        } else {
            self.cc = [0; 128];
        }
        self.key_pressure = [0; 128];
        self.cc[100] = 127;
        self.cc[101] = 127;
        self.cc[98] = 127;
        self.cc[99] = 127;
        self.cc[11] = 127;
        self.cc[43] = 127;
        if !reset_all_controllers {
            self.bend_range = 2.0;
            self.mod_depth_range = 50.0;
            for i in 70..=79 {
                self.cc[i] = 64;
            }
            self.cc[7] = 100;
            self.cc[39] = 0;
            self.cc[10] = 64;
            self.cc[42] = 0;
            self.cc[8] = 64;
            self.cc[40] = 0;
        }
        self.touch();
    }

    /// Marks the state changed for the voices.
    pub(crate) fn touch(&mut self) {
        self.version = self.version.wrapping_add(1);
    }

    pub(crate) fn sustained(&self) -> bool {
        self.cc[SUSTAIN as usize] >= 64
    }

    pub(crate) fn sostenuto(&self) -> bool {
        self.cc[SOSTENUTO as usize] >= 64
    }

    /// The state to compare in tests: all but the controllers of the mode
    /// messages (120–127), the sostenuto pedal's note order and the
    /// version.
    #[cfg(test)]
    pub(crate) fn state(&self) -> State {
        State {
            cc: self.cc[..120].to_vec(),
            drum: self.drum,
            bank: self.bank,
            program: self.program,
            preset: self.preset,
            pitch_bend: self.pitch_bend,
            channel_pressure: self.channel_pressure,
            key_pressure: self.key_pressure.to_vec(),
            bend_range: self.bend_range,
            mod_depth_range: self.mod_depth_range,
            gen_offsets: self.gen_offsets.to_vec(),
            nrpn_active: self.nrpn_active,
            nrpn_select: self.nrpn_select,
        }
    }
}

/// A channel's state, as [`Channel::state`] gives it.
#[cfg(test)]
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct State {
    pub(crate) cc: Vec<u8>,
    pub(crate) drum: bool,
    pub(crate) bank: u16,
    pub(crate) program: u8,
    pub(crate) preset: Option<usize>,
    pub(crate) pitch_bend: u16,
    pub(crate) channel_pressure: u8,
    pub(crate) key_pressure: Vec<u8>,
    pub(crate) bend_range: f64,
    pub(crate) mod_depth_range: f64,
    pub(crate) gen_offsets: Vec<f64>,
    pub(crate) nrpn_active: bool,
    pub(crate) nrpn_select: u32,
}
