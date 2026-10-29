//! SoundFont 2 files: loading, the data model, and resolving which regions
//! a note plays with which generators and modulators.
//!
//! The data model is read through methods; its fields are private, so that
//! later versions can keep more of the file without breaking anyone.

mod generator;
mod modulator;
mod parse;
mod resolve;

use std::fmt;
use std::io::Read;

use crate::Error;

pub use generator::GeneratorType;
pub(crate) use generator::{GENERATOR_COUNT, generator_range};
#[cfg(any(test, feature = "testkit"))]
pub(crate) use modulator::DEFAULT_MODULATORS;
pub use modulator::{ModList, Modulator, Source};
pub use resolve::Region;

/// A loaded SoundFont 2 file. Immutable; share it in an
/// [`Arc`](std::sync::Arc).
///
/// Its [`Debug`](fmt::Debug) output is a summary (the name, and how many
/// presets, instruments, samples and sample points there are), not the
/// data.
pub struct SoundFont {
    pub(crate) info: Info,
    pub(crate) presets: Vec<Preset>,
    pub(crate) instruments: Vec<Instrument>,
    pub(crate) samples: Vec<Sample>,
    /// The sample data, 16-bit, with the low byte of 24-bit samples in
    /// `data24` when the file has them.
    pub(crate) data: Box<[i16]>,
    pub(crate) data24: Option<Box<[u8]>>,
    /// The presets' bank, program and index, sorted: a program change
    /// finds its preset by a binary search.
    pub(crate) by_program: Box<[(u16, u16, u32)]>,
}

impl SoundFont {
    /// Reads a SoundFont 2 file. Any reader works, buffered or not, and
    /// reads of any size; the file is read once from its start.
    pub fn read<R: Read>(reader: R) -> Result<SoundFont, Error> {
        parse::read(reader)
    }

    /// Reads a SoundFont 2 file from memory.
    pub fn from_bytes(bytes: &[u8]) -> Result<SoundFont, Error> {
        parse::read(bytes)
    }

    /// The INFO list: the version and the descriptive texts.
    pub fn info(&self) -> &Info {
        &self.info
    }

    /// The presets, in the file's order. [`find_preset`](Self::find_preset)
    /// gives, and [`regions`](Self::regions) takes, indices into it.
    pub fn presets(&self) -> &[Preset] {
        &self.presets
    }

    /// The instruments, in the file's order.
    pub fn instruments(&self) -> &[Instrument] {
        &self.instruments
    }

    /// The sample headers, in the file's order.
    pub fn samples(&self) -> &[Sample] {
        &self.samples
    }

    /// The sample data (the smpl chunk), 16-bit; [`Sample`] positions index
    /// into it.
    pub fn sample_data(&self) -> &[i16] {
        &self.data
    }

    /// The low bytes of 24-bit sample data (the sm24 chunk), one per sample
    /// point, when the file has a valid one (§6.2).
    pub fn sample_data24(&self) -> Option<&[u8]> {
        self.data24.as_deref()
    }

    /// The index of the preset of `bank` and `program`, if there is one; the
    /// first, when the file has several (§7.2).
    pub fn find_preset(&self, bank: u16, program: u8) -> Option<usize> {
        let key = (bank, u16::from(program));
        let i = self.by_program.partition_point(|&(b, p, _)| (b, p) < key);
        match self.by_program.get(i) {
            Some(&(b, p, index)) if (b, p) == key => Some(index as usize),
            _ => None,
        }
    }

    /// The regions a note of `key` and `velocity` plays with the preset of
    /// index `preset`: every pair of a preset zone and a zone of its
    /// instrument that both contain the key and the velocity, in the file's
    /// order. Zones playing ROM samples are left out. A preset index out of
    /// range gives none. Does not allocate.
    pub fn regions(&self, preset: usize, key: u8, velocity: u8) -> impl Iterator<Item = Region<'_>> {
        resolve::regions(self, preset, key, velocity)
    }

    /// The zones [`regions`](Self::regions) goes through for `preset`.
    pub(crate) fn pairs(&self, preset: usize) -> usize {
        self.presets.get(preset).map_or(0, |p| p.pairs)
    }
}

impl fmt::Debug for SoundFont {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SoundFont")
            .field("name", &self.info.name)
            .field("version", &self.info.version)
            .field("presets", &self.presets.len())
            .field("instruments", &self.instruments.len())
            .field("samples", &self.samples.len())
            .field("sample_points", &self.data.len())
            .field("is_24_bit", &self.data24.is_some())
            .finish()
    }
}

/// The INFO list: the file's version and its descriptive texts.
#[derive(Clone, Debug, Default)]
pub struct Info {
    pub(crate) version: (u16, u16),
    pub(crate) sound_engine: String,
    pub(crate) name: String,
    pub(crate) rom_name: Option<String>,
    pub(crate) rom_version: Option<(u16, u16)>,
    pub(crate) creation_date: Option<String>,
    pub(crate) author: Option<String>,
    pub(crate) product: Option<String>,
    pub(crate) copyright: Option<String>,
    pub(crate) comment: Option<String>,
    pub(crate) software: Option<String>,
}

impl Info {
    /// The version of the SoundFont specification the file follows (ifil),
    /// such as `(2, 4)`.
    pub fn version(&self) -> (u16, u16) {
        self.version
    }

    /// The sound engine the file is made for (isng); "EMU8000" when the
    /// file names none (§5.2).
    pub fn sound_engine(&self) -> &str {
        &self.sound_engine
    }

    /// The SoundFont's name (INAM).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The sound ROM the file's ROM samples are in (irom).
    pub fn rom_name(&self) -> Option<&str> {
        self.rom_name.as_deref()
    }

    /// The version of that ROM (iver).
    pub fn rom_version(&self) -> Option<(u16, u16)> {
        self.rom_version
    }

    /// When the file was made (ICRD), as the file writes it.
    pub fn creation_date(&self) -> Option<&str> {
        self.creation_date.as_deref()
    }

    /// Who made the sounds (IENG).
    pub fn author(&self) -> Option<&str> {
        self.author.as_deref()
    }

    /// The product the file is made for (IPRD).
    pub fn product(&self) -> Option<&str> {
        self.product.as_deref()
    }

    /// The copyright notice (ICOP).
    pub fn copyright(&self) -> Option<&str> {
        self.copyright.as_deref()
    }

    /// Comments on the file (ICMT).
    pub fn comment(&self) -> Option<&str> {
        self.comment.as_deref()
    }

    /// The tools the file was made and edited with (ISFT).
    pub fn software(&self) -> Option<&str> {
        self.software.as_deref()
    }
}

/// A preset: what a program plays.
#[derive(Clone, Debug)]
pub struct Preset {
    pub(crate) name: String,
    pub(crate) bank: u16,
    pub(crate) program: u16,
    pub(crate) zones: Vec<PresetZone>,
    /// Its zones and the instrument zones they play: what finding its
    /// regions goes through.
    pub(crate) pairs: usize,
}

impl Preset {
    /// The name, as the file gives it.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The MIDI bank, as the file gives it: 128 for drum kits (§7.2).
    pub fn bank(&self) -> u16 {
        self.bank
    }

    /// The MIDI program, as the file gives it.
    pub fn program(&self) -> u16 {
        self.program
    }

    /// How many zones the preset has, not counting its global zone, which is
    /// merged into the others.
    pub fn zone_count(&self) -> usize {
        self.zones.len()
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PresetZone {
    /// The global zone's generators and modulators, with the zone's own
    /// in their place (resolved at load time).
    pub(crate) zone: Zone,
    pub(crate) instrument: usize,
}

/// An instrument: the samples a preset's zones play.
#[derive(Clone, Debug)]
pub struct Instrument {
    pub(crate) name: String,
    pub(crate) zones: Vec<InstrumentZone>,
}

impl Instrument {
    /// The name, as the file gives it.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// How many zones the instrument has, not counting its global zone,
    /// which is merged into the others.
    pub fn zone_count(&self) -> usize {
        self.zones.len()
    }
}

#[derive(Clone, Debug)]
pub(crate) struct InstrumentZone {
    /// The global zone's generators with the zone's own in their place,
    /// and the default modulators with the global zone's and the zone's own
    /// in their place (resolved at load time).
    pub(crate) zone: Zone,
    pub(crate) sample: usize,
    /// Whether it has anything to play: its sample has two points or more
    /// in the data and is not in ROM, and its offset generators leave some
    /// of them. Regions of zones that have not are left out.
    pub(crate) plays: bool,
}

/// The generators and modulators of a zone.
#[derive(Clone, Debug)]
pub(crate) struct Zone {
    /// The value of each generator the zone sets.
    pub(crate) generators: [Option<i16>; GENERATOR_COUNT],
    pub(crate) modulators: Vec<Modulator>,
}

/// A sample header: where a sample lies in the sample data, and how to
/// play it. Positions are in sample points from the start of the data,
/// as the file gives them, unchecked.
#[derive(Clone, Debug)]
pub struct Sample {
    pub(crate) name: String,
    pub(crate) start: u32,
    pub(crate) end: u32,
    pub(crate) loop_start: u32,
    pub(crate) loop_end: u32,
    pub(crate) sample_rate: u32,
    pub(crate) original_pitch: u8,
    pub(crate) pitch_correction: i8,
    pub(crate) link: u16,
    pub(crate) kind: SampleKind,
}

impl Sample {
    /// Whether it has two points or more within `data_len` points of data
    /// and is not in ROM: with fewer, a voice has nothing to play.
    pub(crate) fn plays(&self, data_len: usize) -> bool {
        let len = data_len.min(u32::MAX as usize) as u32;
        let start = self.start.min(len);
        let end = self.end.clamp(start, len);
        self.kind != SampleKind::Rom && end - start >= 2
    }

    /// The name, as the file gives it.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The first point.
    pub fn start(&self) -> u32 {
        self.start
    }

    /// The point after the last.
    pub fn end(&self) -> u32 {
        self.end
    }

    /// The first point of the loop.
    pub fn loop_start(&self) -> u32 {
        self.loop_start
    }

    /// The point after the last of the loop.
    pub fn loop_end(&self) -> u32 {
        self.loop_end
    }

    /// In Hz, as the file gives it.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// The key the sample sounds at; 60 when the file gives 255
    /// (unpitched) or another value above 127 (§7.10).
    pub fn original_pitch(&self) -> u8 {
        self.original_pitch
    }

    /// In cents.
    pub fn pitch_correction(&self) -> i8 {
        self.pitch_correction
    }

    /// The index of the other sample of a stereo pair, or the next of a
    /// linked chain, as the file gives it.
    pub fn link(&self) -> u16 {
        self.link
    }

    /// What the sample is.
    pub fn kind(&self) -> SampleKind {
        self.kind
    }
}

/// What a sample is (the sample type, §7.10).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SampleKind {
    /// A sample of one channel.
    Mono,
    /// The left channel of a stereo pair; [`link`](Sample::link) is the right.
    Left,
    /// The right channel of a stereo pair; [`link`](Sample::link) is the left.
    Right,
    /// One of a chain of linked samples, which the specification leaves
    /// undefined.
    Linked,
    /// A sample in a ROM this library does not have (bit 15 of the sample
    /// type); [`SoundFont::regions`] leaves out the zones playing it.
    Rom,
}

#[cfg(all(test, feature = "testkit"))]
mod tests;
