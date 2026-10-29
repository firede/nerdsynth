//! Reading SoundFont 2 files (RIFF `sfbk`).
//!
//! The file is read once, front to back, chunk by chunk. Nothing is
//! allocated for what a size or a count claims: every list grows only as its
//! data actually arrives, so a short file claiming gigabytes costs little.
//! Every index between the lists is checked, and nothing here panics on any
//! input.

use std::collections::HashMap;
use std::io::{self, BufReader, Read};

use super::generator::GENERATOR_COUNT;
use super::modulator::{DEFAULT_MODULATORS, ModList, Modulator, Source};
use super::{Info, Instrument, InstrumentZone, Preset, PresetZone, Sample, SampleKind, SoundFont, Zone};
use crate::Error;
use crate::error::{Format, InvalidFile, Unsupported};

/// The most (preset zone, instrument zone) pairs a file may have, summed
/// over all its presets: each is a region some note may play. Real files
/// have far fewer (GeneralUser GS 2.0.3: see `tests/generaluser.rs`); a
/// file with more is rejected rather than letting one note start millions
/// of voices.
pub(crate) const MAX_REGIONS: usize = 1 << 20;

/// The most modulators all zones together may hold once the default
/// modulators and the global zones' are merged into each zone. Without a
/// bound, a global zone of 60000 modulators in an instrument of 60000 zones
/// would make billions of copies. Each zone now keeps at most
/// [`ModList::CAPACITY`] and a file has at most 65536 zones of each kind, so
/// this is a safeguard.
pub(crate) const MAX_ZONE_MODULATORS: usize = 1 << 22;

/// Text chunks in INFO are at most 256 bytes, ICMT at most 65536 (§5);
/// longer ones are cut there.
const MAX_TEXT: u64 = 65536;

/// Sample data is read in blocks of this many bytes, and its buffer starts
/// at this many samples at most.
const BLOCK: usize = 1 << 16;

type FourCc = [u8; 4];

pub(crate) fn read<R: Read>(reader: R) -> Result<SoundFont, Error> {
    let mut src = Input { reader: BufReader::with_capacity(BLOCK, reader) };

    let mut header = [0u8; 12];
    src.fill(&mut header, "the RIFF header")?;
    if &header[0..4] != b"RIFF" {
        return Err(invalid("not a RIFF file"));
    }
    if &header[8..12] != b"sfbk" {
        return Err(invalid(format!("a RIFF file of form {:?}, not sfbk", fourcc_str(&header[8..12]))));
    }
    let riff_size = u64::from(u32::from_le_bytes([header[4], header[5], header[6], header[7]]));
    let mut left = riff_size.checked_sub(4).ok_or_else(|| invalid("the RIFF chunk is too small"))?;

    let mut info: Option<RawInfo> = None;
    let mut sdta: Option<RawSdta> = None;
    let mut pdta: Option<RawPdta> = None;

    while left >= 8 {
        // A file whose RIFF size claims more than it holds is accepted when
        // it ends cleanly between two chunks.
        let Some((id, size)) = src.chunk_header_or_eof()? else { break };
        left -= 8;
        if size > left {
            return Err(invalid(format!("the {} chunk is larger than the RIFF chunk", fourcc_str(&id))));
        }
        let mut body = Body { src: &mut src, left: size, id };
        if &id == b"LIST" && size >= 4 {
            let mut kind = [0u8; 4];
            body.read(&mut kind)?;
            body.id = kind;
            match &kind {
                b"INFO" => set_once(&mut info, read_info(&mut body)?, "INFO")?,
                b"sdta" => set_once(&mut sdta, read_sdta(&mut body)?, "sdta")?,
                b"pdta" => set_once(&mut pdta, read_pdta(&mut body)?, "pdta")?,
                _ => {}
            }
        }
        body.finish()?;
        left -= size;
        if size % 2 == 1 && left > 0 {
            src.skip(1, "a pad byte")?;
            left -= 1;
        }
    }

    let raw_info = info.ok_or_else(|| invalid("the INFO list is missing"))?;
    let sdta = sdta.ok_or_else(|| invalid("the sdta list is missing"))?;
    let pdta = pdta.ok_or_else(|| invalid("the pdta list is missing"))?;

    let info = raw_info.info;
    let version = raw_info.version.ok_or_else(|| invalid("the ifil chunk is missing"))?;
    if version.0 == 3 {
        return Err(Error::Unsupported(Unsupported::CompressedSamples));
    }
    let data = sdta.smpl.unwrap_or_default();
    if data.len() >= 2 && data[0] == i16::from_le_bytes(*b"Og") && data[1] == i16::from_le_bytes(*b"gS") {
        return Err(Error::Unsupported(Unsupported::CompressedSamples));
    }
    // §6.2: sm24 counts only with smpl, from version 2.04, and when it has
    // exactly one byte per sample (plus a pad byte when that is odd).
    let data24 = match sdta.sm24 {
        Some(mut low)
            if !data.is_empty()
                && version >= (2, 4)
                && (low.len() == data.len() || (data.len() % 2 == 1 && low.len() == data.len() + 1)) =>
        {
            low.truncate(data.len());
            Some(low.into_boxed_slice())
        }
        _ => None,
    };

    let (presets, mut instruments, samples) = build(pdta)?;
    // Zones with nothing to play are left out of the regions once here,
    // rather than at every note on.
    for zone in instruments.iter_mut().flat_map(|i| i.zones.iter_mut()) {
        let sample = &samples[zone.sample];
        zone.plays = sample.plays(data.len()) && crate::synth::zone_plays(sample, &zone.zone.generators, data.len());
    }
    let mut by_program: Box<[(u16, u16, u32)]> =
        presets.iter().enumerate().map(|(i, p)| (p.bank, p.program, i as u32)).collect();
    by_program.sort_unstable();
    Ok(SoundFont { info, presets, instruments, samples, data: data.into_boxed_slice(), data24, by_program })
}

fn invalid(message: impl Into<String>) -> Error {
    Error::Invalid(InvalidFile::new(Format::SoundFont, message))
}

fn fourcc_str(id: &[u8]) -> String {
    id.iter().map(|&b| if b.is_ascii_graphic() || b == b' ' { b as char } else { '?' }).collect()
}

fn set_once<T>(slot: &mut Option<T>, value: T, what: &str) -> Result<(), Error> {
    if slot.is_some() {
        return Err(invalid(format!("the file has two {what} chunks")));
    }
    *slot = Some(value);
    Ok(())
}

/// The reader, with its errors mapped: running out of data is an invalid
/// file, other failures are I/O errors.
struct Input<R> {
    reader: BufReader<R>,
}

impl<R: Read> Input<R> {
    fn fill(&mut self, buf: &mut [u8], what: &str) -> Result<(), Error> {
        self.reader.read_exact(buf).map_err(|e| eof_to_invalid(e, what))
    }

    fn skip(&mut self, n: u64, what: &str) -> Result<(), Error> {
        let copied = io::copy(&mut (&mut self.reader).take(n), &mut io::sink())?;
        if copied < n {
            return Err(invalid(format!("the file ends inside {what}")));
        }
        Ok(())
    }

    /// Reads a chunk header, or gives `None` when the data ends before it.
    fn chunk_header_or_eof(&mut self) -> Result<Option<(FourCc, u64)>, Error> {
        let mut header = [0u8; 8];
        let mut got = 0;
        while got < header.len() {
            match self.reader.read(&mut header[got..]) {
                Ok(0) if got == 0 => return Ok(None),
                Ok(0) => return Err(invalid("the file ends inside a chunk header")),
                Ok(n) => got += n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(Some(split_header(&header)))
    }
}

fn eof_to_invalid(e: io::Error, what: &str) -> Error {
    if e.kind() == io::ErrorKind::UnexpectedEof { invalid(format!("the file ends inside {what}")) } else { e.into() }
}

fn split_header(header: &[u8; 8]) -> (FourCc, u64) {
    let id = [header[0], header[1], header[2], header[3]];
    (id, u64::from(u32::from_le_bytes([header[4], header[5], header[6], header[7]])))
}

/// The data of one chunk: reads cannot go past its end, and what is left
/// unread is skipped.
struct Body<'a, R> {
    src: &'a mut Input<R>,
    left: u64,
    /// The chunk's ID, or a list's type.
    id: FourCc,
}

impl<R: Read> Body<'_, R> {
    fn what(&self) -> String {
        format!("the {} chunk", fourcc_str(&self.id))
    }

    fn read(&mut self, buf: &mut [u8]) -> Result<(), Error> {
        if buf.len() as u64 > self.left {
            return Err(invalid(format!("{} is too short", self.what())));
        }
        let what = self.what();
        self.src.fill(buf, &what)?;
        self.left -= buf.len() as u64;
        Ok(())
    }

    fn finish(self) -> Result<(), Error> {
        let what = self.what();
        self.src.skip(self.left, &what)
    }

    /// Calls `each` with every sub-chunk of a list, the type of which has
    /// been read already. Sub-chunks larger than what is left of the list
    /// are an error; pad bytes after odd sizes are skipped, and so are a
    /// few stray bytes at the end of the list.
    fn chunks(&mut self, mut each: impl FnMut(&mut Body<'_, R>) -> Result<(), Error>) -> Result<(), Error> {
        let list = fourcc_str(&self.id);
        while self.left >= 8 {
            let mut header = [0u8; 8];
            self.read(&mut header)?;
            let (id, size) = split_header(&header);
            if size > self.left {
                return Err(invalid(format!("the {} chunk is larger than its {list} list", fourcc_str(&id))));
            }
            self.left -= size;
            let mut body = Body { src: &mut *self.src, left: size, id };
            each(&mut body)?;
            body.finish()?;
            if size % 2 == 1 && self.left > 0 {
                self.src.skip(1, "a pad byte")?;
                self.left -= 1;
            }
        }
        Ok(())
    }

    /// Reads the chunk as records of `N` bytes each.
    fn records<const N: usize, T>(&mut self, mut each: impl FnMut(&[u8; N]) -> T) -> Result<Vec<T>, Error> {
        if self.left % N as u64 != 0 {
            return Err(invalid(format!("the size of {} is not a multiple of {N}", self.what())));
        }
        // Grows as records arrive; the count is not trusted.
        let mut out = Vec::new();
        let mut record = [0u8; N];
        while self.left > 0 {
            self.read(&mut record)?;
            out.push(each(&record));
        }
        Ok(out)
    }

    /// Reads the chunk's bytes, at most `limit` of them.
    fn bytes(&mut self, limit: u64) -> Result<Vec<u8>, Error> {
        let mut out = Vec::new();
        let mut buf = [0u8; 4096];
        let mut left = self.left.min(limit);
        while left > 0 {
            let k = left.min(buf.len() as u64) as usize;
            self.read(&mut buf[..k])?;
            out.extend_from_slice(&buf[..k]);
            left -= k as u64;
        }
        Ok(out)
    }

    /// Reads the chunk as 16-bit samples, in blocks. The buffer grows with
    /// the data read (see [`grow`]) and never past the chunk's size, so a
    /// whole chunk ends in a buffer of exactly its samples.
    fn samples(&mut self) -> Result<Vec<i16>, Error> {
        let count = usize::try_from(self.left / 2).map_err(|_| invalid("the sample data is too large"))?;
        let mut out: Vec<i16> = Vec::with_capacity(count.min(BLOCK));
        let mut buf = vec![0u8; BLOCK];
        while out.len() < count {
            let n = (count - out.len()).min(BLOCK / 2);
            self.read(&mut buf[..2 * n])?;
            grow(&mut out, n, count);
            out.extend(buf[..2 * n].chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])));
        }
        Ok(out)
    }

    /// Reads the chunk's bytes the way [`Body::samples`] reads samples.
    fn sample_bytes(&mut self) -> Result<Vec<u8>, Error> {
        let count = usize::try_from(self.left).map_err(|_| invalid("the sample data is too large"))?;
        let mut out: Vec<u8> = Vec::with_capacity(count.min(BLOCK));
        let mut buf = vec![0u8; BLOCK];
        while out.len() < count {
            let n = (count - out.len()).min(BLOCK);
            self.read(&mut buf[..n])?;
            grow(&mut out, n, count);
            out.extend_from_slice(&buf[..n]);
        }
        Ok(out)
    }
}

/// Makes room for `n` more items of `total`. The capacity doubles until an
/// eighth of `total` has arrived, then goes to `total` in one step: the
/// copy that growing may make then costs at most an eighth more than the
/// data (P-SF-12), while a file that claims more than it holds costs at
/// most eight times what it holds (P-SF-03).
fn grow<T>(v: &mut Vec<T>, n: usize, total: usize) {
    if v.capacity() - v.len() < n {
        let target = if v.len() >= total / 8 { total } else { (2 * v.capacity()).max(v.len() + n).min(total) };
        v.reserve_exact(target - v.len());
    }
}

/// Text up to the first zero byte: UTF-8 when it is, else Latin-1, which
/// keeps every byte readable.
fn text(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    let bytes = &bytes[..end];
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_owned(),
        Err(_) => bytes.iter().map(|&b| b as char).collect(),
    }
}

struct RawInfo {
    info: Info,
    version: Option<(u16, u16)>,
}

fn version(bytes: &[u8]) -> Option<(u16, u16)> {
    match bytes {
        [a, b, c, d] => Some((u16::from_le_bytes([*a, *b]), u16::from_le_bytes([*c, *d]))),
        _ => None,
    }
}

fn read_info<R: Read>(list: &mut Body<'_, R>) -> Result<RawInfo, Error> {
    let mut info = Info { sound_engine: "EMU8000".into(), ..Info::default() };
    let mut file_version = None;
    list.chunks(|chunk| {
        let id = chunk.id;
        // Only the known chunks are read; the others are skipped unread.
        let known = matches!(
            &id,
            b"ifil" | b"iver" | b"isng" | b"INAM" | b"irom" | b"ICRD" | b"IENG" | b"IPRD" | b"ICOP" | b"ICMT" | b"ISFT"
        );
        if !known {
            return Ok(());
        }
        let bytes = chunk.bytes(MAX_TEXT)?;
        match &id {
            b"ifil" => {
                // §5.1: a missing or malformed ifil makes the file unsound.
                let v = version(&bytes).ok_or_else(|| invalid("the ifil chunk is not 4 bytes"))?;
                info.version = v;
                file_version = Some(v);
            }
            // §5.5: a malformed iver is ignored.
            b"iver" => info.rom_version = version(&bytes),
            b"isng" => {
                let engine = text(&bytes);
                if !engine.is_empty() {
                    info.sound_engine = engine;
                }
            }
            b"INAM" => info.name = text(&bytes),
            b"irom" => info.rom_name = Some(text(&bytes)),
            b"ICRD" => info.creation_date = Some(text(&bytes)),
            b"IENG" => info.author = Some(text(&bytes)),
            b"IPRD" => info.product = Some(text(&bytes)),
            b"ICOP" => info.copyright = Some(text(&bytes)),
            b"ICMT" => info.comment = Some(text(&bytes)),
            b"ISFT" => info.software = Some(text(&bytes)),
            _ => {}
        }
        Ok(())
    })?;
    Ok(RawInfo { info, version: file_version })
}

#[derive(Default)]
struct RawSdta {
    smpl: Option<Vec<i16>>,
    sm24: Option<Vec<u8>>,
}

fn read_sdta<R: Read>(list: &mut Body<'_, R>) -> Result<RawSdta, Error> {
    let mut sdta = RawSdta::default();
    list.chunks(|chunk| {
        match &chunk.id {
            b"smpl" => set_once(&mut sdta.smpl, chunk.samples()?, "smpl")?,
            b"sm24" => set_once(&mut sdta.sm24, chunk.sample_bytes()?, "sm24")?,
            _ => {}
        }
        Ok(())
    })?;
    Ok(sdta)
}

struct RawHeader {
    name: String,
    /// The preset's program, or 0 for an instrument.
    program: u16,
    bank: u16,
    bag: u16,
}

struct RawSample {
    name: String,
    start: u32,
    end: u32,
    loop_start: u32,
    loop_end: u32,
    sample_rate: u32,
    original_pitch: u8,
    pitch_correction: i8,
    link: u16,
    kind: u16,
}

#[derive(Default)]
struct RawPdta {
    phdr: Option<Vec<RawHeader>>,
    pbag: Option<Vec<(u16, u16)>>,
    pmod: Option<Vec<Modulator>>,
    pgen: Option<Vec<(u16, i16)>>,
    inst: Option<Vec<RawHeader>>,
    ibag: Option<Vec<(u16, u16)>>,
    imod: Option<Vec<Modulator>>,
    igen: Option<Vec<(u16, i16)>>,
    shdr: Option<Vec<RawSample>>,
}

fn u16_at(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}

fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

fn read_pdta<R: Read>(list: &mut Body<'_, R>) -> Result<RawPdta, Error> {
    let mut p = RawPdta::default();
    let bag = |r: &[u8; 4]| (u16_at(r, 0), u16_at(r, 2));
    let generator = |r: &[u8; 4]| (u16_at(r, 0), u16_at(r, 2) as i16);
    let modulator = |r: &[u8; 10]| Modulator {
        source: Source(u16_at(r, 0)),
        destination: u16_at(r, 2),
        amount: u16_at(r, 4) as i16,
        amount_source: Source(u16_at(r, 6)),
        transform: u16_at(r, 8),
    };
    list.chunks(|chunk| {
        match &chunk.id {
            b"phdr" => {
                let records = chunk.records(|r: &[u8; 38]| RawHeader {
                    name: text(&r[..20]),
                    program: u16_at(r, 20),
                    bank: u16_at(r, 22),
                    bag: u16_at(r, 24),
                })?;
                set_once(&mut p.phdr, records, "phdr")?;
            }
            b"inst" => {
                let records = chunk.records(|r: &[u8; 22]| RawHeader {
                    name: text(&r[..20]),
                    program: 0,
                    bank: 0,
                    bag: u16_at(r, 20),
                })?;
                set_once(&mut p.inst, records, "inst")?;
            }
            b"pbag" => set_once(&mut p.pbag, chunk.records(bag)?, "pbag")?,
            b"ibag" => set_once(&mut p.ibag, chunk.records(bag)?, "ibag")?,
            b"pmod" => set_once(&mut p.pmod, chunk.records(modulator)?, "pmod")?,
            b"imod" => set_once(&mut p.imod, chunk.records(modulator)?, "imod")?,
            b"pgen" => set_once(&mut p.pgen, chunk.records(generator)?, "pgen")?,
            b"igen" => set_once(&mut p.igen, chunk.records(generator)?, "igen")?,
            b"shdr" => {
                let records = chunk.records(|r: &[u8; 46]| RawSample {
                    name: text(&r[..20]),
                    start: u32_at(r, 20),
                    end: u32_at(r, 24),
                    loop_start: u32_at(r, 28),
                    loop_end: u32_at(r, 32),
                    sample_rate: u32_at(r, 36),
                    original_pitch: r[40],
                    pitch_correction: r[41] as i8,
                    link: u16_at(r, 42),
                    kind: u16_at(r, 44),
                })?;
                set_once(&mut p.shdr, records, "shdr")?;
            }
            _ => {}
        }
        Ok(())
    })?;
    Ok(p)
}

fn required<T>(chunk: Option<T>, id: &str) -> Result<T, Error> {
    chunk.ok_or_else(|| invalid(format!("the {id} chunk is missing")))
}

/// Removes the terminal record of a header list, which must be there
/// (§7.2, §7.6, §7.10), and gives it.
fn split_terminal<T>(mut records: Vec<T>, id: &str) -> Result<(Vec<T>, T), Error> {
    let terminal = records.pop().ok_or_else(|| invalid(format!("the {id} chunk has no terminal record")))?;
    Ok((records, terminal))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Level {
    Preset,
    Instrument,
}

impl Level {
    /// The generator that ends a zone and names what it plays.
    fn link(self) -> u16 {
        match self {
            Level::Preset => 41,
            Level::Instrument => 53,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Level::Preset => "preset",
            Level::Instrument => "instrument",
        }
    }

    /// Whether a zone at this level may set the generator. Unused and
    /// reserved generators are ignored everywhere (§8.1.3); at the preset
    /// level so are the sample offsets and the substitution and sample
    /// generators (§8.5, §9.4).
    fn allows(self, generator: u16) -> bool {
        if generator as usize >= GENERATOR_COUNT {
            return false;
        }
        if matches!(generator, 14 | 18 | 19 | 20 | 42 | 49 | 55 | 59 | 60) {
            return false;
        }
        match self {
            Level::Instrument => generator != 41,
            Level::Preset => !matches!(generator, 0..=4 | 12 | 45 | 46 | 47 | 50 | 53 | 54 | 57 | 58),
        }
    }
}

/// A zone as read, before the global zone is merged into it.
struct RawZone {
    generators: [Option<i16>; GENERATOR_COUNT],
    modulators: Vec<Modulator>,
    /// The instrument or sample it plays; `None` in a global zone.
    link: Option<u16>,
}

/// Where the lists of a level's zones are.
struct Lists<'a> {
    level: Level,
    bags: &'a [(u16, u16)],
    gens: &'a [(u16, i16)],
    mods: &'a [Modulator],
}

impl Lists<'_> {
    /// Reads the zones of one preset or instrument, bags `first..last`,
    /// checking every index.
    fn zones(&self, owner: &str, first: usize, last: usize) -> Result<Vec<RawZone>, Error> {
        let what = self.level.name();
        // Each zone needs the next bag to find where it ends.
        if first > last || last >= self.bags.len() {
            return Err(invalid(format!("the {what} {owner:?} has zone indices out of order or out of range")));
        }
        let mut zones = Vec::with_capacity(last - first);
        for j in first..last {
            let (g0, m0) = self.bags[j];
            let (g1, m1) = self.bags[j + 1];
            let (g0, g1, m0, m1) = (g0 as usize, g1 as usize, m0 as usize, m1 as usize);
            if g0 > g1 || g1 > self.gens.len() {
                return Err(invalid(format!(
                    "a zone of the {what} {owner:?} has generator indices out of order or out of range"
                )));
            }
            if m0 > m1 || m1 > self.mods.len() {
                return Err(invalid(format!(
                    "a zone of the {what} {owner:?} has modulator indices out of order or out of range"
                )));
            }
            zones.push(zone(self.level, &self.gens[g0..g1], &self.mods[m0..m1]));
        }
        Ok(zones)
    }
}

/// Applies the rules of §7.5/§7.9 and §8.1.2 to one zone's lists.
fn zone(level: Level, gens: &[(u16, i16)], mods: &[Modulator]) -> RawZone {
    let mut generators = [None; GENERATOR_COUNT];
    let mut link = None;
    for (position, &(oper, amount)) in gens.iter().enumerate() {
        if oper == level.link() {
            // It ends the zone: generators after it are ignored (§7.5, §7.9).
            link = Some(amount as u16);
            break;
        }
        // keyRange only as the first generator, velRange only first or
        // right after keyRange (§8.1.2).
        let misplaced = match oper {
            43 => position != 0,
            44 => !(position == 0 || (position == 1 && gens[0].0 == 43)),
            _ => false,
        };
        if misplaced || !level.allows(oper) {
            continue;
        }
        // Of two identical generators in a zone, the first is ignored.
        generators[oper as usize] = Some(amount);
    }

    // §7.4: of two modulators with the same sources and destination in one
    // zone, the first is ignored.
    let mut modulators = Vec::new();
    let valid: Vec<Modulator> = mods.iter().filter(|m| m.is_valid()).copied().collect();
    upsert(&mut modulators, &valid, |m| key(m, 0));
    RawZone { generators, modulators, link }
}

/// A modulator's identity (§9.5.1): its sources, destination and transform;
/// with `transform` 0 instead, its identity within one zone (§7.4).
fn key(m: &Modulator, transform: u16) -> u64 {
    u64::from(m.source.0)
        | u64::from(m.destination) << 16
        | u64::from(m.amount_source.0) << 32
        | u64::from(transform) << 48
}

/// Puts each of `top` in place of the identical one in `base` (§9.5.1), or
/// after it.
fn overlay(base: &mut Vec<Modulator>, top: &[Modulator]) {
    upsert(base, top, |m| key(m, m.transform));
}

/// Puts each of `top` in place of the one in `list` with the same key, or
/// after it. `list` has no two of the same key. Linear for the usual few
/// modulators, hashed for many, so that no file makes loading quadratic.
fn upsert(list: &mut Vec<Modulator>, top: &[Modulator], key: impl Fn(&Modulator) -> u64) {
    if list.len() + top.len() <= 32 {
        for m in top {
            match list.iter_mut().find(|k| key(k) == key(m)) {
                Some(k) => *k = *m,
                None => list.push(*m),
            }
        }
        return;
    }
    let mut index: HashMap<u64, usize> = list.iter().enumerate().map(|(i, m)| (key(m), i)).collect();
    for m in top {
        match index.get(&key(m)) {
            Some(&i) => list[i] = *m,
            None => {
                index.insert(key(m), list.len());
                list.push(*m);
            }
        }
    }
}

/// Splits off the global zone (§7.3, §7.7): the first zone, when it has no
/// instrument or sample generator. Other zones without one are ignored.
fn split_global(zones: Vec<RawZone>) -> (Option<RawZone>, Vec<RawZone>) {
    let mut zones = zones.into_iter();
    let mut global = None;
    let mut rest = Vec::new();
    if let Some(first) = zones.next() {
        if first.link.is_none() {
            global = Some(first);
        } else {
            rest.push(first);
        }
    }
    rest.extend(zones.filter(|z| z.link.is_some()));
    (global, rest)
}

/// The zone's generators, with the global zone's where it sets none.
fn merge_generators(global: Option<&RawZone>, own: &[Option<i16>; GENERATOR_COUNT]) -> [Option<i16>; GENERATOR_COUNT] {
    let mut out = *own;
    if let Some(global) = global {
        for (o, g) in out.iter_mut().zip(global.generators.iter()) {
            if o.is_none() {
                *o = *g;
            }
        }
    }
    out
}

type Parsed = (Vec<Preset>, Vec<Instrument>, Vec<Sample>);

fn build(p: RawPdta) -> Result<Parsed, Error> {
    let (phdr, pbag, pmod, pgen) =
        (required(p.phdr, "phdr")?, required(p.pbag, "pbag")?, required(p.pmod, "pmod")?, required(p.pgen, "pgen")?);
    let (inst, ibag, imod, igen) =
        (required(p.inst, "inst")?, required(p.ibag, "ibag")?, required(p.imod, "imod")?, required(p.igen, "igen")?);
    let shdr = required(p.shdr, "shdr")?;

    let (shdr, _) = split_terminal(shdr, "shdr")?;
    let samples = shdr.into_iter().map(sample).collect::<Result<Vec<_>, _>>()?;

    let mut zone_modulators = 0usize;
    let mut count_modulators = |n: usize| -> Result<(), Error> {
        zone_modulators += n;
        if zone_modulators > MAX_ZONE_MODULATORS {
            return Err(invalid(format!("more than {MAX_ZONE_MODULATORS} modulators once the zones are merged")));
        }
        Ok(())
    };

    let lists = Lists { level: Level::Instrument, bags: &ibag, gens: &igen, mods: &imod };
    let (inst, inst_terminal) = split_terminal(inst, "inst")?;
    let mut instruments = Vec::with_capacity(inst.len());
    for (i, header) in inst.iter().enumerate() {
        let last = inst.get(i + 1).map_or(inst_terminal.bag, |h| h.bag) as usize;
        let (global, own) = split_global(lists.zones(&header.name, header.bag as usize, last)?);
        let mut base = DEFAULT_MODULATORS.to_vec();
        if let Some(g) = &global {
            overlay(&mut base, &g.modulators);
        }
        // Only the first ones are kept once a zone's are put over them (in
        // place, or after them), so the rest need not be copied per zone.
        base.truncate(ModList::CAPACITY);
        let mut zones = Vec::with_capacity(own.len());
        for z in own {
            let sample = z.link.unwrap_or_default() as usize;
            if sample >= samples.len() {
                return Err(invalid(format!(
                    "the instrument {:?} plays sample {sample}, which is not there",
                    header.name
                )));
            }
            let mut modulators = base.clone();
            overlay(&mut modulators, &z.modulators);
            modulators.truncate(ModList::CAPACITY);
            count_modulators(modulators.len())?;
            let generators = merge_generators(global.as_ref(), &z.generators);
            zones.push(InstrumentZone { zone: Zone { generators, modulators }, sample, plays: false });
        }
        instruments.push(Instrument { name: header.name.clone(), zones });
    }

    let lists = Lists { level: Level::Preset, bags: &pbag, gens: &pgen, mods: &pmod };
    let (phdr, preset_terminal) = split_terminal(phdr, "phdr")?;
    let mut presets = Vec::with_capacity(phdr.len());
    let mut regions = 0usize;
    for (i, header) in phdr.iter().enumerate() {
        let last = phdr.get(i + 1).map_or(preset_terminal.bag, |h| h.bag) as usize;
        let raw = lists.zones(&header.name, header.bag as usize, last)?;
        // §7.2: a preset without zones is ignored.
        if raw.is_empty() {
            continue;
        }
        let (global, own) = split_global(raw);
        let mut base = global.as_ref().map(|g| g.modulators.clone()).unwrap_or_default();
        base.truncate(ModList::CAPACITY);
        let mut zones = Vec::with_capacity(own.len());
        let mut pairs = own.len();
        for z in own {
            let instrument = z.link.unwrap_or_default() as usize;
            let Some(target) = instruments.get(instrument) else {
                return Err(invalid(format!(
                    "the preset {:?} plays instrument {instrument}, which is not there",
                    header.name
                )));
            };
            regions += target.zones.len();
            pairs += target.zones.len();
            if regions > MAX_REGIONS {
                return Err(invalid(format!("more than {MAX_REGIONS} regions (preset zone, instrument zone pairs)")));
            }
            let mut modulators = base.clone();
            overlay(&mut modulators, &z.modulators);
            modulators.truncate(ModList::CAPACITY);
            count_modulators(modulators.len())?;
            let generators = merge_generators(global.as_ref(), &z.generators);
            zones.push(PresetZone { zone: Zone { generators, modulators }, instrument });
        }
        presets.push(Preset { name: header.name.clone(), bank: header.bank, program: header.program, zones, pairs });
    }

    Ok((presets, instruments, samples))
}

fn sample(raw: RawSample) -> Result<Sample, Error> {
    // Bit 4 marks compressed (SoundFont 3) samples.
    if raw.kind & 0x10 != 0 {
        return Err(Error::Unsupported(Unsupported::CompressedSamples));
    }
    let kind = if raw.kind & 0x8000 != 0 {
        SampleKind::Rom
    } else {
        match raw.kind & 0x0F {
            2 => SampleKind::Right,
            4 => SampleKind::Left,
            8 => SampleKind::Linked,
            _ => SampleKind::Mono,
        }
    };
    // §7.10: 255 means unpitched and 128 to 254 are illegal; both play as 60.
    let original_pitch = if raw.original_pitch > 127 { 60 } else { raw.original_pitch };
    Ok(Sample {
        name: raw.name,
        start: raw.start,
        end: raw.end,
        loop_start: raw.loop_start,
        loop_end: raw.loop_end,
        sample_rate: raw.sample_rate,
        original_pitch,
        pitch_correction: raw.pitch_correction,
        link: raw.link,
        kind,
    })
}
