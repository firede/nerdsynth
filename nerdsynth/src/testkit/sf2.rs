//! A builder of SoundFont 2 files in memory.
//!
//! ```
//! use nerdsynth::soundfont::GeneratorType as G;
//! use nerdsynth::testkit::sf2::{SampleSpec, Sf2, Zone, square};
//!
//! // One preset (0:0) playing one instrument zone playing one sample.
//! let sf = Sf2::single(SampleSpec::new("square", square(4410, 100, 16000)), Zone::new().set(G::Pan, -250)).load();
//! assert_eq!(sf.regions(0, 60, 100).count(), 1);
//!
//! // Or anything else, piece by piece.
//! let mut b = Sf2::new();
//! let s = b.sample(SampleSpec::new("a", vec![0; 100]).key(69).looped(10, 90));
//! let i = b.instrument("inst", [Zone::new().set(G::Pan, 100), Zone::new().keys(0, 63).sample(s)]);
//! b.preset("piano", 0, 0, [Zone::new().set(G::CoarseTune, 12).instrument(i)]);
//! let bytes = b.build();
//! # let _ = bytes;
//! ```
//!
//! What it writes is valid unless asked otherwise; to write invalid files,
//! add [`raw`](Zone::raw) generators in any order, add chunks anywhere
//! ([`Sf2::chunk`]), replace the body of any chunk ([`Sf2::replace`], with
//! [`Sf2::body`] giving what would be written), leave chunks out
//! ([`Sf2::omit`]), or patch the bytes [`Sf2::build`] gives, finding fields
//! with [`find_chunk`].

use std::ops::Range;

use crate::SoundFont;
use crate::soundfont::{GeneratorType, Modulator, Source};

/// A chunk ID.
pub type FourCc = [u8; 4];

/// Where [`Sf2::chunk`] adds a chunk: at the end of the RIFF chunk or of
/// one of its lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    Riff,
    Info,
    Sdta,
    Pdta,
}

/// A sample header as the shdr chunk stores it, positions absolute.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SampleHeader {
    pub name: String,
    pub start: u32,
    pub end: u32,
    pub loop_start: u32,
    pub loop_end: u32,
    pub sample_rate: u32,
    pub original_pitch: u8,
    pub pitch_correction: i8,
    pub link: u16,
    /// 1 mono, 2 right, 4 left, 8 linked; bit 15 ROM.
    pub kind: u16,
}

/// A sample to add with [`Sf2::sample`]: its points, and its header with
/// positions relative to its first point.
#[derive(Clone, Debug)]
pub struct SampleSpec {
    pub name: String,
    pub pcm: Vec<i16>,
    /// The loop, relative to the first point; the whole sample by default.
    pub loop_points: Option<(u32, u32)>,
    pub sample_rate: u32,
    pub original_pitch: u8,
    pub pitch_correction: i8,
    pub link: u16,
    pub kind: u16,
    /// The low bytes of 24-bit samples, one per point; they go in sm24.
    pub low: Option<Vec<u8>>,
}

impl SampleSpec {
    /// A mono sample at 44100 Hz sounding at key 60, looped whole.
    pub fn new(name: &str, pcm: Vec<i16>) -> SampleSpec {
        SampleSpec {
            name: name.into(),
            pcm,
            loop_points: None,
            sample_rate: 44100,
            original_pitch: 60,
            pitch_correction: 0,
            link: 0,
            kind: 1,
            low: None,
        }
    }

    pub fn rate(mut self, sample_rate: u32) -> Self {
        self.sample_rate = sample_rate;
        self
    }

    pub fn key(mut self, original_pitch: u8) -> Self {
        self.original_pitch = original_pitch;
        self
    }

    pub fn correction(mut self, cents: i8) -> Self {
        self.pitch_correction = cents;
        self
    }

    /// The loop, from `start` to `end` points after the first point.
    pub fn looped(mut self, start: u32, end: u32) -> Self {
        self.loop_points = Some((start, end));
        self
    }

    /// The sample type and the linked sample.
    pub fn kind(mut self, kind: u16, link: u16) -> Self {
        self.kind = kind;
        self.link = link;
        self
    }

    /// The low bytes of 24-bit points; the file gets an sm24 chunk.
    pub fn low_bytes(mut self, low: Vec<u8>) -> Self {
        self.low = Some(low);
        self
    }
}

/// A preset or instrument zone: generators and modulators, then what it
/// plays. A zone that plays nothing is a global zone when it comes first.
#[derive(Clone, Debug, Default)]
pub struct Zone {
    keys: Option<(u8, u8)>,
    vels: Option<(u8, u8)>,
    generators: Vec<(u16, u16)>,
    modulators: Vec<Modulator>,
    link: Option<u16>,
    trailing: Vec<(u16, u16)>,
}

impl Zone {
    pub fn new() -> Zone {
        Zone::default()
    }

    /// Sets a generator, written in call order after the ranges.
    pub fn set(mut self, generator: GeneratorType, value: i16) -> Self {
        self.generators.push((generator as u16, value as u16));
        self
    }

    /// The key range, written first as the specification requires.
    pub fn keys(mut self, low: u8, high: u8) -> Self {
        self.keys = Some((low, high));
        self
    }

    /// The velocity range, written right after the key range.
    pub fn vels(mut self, low: u8, high: u8) -> Self {
        self.vels = Some((low, high));
        self
    }

    /// Any generator, unchecked, in call order (after the ranges set with
    /// [`Zone::keys`] and [`Zone::vels`]).
    pub fn raw(mut self, generator: u16, amount: u16) -> Self {
        self.generators.push((generator, amount));
        self
    }

    /// A range generator in call order, wherever that puts it.
    pub fn range(self, generator: GeneratorType, low: u8, high: u8) -> Self {
        self.raw(generator as u16, u16::from_le_bytes([low, high]))
    }

    pub fn modulator(mut self, m: Modulator) -> Self {
        self.modulators.push(m);
        self
    }

    /// Plays sample `id` (in an instrument zone): a sampleID generator,
    /// written last.
    pub fn sample(mut self, id: u16) -> Self {
        self.link = Some(53);
        self.trailing.insert(0, (53, id));
        self
    }

    /// Plays instrument `id` (in a preset zone): an instrument generator,
    /// written last.
    pub fn instrument(mut self, id: u16) -> Self {
        self.link = Some(41);
        self.trailing.insert(0, (41, id));
        self
    }

    /// A generator written after the sampleID or instrument generator,
    /// which the specification says to ignore.
    pub fn after_link(mut self, generator: u16, amount: u16) -> Self {
        self.trailing.push((generator, amount));
        self
    }

    fn generator_list(&self) -> Vec<(u16, u16)> {
        let mut out = Vec::new();
        if let Some((lo, hi)) = self.keys {
            out.push((43, u16::from_le_bytes([lo, hi])));
        }
        if let Some((lo, hi)) = self.vels {
            out.push((44, u16::from_le_bytes([lo, hi])));
        }
        out.extend_from_slice(&self.generators);
        if self.link.is_some() {
            out.extend_from_slice(&self.trailing);
        }
        out
    }
}

#[derive(Clone, Debug)]
struct Header {
    name: String,
    bank: u16,
    program: u16,
    zones: Vec<Zone>,
}

/// A SoundFont 2 file being built. See the [module](self) documentation.
#[derive(Clone, Debug)]
pub struct Sf2 {
    version: (u16, u16),
    name: String,
    data: Vec<i16>,
    low: Vec<u8>,
    has_low: bool,
    sm24: Option<Vec<u8>>,
    samples: Vec<SampleHeader>,
    instruments: Vec<Header>,
    presets: Vec<Header>,
    extra: Vec<(Place, FourCc, Vec<u8>)>,
    replaced: Vec<(FourCc, Vec<u8>)>,
    omitted: Vec<FourCc>,
}

impl Default for Sf2 {
    fn default() -> Self {
        Sf2::new()
    }
}

impl Sf2 {
    /// An empty file of version 2.04, named "Test".
    pub fn new() -> Sf2 {
        Sf2 {
            version: (2, 4),
            name: "Test".into(),
            data: Vec::new(),
            low: Vec::new(),
            has_low: false,
            sm24: None,
            samples: Vec::new(),
            instruments: Vec::new(),
            presets: Vec::new(),
            extra: Vec::new(),
            replaced: Vec::new(),
            omitted: Vec::new(),
        }
    }

    /// One preset (bank 0, program 0, "Preset") with one zone playing one
    /// instrument ("Instrument") with one zone, `zone`, playing `sample`.
    pub fn single(sample: SampleSpec, zone: Zone) -> Sf2 {
        let mut b = Sf2::new();
        let s = b.sample(sample);
        let i = b.instrument("Instrument", [zone.sample(s)]);
        b.preset("Preset", 0, 0, [Zone::new().instrument(i)]);
        b
    }

    pub fn version(&mut self, major: u16, minor: u16) -> &mut Self {
        self.version = (major, minor);
        self
    }

    pub fn name(&mut self, name: &str) -> &mut Self {
        self.name = name.into();
        self
    }

    /// Adds a sample: its points, followed by 46 zero points as the
    /// specification requires, and its header. Gives its index.
    pub fn sample(&mut self, spec: SampleSpec) -> u16 {
        let start = self.data.len() as u32;
        let len = spec.pcm.len() as u32;
        let (ls, le) = spec.loop_points.unwrap_or((0, len));
        self.pcm(&spec.pcm);
        if let Some(low) = &spec.low {
            self.has_low = true;
            self.low.truncate(start as usize);
            self.low.resize(start as usize, 0);
            self.low.extend(low.iter().copied().chain(std::iter::repeat(0)).take(spec.pcm.len()));
        }
        self.pcm(&[0; 46]);
        self.sample_header(SampleHeader {
            name: spec.name,
            start,
            end: start + len,
            loop_start: start + ls,
            loop_end: start + le,
            sample_rate: spec.sample_rate,
            original_pitch: spec.original_pitch,
            pitch_correction: spec.pitch_correction,
            link: spec.link,
            kind: spec.kind,
        })
    }

    /// Appends points to the sample data as they are; gives the position of
    /// the first.
    pub fn pcm(&mut self, points: &[i16]) -> u32 {
        let at = self.data.len() as u32;
        self.data.extend_from_slice(points);
        at
    }

    /// Adds a sample header as it is; gives its index.
    pub fn sample_header(&mut self, header: SampleHeader) -> u16 {
        self.samples.push(header);
        (self.samples.len() - 1) as u16
    }

    /// Writes `bytes` as the sm24 chunk, instead of the low bytes of the
    /// samples (see [`SampleSpec::low_bytes`]).
    pub fn sm24(&mut self, bytes: Vec<u8>) -> &mut Self {
        self.sm24 = Some(bytes);
        self
    }

    /// Adds an instrument; gives its index.
    pub fn instrument(&mut self, name: &str, zones: impl IntoIterator<Item = Zone>) -> u16 {
        self.instruments.push(Header { name: name.into(), bank: 0, program: 0, zones: zones.into_iter().collect() });
        (self.instruments.len() - 1) as u16
    }

    /// Adds a preset; gives its index among the presets written.
    pub fn preset(&mut self, name: &str, bank: u16, program: u16, zones: impl IntoIterator<Item = Zone>) -> usize {
        self.presets.push(Header { name: name.into(), bank, program, zones: zones.into_iter().collect() });
        self.presets.len() - 1
    }

    /// Adds a chunk at the end of the RIFF chunk or of a list. Write a
    /// list as a `LIST` chunk whose data starts with its type.
    pub fn chunk(&mut self, place: Place, id: &[u8; 4], data: &[u8]) -> &mut Self {
        self.extra.push((place, *id, data.to_vec()));
        self
    }

    /// Writes `data` as the body of the chunk `id` (any of INFO, sdta and
    /// pdta, such as `ifil`, `smpl` or `pbag`), instead of what the builder
    /// would write. Adds the chunk where it belongs if it is not written
    /// otherwise.
    pub fn replace(&mut self, id: &[u8; 4], data: Vec<u8>) -> &mut Self {
        self.replaced.retain(|(i, _)| i != id);
        self.replaced.push((*id, data));
        self
    }

    /// Leaves the chunk `id` out, or a whole list when `id` is `INFO`,
    /// `sdta` or `pdta`.
    pub fn omit(&mut self, id: &[u8; 4]) -> &mut Self {
        self.omitted.push(*id);
        self
    }

    /// What the builder writes as the body of chunk `id`, before
    /// [`Sf2::replace`].
    pub fn body(&self, id: &[u8; 4]) -> Option<Vec<u8>> {
        let lists = self.lists();
        lists.iter().flat_map(|(_, chunks)| chunks.iter()).find(|(i, _)| i == id).map(|(_, b)| b.clone())
    }

    /// The file's bytes.
    pub fn build(&self) -> Vec<u8> {
        let mut riff = b"sfbk".to_vec();
        for (kind, mut chunks) in self.lists() {
            if self.omitted.contains(&kind) {
                continue;
            }
            for (id, data) in &self.replaced {
                if let Some(c) = chunks.iter_mut().find(|(i, _)| i == id) {
                    c.1 = data.clone();
                } else if belongs(&kind, id) {
                    chunks.push((*id, data.clone()));
                }
            }
            chunks.retain(|(id, _)| !self.omitted.contains(id));
            let place = match &kind {
                b"INFO" => Place::Info,
                b"sdta" => Place::Sdta,
                _ => Place::Pdta,
            };
            for (p, id, data) in &self.extra {
                if *p == place {
                    chunks.push((*id, data.clone()));
                }
            }
            let mut list = kind.to_vec();
            for (id, data) in &chunks {
                write_chunk(&mut list, id, data);
            }
            write_chunk(&mut riff, b"LIST", &list);
        }
        for (p, id, data) in &self.extra {
            if *p == Place::Riff {
                write_chunk(&mut riff, id, data);
            }
        }
        let mut out = Vec::with_capacity(riff.len() + 8);
        write_chunk(&mut out, b"RIFF", &riff);
        out
    }

    /// Builds and loads the file; panics with the error if it does not
    /// load.
    #[track_caller]
    pub fn load(&self) -> SoundFont {
        match SoundFont::from_bytes(&self.build()) {
            Ok(sf) => sf,
            Err(e) => panic!("the built SoundFont did not load: {e}"),
        }
    }

    fn lists(&self) -> Vec<(FourCc, Vec<Chunk>)> {
        let mut info = Vec::new();
        let mut ifil = Vec::new();
        ifil.extend_from_slice(&self.version.0.to_le_bytes());
        ifil.extend_from_slice(&self.version.1.to_le_bytes());
        info.push((*b"ifil", ifil));
        info.push((*b"isng", text(b"EMU8000")));
        info.push((*b"INAM", text(self.name.as_bytes())));

        let mut sdta = Vec::new();
        let smpl: Vec<u8> = self.data.iter().flat_map(|s| s.to_le_bytes()).collect();
        sdta.push((*b"smpl", smpl));
        if let Some(sm24) = &self.sm24 {
            sdta.push((*b"sm24", sm24.clone()));
        } else if self.has_low {
            let mut low = self.low.clone();
            low.resize(self.data.len(), 0);
            sdta.push((*b"sm24", low));
        }

        let (phdr, pbag, pmod, pgen) = headers(&self.presets, 38, b"EOP");
        let (inst, ibag, imod, igen) = headers(&self.instruments, 22, b"EOI");
        let mut shdr = Vec::new();
        let terminal = SampleHeader {
            name: "EOS".into(),
            start: 0,
            end: 0,
            loop_start: 0,
            loop_end: 0,
            sample_rate: 0,
            original_pitch: 0,
            pitch_correction: 0,
            link: 0,
            kind: 0,
        };
        for h in self.samples.iter().chain(std::iter::once(&terminal)) {
            shdr.extend_from_slice(&name20(&h.name));
            for v in [h.start, h.end, h.loop_start, h.loop_end, h.sample_rate] {
                shdr.extend_from_slice(&v.to_le_bytes());
            }
            shdr.push(h.original_pitch);
            shdr.push(h.pitch_correction as u8);
            shdr.extend_from_slice(&h.link.to_le_bytes());
            shdr.extend_from_slice(&h.kind.to_le_bytes());
        }
        let pdta = vec![
            (*b"phdr", phdr),
            (*b"pbag", pbag),
            (*b"pmod", pmod),
            (*b"pgen", pgen),
            (*b"inst", inst),
            (*b"ibag", ibag),
            (*b"imod", imod),
            (*b"igen", igen),
            (*b"shdr", shdr),
        ];
        vec![(*b"INFO", info), (*b"sdta", sdta), (*b"pdta", pdta)]
    }
}

/// A chunk's ID and data.
type Chunk = (FourCc, Vec<u8>);

fn belongs(list: &FourCc, id: &FourCc) -> bool {
    let sdta = matches!(id, b"smpl" | b"sm24");
    let pdta = matches!(id, b"phdr" | b"pbag" | b"pmod" | b"pgen" | b"inst" | b"ibag" | b"imod" | b"igen" | b"shdr");
    match list {
        b"sdta" => sdta,
        b"pdta" => pdta,
        _ => !sdta && !pdta,
    }
}

/// A zero-terminated string padded to an even length.
fn text(s: &[u8]) -> Vec<u8> {
    let mut v = s.to_vec();
    v.push(0);
    if v.len() % 2 == 1 {
        v.push(0);
    }
    v
}

fn name20(s: &str) -> [u8; 20] {
    let mut b = [0u8; 20];
    let n = s.len().min(20);
    b[..n].copy_from_slice(&s.as_bytes()[..n]);
    b
}

fn write_chunk(out: &mut Vec<u8>, id: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(id);
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(data);
    if data.len() % 2 == 1 {
        out.push(0);
    }
}

fn write_modulator(out: &mut Vec<u8>, m: &Modulator) {
    for v in [m.source.0, m.destination, m.amount as u16, m.amount_source.0, m.transform] {
        out.extend_from_slice(&v.to_le_bytes());
    }
}

/// The header, bag, modulator and generator chunks of presets (records of
/// 38 bytes) or instruments (22 bytes), with their terminal records.
fn headers(list: &[Header], size: usize, terminal: &[u8]) -> (Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>) {
    let (mut hdr, mut bag, mut mods, mut gens) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let (mut bags, mut gen_count, mut mod_count) = (0u16, 0u16, 0u16);
    let header = |hdr: &mut Vec<u8>, name: &str, bank: u16, program: u16, bag_index: u16| {
        hdr.extend_from_slice(&name20(name));
        if size == 38 {
            hdr.extend_from_slice(&program.to_le_bytes());
            hdr.extend_from_slice(&bank.to_le_bytes());
            hdr.extend_from_slice(&bag_index.to_le_bytes());
            hdr.extend_from_slice(&[0; 12]);
        } else {
            hdr.extend_from_slice(&bag_index.to_le_bytes());
        }
    };
    for h in list {
        header(&mut hdr, &h.name, h.bank, h.program, bags);
        for z in &h.zones {
            bag.extend_from_slice(&gen_count.to_le_bytes());
            bag.extend_from_slice(&mod_count.to_le_bytes());
            bags = bags.wrapping_add(1);
            for (oper, amount) in z.generator_list() {
                gens.extend_from_slice(&oper.to_le_bytes());
                gens.extend_from_slice(&amount.to_le_bytes());
                gen_count = gen_count.wrapping_add(1);
            }
            for m in &z.modulators {
                write_modulator(&mut mods, m);
                mod_count = mod_count.wrapping_add(1);
            }
        }
    }
    header(&mut hdr, std::str::from_utf8(terminal).unwrap_or("END"), 0, 0, bags);
    bag.extend_from_slice(&gen_count.to_le_bytes());
    bag.extend_from_slice(&mod_count.to_le_bytes());
    gens.extend_from_slice(&[0; 4]);
    mods.extend_from_slice(&[0; 10]);
    (hdr, bag, mods, gens)
}

/// The range of the data of the first chunk `id` in a RIFF file, found by
/// walking its chunks and lists (`id` may also be a list's type, giving the
/// list's data after the type).
pub fn find_chunk(bytes: &[u8], id: &[u8; 4]) -> Option<Range<usize>> {
    fn walk(bytes: &[u8], mut at: usize, end: usize, id: &[u8; 4]) -> Option<Range<usize>> {
        while at + 8 <= end {
            let cid = &bytes[at..at + 4];
            let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().ok()?) as usize;
            let data = at + 8;
            let data_end = data.checked_add(size)?.min(end);
            if cid == id {
                return Some(data..data_end);
            }
            if (cid == b"LIST" || cid == b"RIFF") && data + 4 <= data_end {
                if &bytes[data..data + 4] == id {
                    return Some(data + 4..data_end);
                }
                if let Some(r) = walk(bytes, data + 4, data_end, id) {
                    return Some(r);
                }
            }
            at = data_end + (size % 2);
        }
        None
    }
    walk(bytes, 0, bytes.len(), id)
}

/// A square wave of `len` points, `period` points long, between `amplitude`
/// and `-amplitude`.
pub fn square(len: usize, period: usize, amplitude: i16) -> Vec<i16> {
    let period = period.max(1);
    (0..len).map(|i| if i % period < period.div_ceil(2) { amplitude } else { -amplitude }).collect()
}

/// A sine wave of `len` points, `period` points long.
pub fn sine(len: usize, period: f64, amplitude: i16) -> Vec<i16> {
    (0..len)
        .map(|i| ((i as f64 / period * std::f64::consts::TAU).sin() * f64::from(amplitude)).round() as i16)
        .collect()
}

/// A modulator source (§8.2): a controller `index` (a MIDI CC when `cc`),
/// its direction, polarity and curve (0 linear, 1 concave, 2 convex,
/// 3 switch).
pub fn source(index: u8, cc: bool, negative: bool, bipolar: bool, curve: u8) -> Source {
    Source(
        u16::from(curve) << 10
            | u16::from(bipolar) << 9
            | u16::from(negative) << 8
            | u16::from(cc) << 7
            | u16::from(index & 0x7F),
    )
}

/// A modulator with a linear transform.
pub fn modulator(source: Source, destination: GeneratorType, amount: i16, amount_source: Source) -> Modulator {
    Modulator { source, destination: destination as u16, amount, amount_source, transform: 0 }
}

/// A modulator source of any 16 bits, valid or not.
pub fn raw_source(bits: u16) -> Source {
    Source(bits)
}

/// A modulator of any fields, valid or not (the destination by number).
pub fn raw_modulator(
    source: Source,
    destination: u16,
    amount: i16,
    amount_source: Source,
    transform: u16,
) -> Modulator {
    Modulator { source, destination, amount, amount_source, transform }
}

/// The default modulators every instrument zone starts with (§8.4), as this
/// library has them.
pub const DEFAULT_MODULATORS: &[Modulator] = crate::soundfont::DEFAULT_MODULATORS;
