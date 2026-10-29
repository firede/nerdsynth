//! Tests of loading (P-SF-01 to P-SF-15 in `spec/behavior.md`) and of
//! resolving regions, on files built with the testkit.

use std::io::{self, Read};

use super::GeneratorType as G;
use super::parse::{MAX_REGIONS, MAX_ZONE_MODULATORS};
use super::*;
use crate::testkit::sf2::{Place, SampleHeader, SampleSpec, Sf2, Zone, find_chunk, modulator, source, square};

fn basic() -> Sf2 {
    Sf2::single(SampleSpec::new("square", square(441, 100, 16000)).key(69), Zone::new().set(G::Pan, -250))
}

fn load(bytes: &[u8]) -> Result<SoundFont, Error> {
    SoundFont::from_bytes(bytes)
}

#[track_caller]
fn invalid(bytes: &[u8]) -> String {
    match load(bytes) {
        Err(Error::Invalid(e)) => {
            assert_eq!(e.format(), crate::error::Format::SoundFont);
            e.to_string()
        }
        Err(e) => panic!("{e}, not invalid"),
        Ok(_) => panic!("loaded"),
    }
}

#[track_caller]
fn unsupported(bytes: &[u8]) -> crate::error::Unsupported {
    match load(bytes) {
        Err(Error::Unsupported(u)) => u,
        Err(e) => panic!("{e}, not unsupported"),
        Ok(_) => panic!("loaded"),
    }
}

fn put_u16(bytes: &mut [u8], at: usize, v: u16) {
    bytes[at..at + 2].copy_from_slice(&v.to_le_bytes());
}

fn put_u32(bytes: &mut [u8], at: usize, v: u32) {
    bytes[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

/// The offset of the data of chunk `id`.
fn data_of(bytes: &[u8], id: &[u8; 4]) -> usize {
    find_chunk(bytes, id).unwrap_or_else(|| panic!("no {id:?} chunk")).start
}

fn only_region(sf: &SoundFont, key: u8, velocity: u8) -> Region<'_> {
    let regions: Vec<_> = sf.regions(0, key, velocity).collect();
    assert_eq!(regions.len(), 1, "regions at key {key}, velocity {velocity}");
    regions[0]
}

fn mods_of(r: &Region<'_>) -> Vec<Modulator> {
    let mut list = ModList::new();
    r.modulators(&mut list);
    list.as_slice().to_vec()
}

#[test]
fn builds_and_loads() {
    let sf = basic().load();
    assert_eq!((sf.presets().len(), sf.instruments().len(), sf.samples().len()), (1, 1, 1));
    assert_eq!(sf.info().version, (2, 4));
    assert_eq!(sf.info().name, "Test");
    assert_eq!(sf.info().sound_engine, "EMU8000");
    assert_eq!(sf.sample_data().len(), 441 + 46);
    let s = &sf.samples()[0];
    assert_eq!((s.start, s.end, s.loop_start, s.loop_end, s.original_pitch), (0, 441, 0, 441, 69));
    assert_eq!(s.kind, SampleKind::Mono);
    assert_eq!(sf.find_preset(0, 0), Some(0));
    assert_eq!(sf.find_preset(0, 1), None);
    assert_eq!(only_region(&sf, 60, 100).generators()[G::Pan as usize], -250);
}

// P-SF-01

#[test]
fn unknown_and_odd_chunks_are_skipped() {
    let mut b = basic();
    b.chunk(Place::Info, b"ICMT", b"odd\0x") // 5 bytes: padded
        .chunk(Place::Info, b"ZZZZ", b"?")
        .chunk(Place::Sdta, b"ZZZZ", b"abc")
        .chunk(Place::Pdta, b"ZZZZ", b"abcde")
        .chunk(Place::Riff, b"LIST", b"junkdata!")
        .chunk(Place::Riff, b"JUNK", b"x");
    let sf = b.load();
    assert_eq!(sf.info().comment.as_deref(), Some("odd"));
    assert_eq!(sf.presets().len(), 1);
}

#[test]
fn a_chunk_larger_than_its_list_is_an_error() {
    for id in [b"phdr", b"igen", b"shdr", b"smpl", b"ifil"] {
        let mut bytes = basic().build();
        let at = data_of(&bytes, id) - 4;
        let size = u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        put_u32(&mut bytes, at, size + 1000);
        let m = invalid(&bytes);
        assert!(m.contains("larger than"), "{id:?}: {m}");
    }
    // A list larger than the RIFF chunk.
    let mut bytes = basic().build();
    let at = data_of(&bytes, b"pdta") - 8;
    put_u32(&mut bytes, at, 1 << 30);
    assert!(invalid(&bytes).contains("larger than the RIFF chunk"));
}

#[test]
fn every_truncation_fails_cleanly() {
    let bytes = basic().build();
    for n in 0..bytes.len() {
        assert!(load(&bytes[..n]).is_err(), "loaded the first {n} bytes");
    }
    // When the RIFF size claims more than there is, the file still loads
    // if it ends between chunks.
    let mut bytes = bytes;
    let size = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
    put_u32(&mut bytes, 4, size + 100);
    load(&bytes).unwrap();
}

#[test]
fn not_a_soundfont() {
    assert!(invalid(b"").contains("ends inside"));
    assert!(invalid(b"RIFX\x04\0\0\0sfbk").contains("not a RIFF"));
    assert!(invalid(b"RIFF\x04\0\0\0WAVE").contains("not sfbk"));
    for list in [b"INFO", b"sdta", b"pdta"] {
        let mut b = basic();
        b.omit(list);
        assert!(invalid(&b.build()).contains("missing"));
    }
    for id in [b"ifil", b"phdr", b"pbag", b"pmod", b"pgen", b"inst", b"ibag", b"imod", b"igen", b"shdr"] {
        let mut b = basic();
        b.omit(id);
        let m = invalid(&b.build());
        assert!(m.contains("missing"), "{id:?}: {m}");
    }
    // Two of a chunk.
    let mut b = basic();
    let phdr = b.body(b"phdr").unwrap();
    b.chunk(Place::Pdta, b"phdr", &phdr);
    assert!(invalid(&b.build()).contains("two"));
    // Records of the wrong size.
    let mut b = basic();
    let mut pgen = b.body(b"pgen").unwrap();
    pgen.push(0);
    pgen.push(0);
    b.replace(b"pgen", pgen);
    assert!(invalid(&b.build()).contains("multiple of 4"));
}

#[test]
fn the_smpl_chunk_is_optional() {
    // A file of ROM samples only has no sample data (§6).
    let mut b = Sf2::new();
    b.sample_header(SampleHeader {
        name: "rom".into(),
        start: 0,
        end: 100,
        loop_start: 0,
        loop_end: 100,
        sample_rate: 44100,
        original_pitch: 60,
        pitch_correction: 0,
        link: 0,
        kind: 0x8001,
    });
    let i = b.instrument("i", [Zone::new().sample(0)]);
    b.preset("p", 0, 0, [Zone::new().instrument(i)]);
    b.omit(b"smpl");
    let sf = b.load();
    assert!(sf.sample_data().is_empty());
    assert_eq!(sf.regions(0, 60, 100).count(), 0);
}

#[test]
fn info_texts() {
    let mut b = basic();
    b.name("Bank").chunk(Place::Info, b"IENG", b"Me\0\0").chunk(Place::Info, b"ICOP", b"caf\xe9\0\0");
    b.replace(b"isng", b"\0\0".to_vec());
    let sf = b.load();
    assert_eq!(sf.info().name, "Bank");
    assert_eq!(sf.info().author.as_deref(), Some("Me"));
    // Not UTF-8: read as Latin-1.
    assert_eq!(sf.info().copyright.as_deref(), Some("café"));
    // An empty isng means EMU8000 (§5.2).
    assert_eq!(sf.info().sound_engine, "EMU8000");
    // A malformed ifil is an error; a malformed iver is ignored.
    let mut b = basic();
    b.replace(b"ifil", vec![2, 0]);
    assert!(invalid(&b.build()).contains("ifil"));
    let mut b = basic();
    b.chunk(Place::Info, b"iver", &[1, 0]);
    assert_eq!(b.load().info().rom_version, None);
}

// P-SF-02

/// Gives at most `sizes[i % len]` bytes per read, and fails with
/// `Interrupted` now and then.
struct Trickle<'a> {
    data: &'a [u8],
    sizes: &'a [usize],
    reads: usize,
}

impl Read for Trickle<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.reads += 1;
        if self.reads % 7 == 0 {
            return Err(io::ErrorKind::Interrupted.into());
        }
        let n = self.sizes[self.reads % self.sizes.len()].min(buf.len()).min(self.data.len());
        buf[..n].copy_from_slice(&self.data[..n]);
        self.data = &self.data[n..];
        Ok(n)
    }
}

#[test]
fn short_reads_do_not_matter() {
    let mut b = basic();
    b.chunk(Place::Info, b"ICMT", b"odd");
    let bytes = b.build();
    let whole = format!("{:?}", load(&bytes).unwrap());
    for sizes in [&[1][..], &[3, 1, 4, 1, 5, 9, 2, 6], &[1000]] {
        let sf = SoundFont::read(Trickle { data: &bytes, sizes, reads: 0 }).unwrap();
        assert_eq!(format!("{sf:?}"), whole, "reads of {sizes:?}");
    }
}

#[test]
fn read_errors_are_io_errors() {
    struct Failing;
    impl Read for Failing {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("disk on fire"))
        }
    }
    assert!(matches!(SoundFont::read(Failing), Err(Error::Io(_))));
}

// P-SF-03 (the allocations themselves are measured in tests/memory.rs)

#[test]
fn huge_claims_in_a_small_file_fail_quickly() {
    let good = basic().build();
    for id in [b"smpl", b"phdr", b"pbag", b"pgen", b"pmod", b"igen", b"shdr", b"ifil"] {
        let mut bytes = good.clone();
        let at = data_of(&bytes, id) - 4;
        // A multiple of every record size, so that only the data runs out.
        put_u32(&mut bytes, at, 0x7FFF_0000 / 96140 * 96140);
        // Make every enclosing size claim as much, so that only the end of
        // the data stops the reader.
        let list = [b"INFO", b"sdta", b"pdta"].into_iter().find(|l| {
            let r = find_chunk(&bytes, l).unwrap();
            r.contains(&at)
        });
        let list_at = data_of(&bytes, list.unwrap()) - 8;
        put_u32(&mut bytes, list_at, 0x7FFF_FFF8);
        put_u32(&mut bytes, 4, 0xFFFF_FFF0);
        let m = invalid(&bytes);
        assert!(m.contains("ends inside"), "{id:?}: {m}");
    }
}

// P-SF-04

#[test]
fn header_lists_need_their_terminal_record() {
    for id in [b"phdr", b"inst", b"shdr"] {
        let mut b = basic();
        b.replace(id, Vec::new());
        let m = invalid(&b.build());
        assert!(m.contains("terminal"), "{id:?}: {m}");
    }
    // The bag lists need theirs to end the last zone.
    let mut b = basic();
    let pbag = b.body(b"pbag").unwrap();
    b.replace(b"pbag", pbag[..4].to_vec());
    assert!(invalid(&b.build()).contains("zone indices"));
}

#[test]
fn the_terminal_record_ends_the_last_item() {
    let mut b = Sf2::new();
    let s = b.sample(SampleSpec::new("s", vec![0; 10]));
    let i = b.instrument("i", [Zone::new().keys(0, 63).sample(s), Zone::new().keys(64, 127).sample(s)]);
    b.preset("p", 0, 0, [Zone::new().instrument(i)]);
    assert_eq!(b.load().instruments()[0].zone_count(), 2);
    // Moving EOI's bag index back to 1 leaves the instrument one zone.
    let mut bytes = b.build();
    let inst = data_of(&bytes, b"inst");
    put_u16(&mut bytes, inst + 22 + 20, 1);
    let sf = load(&bytes).unwrap();
    assert_eq!(sf.instruments()[0].zone_count(), 1);
    assert_eq!(sf.regions(0, 100, 100).count(), 0);
    // And moving EOP's to 0 leaves the preset none, so it is ignored (§7.2).
    let mut bytes = b.build();
    let phdr = data_of(&bytes, b"phdr");
    put_u16(&mut bytes, phdr + 38 + 24, 0);
    assert_eq!(load(&bytes).unwrap().presets().len(), 0);
}

// P-SF-05

#[test]
fn indices_out_of_range_or_order_are_errors() {
    let mut b = Sf2::new();
    let s = b.sample(SampleSpec::new("s", vec![0; 10]));
    let m = modulator(source(1, true, false, false, 0), G::Pan, 10, Source::NONE);
    let i = b.instrument("i", [Zone::new().set(G::Pan, 1).modulator(m).sample(s), Zone::new().modulator(m).sample(s)]);
    b.preset("p", 0, 0, [Zone::new().modulator(m).instrument(i), Zone::new().modulator(m).instrument(i)]);
    let good = b.build();
    load(&good).unwrap();
    let cases: [(&str, &[u8; 4], usize, u16); 11] = [
        ("preset bag index", b"phdr", 24, 500),
        ("terminal preset bag index", b"phdr", 38 + 38 + 24, 500),
        ("instrument bag index", b"inst", 20, 500),
        ("terminal instrument bag index", b"inst", 22 + 22 + 20, 500),
        ("pbag generator index", b"pbag", 4, 500),
        ("pbag generators backwards", b"pbag", 4, 0xFFFF),
        ("ibag generator index", b"ibag", 8, 500),
        ("ibag modulator index", b"ibag", 6, 500),
        ("pbag modulator index", b"pbag", 10, 500),
        ("instrument id", b"pgen", 2, 500),
        ("sample id", b"igen", 4 + 2, 500),
    ];
    for (name, id, offset, value) in cases {
        let mut bytes = good.clone();
        let at = data_of(&bytes, id) + offset;
        put_u16(&mut bytes, at, value);
        assert!(matches!(load(&bytes), Err(Error::Invalid(_))), "{name}");
    }
    // Bags out of order: the second preset starts before the first.
    let mut b2 = b.clone();
    b2.preset("q", 0, 1, [Zone::new().instrument(i)]);
    let mut bytes = b2.build();
    let phdr = data_of(&bytes, b"phdr");
    put_u16(&mut bytes, phdr + 24, 3);
    assert!(invalid(&bytes).contains("out of order"));
}

// P-SF-06

#[test]
fn global_zones_and_zones_that_play_nothing() {
    let mut b = Sf2::new();
    let s = b.sample(SampleSpec::new("s", vec![0; 10]));
    let i = b.instrument(
        "i",
        [
            Zone::new().set(G::Pan, 100).set(G::CoarseTune, 5),
            Zone::new().set(G::Pan, -100).sample(s),
            Zone::new().set(G::ReverbEffectsSend, 7),
            Zone::new().keys(0, 10).sample(s),
        ],
    );
    let j = b.instrument("j", [Zone::new().sample(s), Zone::new().set(G::Pan, 300)]);
    b.preset(
        "p",
        0,
        0,
        [
            Zone::new().set(G::FineTune, 3),
            Zone::new().set(G::FineTune, 4).instrument(i),
            Zone::new().set(G::ChorusEffectsSend, 9),
            Zone::new().instrument(i),
        ],
    );
    b.preset("q", 0, 1, [Zone::new().instrument(j)]);
    let sf = b.load();
    // The zone without a sample after the first is ignored.
    assert_eq!(sf.instruments()[0].zone_count(), 2);
    assert_eq!(sf.instruments()[1].zone_count(), 1);
    assert_eq!(sf.presets()[0].zone_count(), 2);
    let regions: Vec<_> = sf.regions(0, 5, 100).map(|r| r.generators()).collect();
    assert_eq!(regions.len(), 4);
    // A zone's own values replace the global zone's; the others are the
    // global zone's.
    assert_eq!(regions[0][G::Pan as usize], -100);
    assert_eq!(regions[0][G::CoarseTune as usize], 5);
    assert_eq!(regions[0][G::FineTune as usize], 4);
    assert_eq!(regions[1][G::Pan as usize], 100);
    assert_eq!(regions[2][G::FineTune as usize], 3);
    for g in &regions {
        assert_eq!(g[G::ReverbEffectsSend as usize], 0);
        assert_eq!(g[G::ChorusEffectsSend as usize], 0);
    }
    assert_eq!(sf.regions(0, 50, 100).count(), 2);
    // In j the first zone plays a sample, so it is not global, and the
    // second, playing nothing, is ignored.
    assert_eq!(sf.regions(1, 60, 100).next().unwrap().generators()[G::Pan as usize], 0);
}

#[test]
fn generators_after_the_link_are_ignored() {
    let mut b = Sf2::new();
    let s = b.sample(SampleSpec::new("s", vec![0; 10]));
    let i = b.instrument("i", [Zone::new().sample(s).after_link(G::Pan as u16, 200)]);
    b.preset("p", 0, 0, [Zone::new().instrument(i).after_link(G::CoarseTune as u16, 7)]);
    let sf = b.load();
    let g = only_region(&sf, 60, 100).generators();
    assert_eq!((g[G::Pan as usize], g[G::CoarseTune as usize]), (0, 0));
}

// P-SF-07

#[test]
fn key_and_velocity_ranges_count_only_in_their_places() {
    let mut b = Sf2::new();
    let s = b.sample(SampleSpec::new("s", vec![0; 10]));
    let i = b.instrument(
        "i",
        [
            // keyRange first, velRange right after: both count.
            Zone::new().keys(0, 9).vels(0, 9).sample(s),
            // velRange first: counts.
            Zone::new().vels(10, 19).set(G::Pan, 1).sample(s),
            // keyRange after another generator: ignored.
            Zone::new().set(G::Pan, 2).range(G::KeyRange, 20, 29).sample(s),
            // velRange after another generator: ignored.
            Zone::new().keys(30, 39).set(G::Pan, 3).range(G::VelRange, 30, 39).sample(s),
            // velRange then keyRange: the keyRange is not first, ignored.
            Zone::new().range(G::VelRange, 40, 49).range(G::KeyRange, 40, 49).set(G::Pan, 4).sample(s),
        ],
    );
    b.preset("p", 0, 0, [Zone::new().set(G::Pan, 0).range(G::KeyRange, 50, 59).instrument(i)]);
    let sf = b.load();
    let pans = |key: u8, vel: u8| -> Vec<i32> { sf.regions(0, key, vel).map(|r| r.generators()[17]).collect() };
    assert_eq!(pans(5, 5), vec![0, 2]);
    assert_eq!(pans(5, 15), vec![1, 2]);
    assert_eq!(pans(35, 100), vec![2, 3]);
    assert_eq!(pans(100, 45), vec![2, 4]);
    assert_eq!(pans(100, 100), vec![2]);
}

#[test]
fn ranges_in_the_global_zone_apply_to_the_others() {
    let mut b = Sf2::new();
    let s = b.sample(SampleSpec::new("s", vec![0; 10]));
    let i = b.instrument("i", [Zone::new().keys(10, 20), Zone::new().sample(s), Zone::new().keys(0, 5).sample(s)]);
    b.preset("p", 0, 0, [Zone::new().vels(1, 50), Zone::new().instrument(i)]);
    let sf = b.load();
    assert_eq!(sf.regions(0, 15, 40).count(), 1);
    assert_eq!(sf.regions(0, 3, 40).count(), 1);
    assert_eq!(sf.regions(0, 15, 60).count(), 0);
    assert_eq!(sf.regions(0, 30, 40).count(), 0);
}

// P-SF-08

#[test]
fn of_two_identical_generators_the_later_counts() {
    let mut b = Sf2::new();
    let s = b.sample(SampleSpec::new("s", vec![0; 10]));
    let i = b.instrument("i", [Zone::new().set(G::Pan, 100).set(G::Pan, -100).sample(s)]);
    b.preset("p", 0, 0, [Zone::new().set(G::CoarseTune, 1).set(G::CoarseTune, 2).instrument(i)]);
    let sf = b.load();
    let g = only_region(&sf, 60, 100).generators();
    assert_eq!((g[G::Pan as usize], g[G::CoarseTune as usize]), (-100, 2));
}

// P-SF-09

#[test]
fn soundfont_3_is_unsupported() {
    use crate::error::Unsupported::CompressedSamples;
    let mut b = basic();
    b.version(3, 1);
    assert_eq!(unsupported(&b.build()), CompressedSamples);
    // The compressed bit in a sample type.
    let mut b = Sf2::new();
    let s = b.sample(SampleSpec::new("ogg", vec![0; 10]).kind(0x11, 0));
    let i = b.instrument("i", [Zone::new().sample(s)]);
    b.preset("p", 0, 0, [Zone::new().instrument(i)]);
    assert_eq!(unsupported(&b.build()), CompressedSamples);
    // Ogg data in a file claiming 2.01.
    let mut ogg = vec![0i16; 100];
    ogg[0] = i16::from_le_bytes(*b"Og");
    ogg[1] = i16::from_le_bytes(*b"gS");
    let b = Sf2::single(SampleSpec::new("s", ogg), Zone::new());
    assert_eq!(unsupported(&b.build()), CompressedSamples);
}

// P-SF-10

#[test]
fn sm24_counts_when_valid() {
    let low: Vec<u8> = (0..11).collect();
    let spec = SampleSpec::new("s", vec![1; 11]).low_bytes(low.clone());
    let b = Sf2::single(spec.clone(), Zone::new());
    let sf = b.load();
    let data24 = sf.sample_data24().expect("sm24");
    assert_eq!(data24.len(), sf.sample_data().len());
    assert_eq!(&data24[..11], &low[..]);

    // An odd number of points: sm24 may have a pad byte, which is dropped.
    let mut b = Sf2::single(SampleSpec::new("s", vec![1; 12]), Zone::new());
    b.pcm(&[0]); // 12 + 46 + 1 = 59 points
    for n in [59, 60] {
        b.sm24(vec![7; n]);
        let sf = b.load();
        assert_eq!(sf.sample_data().len(), 59);
        assert_eq!(sf.sample_data24().map(<[u8]>::len), Some(59), "sm24 of {n}");
    }
    // An even number: no pad byte.
    let mut b = Sf2::single(SampleSpec::new("s", vec![1; 11]), Zone::new());
    b.pcm(&[0]); // 58 points
    b.sm24(vec![7; 59]);
    assert!(b.load().sample_data24().is_none());

    // The wrong size: ignored.
    let mut b = Sf2::single(spec.clone(), Zone::new());
    b.sm24(vec![0; 10]);
    assert!(b.load().sample_data24().is_none());
    // Before version 2.04: ignored.
    let mut b = Sf2::single(spec, Zone::new());
    b.version(2, 1);
    assert!(b.load().sample_data24().is_none());
    // Without smpl: ignored.
    let mut b = Sf2::new();
    b.sm24(vec![0; 4]).omit(b"smpl");
    let sf = b.load();
    assert!(sf.sample_data().is_empty() && sf.sample_data24().is_none());
}

// P-SF-11

#[test]
fn sample_headers_outside_the_data_load() {
    let mut b = Sf2::new();
    b.pcm(&[0; 100]);
    let mut h = SampleHeader {
        name: "far".into(),
        start: 50,
        end: 1_000_000,
        loop_start: 2_000_000,
        loop_end: 10,
        sample_rate: 0,
        original_pitch: 60,
        pitch_correction: -100,
        link: 999,
        kind: 1,
    };
    let s0 = b.sample_header(h.clone());
    h.start = u32::MAX;
    h.end = 0;
    let s1 = b.sample_header(h);
    let i = b.instrument("i", [Zone::new().sample(s0), Zone::new().sample(s1)]);
    b.preset("p", 0, 0, [Zone::new().instrument(i)]);
    let sf = b.load();
    let s = &sf.samples()[0];
    assert_eq!((s.start, s.end, s.loop_start, s.loop_end), (50, 1_000_000, 2_000_000, 10));
    assert_eq!((sf.samples()[1].start, sf.samples()[1].end), (u32::MAX, 0));
    // The first header clamped into the data plays; the second has no point
    // in it, so its zone gives no region.
    assert_eq!(sf.regions(0, 60, 100).count(), 1);
}

// P-SF-13

/// An instrument of `zones` zones and a preset playing it `layers` times.
fn layered(zones: usize, layers: usize) -> Vec<u8> {
    let mut b = Sf2::new();
    let s = b.sample(SampleSpec::new("s", vec![0; 10]));
    let i = b.instrument("i", (0..zones).map(|_| Zone::new().sample(s)));
    b.preset("p", 0, 0, (0..layers).map(|_| Zone::new().instrument(i)));
    b.build()
}

#[test]
fn regions_are_bounded() {
    assert_eq!(MAX_REGIONS, 1 << 20);
    let sf = load(&layered(1024, 1024)).unwrap();
    assert_eq!(sf.presets()[0].zone_count(), 1024);
    assert!(invalid(&layered(1024, 1025)).contains("regions"));
}

#[test]
fn merged_modulators_are_bounded() {
    // A global zone of many modulators, merged into every zone.
    let distinct: Vec<Modulator> = (0..=127u8)
        .flat_map(|cc| (0..61u16).map(move |d| (cc, d)))
        .map(|(cc, d)| Modulator {
            source: source(cc, true, false, false, 0),
            destination: d,
            amount: 1,
            amount_source: Source::NONE,
            transform: 0,
        })
        .filter(|m| m.is_valid() && !DEFAULT_MODULATORS.iter().any(|d| d.is_identical(m)))
        .take(4000)
        .collect();
    let build = |zones: usize| {
        let mut b = Sf2::new();
        let s = b.sample(SampleSpec::new("s", vec![0; 10]));
        let mut global = Zone::new();
        for m in &distinct {
            global = global.modulator(*m);
        }
        let i = b.instrument("i", std::iter::once(global).chain((0..zones).map(|_| Zone::new().sample(s))));
        b.preset("p", 0, 0, [Zone::new().instrument(i)]);
        b.build()
    };
    // Each zone keeps the first 64, as many as a voice plays: the defaults
    // and the first of the global zone's.
    let sf = load(&build(10)).unwrap();
    let mut list = ModList::new();
    sf.regions(0, 60, 100).next().unwrap().modulators(&mut list);
    assert_eq!(list.as_slice().len(), ModList::CAPACITY);
    assert_eq!(list.as_slice()[..DEFAULT_MODULATORS.len()], *DEFAULT_MODULATORS);
    assert_eq!(list.as_slice()[DEFAULT_MODULATORS.len()], distinct[0]);
    // So the merged modulators of the most zones a file can have stay
    // within the bound.
    let sf = load(&build(5000)).unwrap();
    assert!(sf.instruments[0].zones.iter().all(|z| z.zone.modulators.len() == ModList::CAPACITY));
    const { assert!(65536 * ModList::CAPACITY <= MAX_ZONE_MODULATORS) };
}

/// A global zone's many modulators are not copied into every zone before
/// being cut to 64: loading stays linear in the file.
#[test]
fn big_global_zones_load_in_linear_time() {
    let distinct: Vec<Modulator> = (0..=127u8)
        .flat_map(|cc| (0..61u16).map(move |d| (cc, d)))
        .map(|(cc, d)| Modulator {
            source: source(cc, true, false, false, 0),
            destination: d,
            amount: 1,
            amount_source: Source::NONE,
            transform: 0,
        })
        .filter(|m| m.is_valid() && !DEFAULT_MODULATORS.iter().any(|d| d.is_identical(m)))
        .take(4000)
        .collect();
    let mut b = Sf2::new();
    let s = b.sample(SampleSpec::new("s", vec![0; 10]));
    let global = distinct.iter().fold(Zone::new(), |z, m| z.modulator(*m));
    let i = b.instrument("i", std::iter::once(global.clone()).chain((0..8000).map(|_| Zone::new().sample(s))));
    let j = b.instrument("j", [Zone::new().sample(s)]);
    let zones = (0..8000).map(|k| Zone::new().instrument(if k == 0 { i } else { j }));
    b.preset("p", 0, 0, std::iter::once(global).chain(zones));
    let bytes = b.build();
    let t = std::time::Instant::now();
    let sf = load(&bytes).unwrap();
    // Copying 4000 modulators into each of 16000 zones took 10 s and more.
    assert!(t.elapsed() < std::time::Duration::from_secs(2), "{:?}", t.elapsed());
    assert!(sf.presets[0].zones.iter().all(|z| z.zone.modulators.len() == ModList::CAPACITY));
}

// P-SF-14

#[test]
fn unpitched_samples_play_at_60() {
    let mut b = Sf2::new();
    for key in [255u8, 128, 200, 127, 0, 69] {
        b.sample(SampleSpec::new("s", vec![0; 10]).key(key));
    }
    let sf = b.load();
    let pitches: Vec<u8> = sf.samples().iter().map(|s| s.original_pitch).collect();
    assert_eq!(pitches, [60, 60, 60, 127, 0, 69]);
}

// P-SF-15

#[test]
fn rom_samples_are_left_out() {
    let mut b = Sf2::new();
    let ram = b.sample(SampleSpec::new("ram", vec![0; 10]).kind(4, 1));
    let rom = b.sample(SampleSpec::new("rom", vec![0; 10]).kind(0x8001, 0));
    let i = b.instrument("i", [Zone::new().sample(rom), Zone::new().set(G::Pan, 9).sample(ram)]);
    b.preset("p", 0, 0, [Zone::new().instrument(i)]);
    let sf = b.load();
    assert_eq!(sf.samples()[1].kind, SampleKind::Rom);
    assert_eq!(sf.samples()[0].kind, SampleKind::Left);
    assert_eq!(sf.samples()[0].link, 1);
    let r = only_region(&sf, 60, 100);
    assert_eq!(r.sample().name, "ram");
    assert_eq!(r.generators()[G::Pan as usize], 9);
}

// Resolving generators

#[test]
fn preset_values_add_to_instrument_values_and_clamp() {
    let mut b = Sf2::new();
    let s = b.sample(SampleSpec::new("s", vec![0; 10]));
    let i = b.instrument(
        "i",
        [Zone::new().set(G::Pan, 400).set(G::InitialAttenuation, 100).set(G::CoarseTune, -100).sample(s)],
    );
    b.preset(
        "p",
        0,
        0,
        [Zone::new()
            .set(G::Pan, 300)
            .set(G::InitialAttenuation, -500)
            .set(G::AttackVolEnv, 1200)
            .set(G::InitialFilterFc, 1000)
            .set(G::CoarseTune, -50)
            .instrument(i)],
    );
    let sf = b.load();
    let g = only_region(&sf, 60, 100).generators();
    assert_eq!(g[G::Pan as usize], 500);
    assert_eq!(g[G::InitialAttenuation as usize], 0);
    // Defaults where the instrument sets nothing.
    assert_eq!(g[G::AttackVolEnv as usize], -12000 + 1200);
    assert_eq!(g[G::InitialFilterFc as usize], 13500);
    assert_eq!(g[G::CoarseTune as usize], -120);
    assert_eq!(g[G::ScaleTuning as usize], 100);
    assert_eq!(g[G::Keynum as usize], -1);
    assert_eq!(g[G::Velocity as usize], -1);
    assert_eq!(g[G::OverridingRootKey as usize], -1);
    assert_eq!(g[G::SampleId as usize], 0);
    assert_eq!(g[G::Instrument as usize], 0);
}

#[test]
fn generators_a_preset_may_not_set_are_ignored() {
    let mut b = Sf2::new();
    let s = b.sample(SampleSpec::new("s", vec![0; 100]));
    let t = b.sample(SampleSpec::new("t", vec![0; 100]));
    let i = b.instrument("i", [Zone::new().set(G::SampleModes, 1).set(G::ExclusiveClass, 3).sample(s)]);
    let instrument_only = [
        G::StartAddrsOffset,
        G::EndAddrsOffset,
        G::StartloopAddrsOffset,
        G::EndloopAddrsOffset,
        G::StartAddrsCoarseOffset,
        G::EndAddrsCoarseOffset,
        G::StartloopAddrsCoarseOffset,
        G::EndloopAddrsCoarseOffset,
        G::Keynum,
        G::Velocity,
        G::SampleModes,
        G::ExclusiveClass,
        G::OverridingRootKey,
    ];
    let mut z = Zone::new().set(G::SampleId, t as i16).set(G::Unused1, 5).set(G::Reserved1, 5);
    for g in instrument_only {
        z = z.set(g, 7);
    }
    b.preset("p", 0, 0, [z.instrument(i)]);
    let sf = b.load();
    let r = only_region(&sf, 60, 100);
    let g = r.generators();
    assert_eq!(r.sample().name, "s");
    for x in instrument_only {
        let want = match x {
            G::SampleModes => 1,
            G::ExclusiveClass => 3,
            G::Keynum | G::Velocity | G::OverridingRootKey => -1,
            _ => 0,
        };
        assert_eq!(g[x as usize], want, "{x:?}");
    }
    assert_eq!(g[G::Unused1 as usize], 0);
    assert_eq!(g[G::Reserved1 as usize], 0);
    // An instrument generator in an instrument zone is ignored too.
    let mut b = Sf2::new();
    let s = b.sample(SampleSpec::new("s", vec![0; 100]));
    let i = b.instrument("i", [Zone::new().set(G::Instrument, 5).sample(s)]);
    b.preset("p", 0, 0, [Zone::new().instrument(i)]);
    assert_eq!(b.load().regions(0, 60, 100).count(), 1);
}

#[test]
fn ranges_intersect() {
    let mut b = Sf2::new();
    let s = b.sample(SampleSpec::new("s", vec![0; 10]));
    let i = b.instrument("i", [Zone::new().keys(10, 50).vels(20, 127).sample(s)]);
    b.preset("p", 0, 0, [Zone::new().keys(40, 90).vels(0, 100).instrument(i)]);
    let sf = b.load();
    assert_eq!(sf.regions(0, 39, 50).count(), 0);
    assert_eq!(sf.regions(0, 51, 50).count(), 0);
    assert_eq!(sf.regions(0, 45, 101).count(), 0);
    assert_eq!(sf.regions(0, 45, 19).count(), 0);
    let g = only_region(&sf, 45, 50).generators();
    assert_eq!(g[G::KeyRange as usize], 40 | 50 << 8);
    assert_eq!(g[G::VelRange as usize], 20 | 100 << 8);
}

#[test]
fn regions_of_every_layer() {
    let mut b = Sf2::new();
    let s = b.sample(SampleSpec::new("s", vec![0; 10]));
    let i = b.instrument("i", [Zone::new().keys(0, 60).sample(s), Zone::new().keys(60, 127).sample(s)]);
    let j = b.instrument("j", [Zone::new().sample(s)]);
    b.preset("p", 0, 0, [Zone::new().instrument(i), Zone::new().instrument(j), Zone::new().keys(0, 10).instrument(i)]);
    let sf = b.load();
    assert_eq!(sf.regions(0, 60, 1).count(), 3);
    assert_eq!(sf.regions(0, 61, 1).count(), 2);
    assert_eq!(sf.regions(0, 5, 1).count(), 3);
    assert_eq!(sf.regions(1, 5, 1).count(), 0);
    assert_eq!(sf.regions(usize::MAX, 5, 1).count(), 0);
}

#[test]
fn the_first_of_two_presets_with_one_number_is_found() {
    let mut b = basic();
    b.preset("again", 0, 0, [Zone::new().instrument(0)]);
    b.preset("drums", 128, 0, [Zone::new().instrument(0)]);
    let sf = b.load();
    assert_eq!(sf.find_preset(0, 0), Some(0));
    assert_eq!(sf.find_preset(128, 0), Some(2));
    assert_eq!(sf.presets()[2].name, "drums");
}

// Resolving modulators

#[test]
fn the_default_modulators() {
    // The specification's, but for velocity to filter cutoff, which is
    // still open (decisions.md).
    assert_eq!(DEFAULT_MODULATORS.len(), 9);
    assert!(DEFAULT_MODULATORS.iter().all(|m| m.is_valid()));
    assert!(!DEFAULT_MODULATORS.iter().any(|m| m.destination == G::InitialFilterFc as u16));
    for (i, a) in DEFAULT_MODULATORS.iter().enumerate() {
        for b in &DEFAULT_MODULATORS[i + 1..] {
            assert!(!a.is_identical(b));
        }
    }
    let sf = basic().load();
    assert_eq!(mods_of(&only_region(&sf, 60, 100)), DEFAULT_MODULATORS);
}

#[test]
fn modulators_merge_by_level() {
    let cc1 = source(1, true, false, false, 0);
    let cc2 = source(2, true, false, false, 0);
    let vel = Source(0x0502);
    let m = |s: Source, d: G, amount: i16| modulator(s, d, amount, Source::NONE);
    let mut b = Sf2::new();
    let s = b.sample(SampleSpec::new("s", vec![0; 10]));
    let i = b.instrument(
        "i",
        [
            // The global zone replaces a default, and adds one.
            Zone::new().modulator(m(vel, G::InitialAttenuation, 480)).modulator(m(cc1, G::Pan, 10)),
            // The zone replaces the global zone's, and adds one.
            Zone::new().keys(1, 127).modulator(m(cc1, G::Pan, 20)).modulator(m(cc2, G::Pan, 30)).sample(s),
            Zone::new().keys(0, 0).sample(s),
        ],
    );
    b.preset(
        "p",
        0,
        0,
        [
            Zone::new().modulator(m(cc2, G::Pan, 1)).modulator(m(cc2, G::CoarseTune, 2)),
            // The local zone replaces the global zone's; both add to the
            // instrument's.
            Zone::new().modulator(m(cc2, G::Pan, 3)).modulator(m(vel, G::InitialAttenuation, -80)).instrument(i),
        ],
    );
    let sf = b.load();
    let mods = mods_of(&only_region(&sf, 60, 100));
    let mut want = DEFAULT_MODULATORS.to_vec();
    want[0].amount = 480 - 80;
    want.push(m(cc1, G::Pan, 20));
    want.push(m(cc2, G::Pan, 33));
    want.push(m(cc2, G::CoarseTune, 2));
    assert_eq!(mods, want);
    // The other zone has only the global zone's.
    let mods = mods_of(&only_region(&sf, 0, 100));
    let mut want = DEFAULT_MODULATORS.to_vec();
    want[0].amount = 400;
    want.push(m(cc1, G::Pan, 10));
    want.push(m(cc2, G::Pan, 3));
    want.push(m(cc2, G::CoarseTune, 2));
    assert_eq!(mods, want);
}

#[test]
fn amounts_saturate() {
    let vel = Source(0x0502);
    let mut b = Sf2::new();
    let s = b.sample(SampleSpec::new("s", vec![0; 10]));
    let i = b
        .instrument("i", [Zone::new().modulator(modulator(vel, G::InitialAttenuation, 30000, Source::NONE)).sample(s)]);
    b.preset(
        "p",
        0,
        0,
        [Zone::new().modulator(modulator(vel, G::InitialAttenuation, 30000, Source::NONE)).instrument(i)],
    );
    let sf = b.load();
    assert_eq!(mods_of(&only_region(&sf, 60, 100))[0].amount, i16::MAX);
}

#[test]
fn modulators_differing_in_transform_are_not_identical() {
    let cc1 = source(1, true, false, false, 0);
    let mut abs = modulator(cc1, G::Pan, 5, Source::NONE);
    abs.transform = 2;
    let mut b = Sf2::new();
    let s = b.sample(SampleSpec::new("s", vec![0; 10]));
    let i = b.instrument("i", [Zone::new().modulator(modulator(cc1, G::Pan, 7, Source::NONE)).sample(s)]);
    b.preset("p", 0, 0, [Zone::new().modulator(abs).instrument(i)]);
    let sf = b.load();
    let mods = mods_of(&only_region(&sf, 60, 100));
    assert_eq!(&mods[DEFAULT_MODULATORS.len()..], &[modulator(cc1, G::Pan, 7, Source::NONE), abs]);
}

#[test]
fn invalid_and_duplicate_modulators_in_a_zone() {
    let cc1 = source(1, true, false, false, 0);
    let ok = modulator(cc1, G::Pan, 1, Source::NONE);
    let invalid_ones = [
        modulator(source(0, true, false, false, 0), G::Pan, 1, Source::NONE), // CC0: bank select
        modulator(source(6, true, false, false, 0), G::Pan, 1, Source::NONE), // data entry
        modulator(source(120, true, false, false, 0), G::Pan, 1, Source::NONE),
        modulator(source(5, false, false, false, 0), G::Pan, 1, Source::NONE), // no such controller
        modulator(source(1, true, false, false, 4), G::Pan, 1, Source::NONE),  // no such curve
        modulator(cc1, G::Pan, 1, Source(0x007F)),                             // link as amount source
        Modulator { destination: 61, ..ok },
        Modulator { transform: 1, ..ok },
    ];
    let mut z = Zone::new();
    for m in invalid_ones {
        assert!(!m.is_valid(), "{m:?}");
        z = z.modulator(m);
    }
    // Of two with the same sources and destination, the later counts, even
    // with another transform (§7.4).
    let later = Modulator { amount: 2, transform: 2, ..ok };
    z = z.modulator(ok).modulator(later);
    // A link destination is kept for now.
    let linked = Modulator { destination: 0x8000, ..ok };
    z = z.modulator(linked);
    let mut b = Sf2::new();
    let s = b.sample(SampleSpec::new("s", vec![0; 10]));
    let i = b.instrument("i", [z.sample(s)]);
    b.preset("p", 0, 0, [Zone::new().instrument(i)]);
    let sf = b.load();
    let mods = mods_of(&only_region(&sf, 60, 100));
    assert_eq!(&mods[DEFAULT_MODULATORS.len()..], &[later, linked]);
}

#[test]
fn many_modulators_load_in_linear_time() {
    // 30000 distinct modulators in a global zone and as many in a zone:
    // merging must not compare every pair.
    let all: Vec<Modulator> = (0..=127u8)
        .flat_map(|cc| {
            (0..61u16).flat_map(move |d| (0..4u8).flat_map(move |curve| (0..3u8).map(move |a| (cc, d, curve, a))))
        })
        .map(|(cc, d, curve, a)| Modulator {
            source: source(cc, true, false, false, curve),
            destination: d,
            amount: 1,
            amount_source: if a == 0 { Source::NONE } else { source(a, true, false, false, 0) },
            transform: 0,
        })
        .filter(|m| m.is_valid())
        .take(60000)
        .collect();
    let mut global = Zone::new();
    for m in &all[..30000] {
        global = global.modulator(*m);
    }
    let mut own = Zone::new();
    for m in &all[30000..] {
        own = own.modulator(*m);
    }
    let mut b = Sf2::new();
    let s = b.sample(SampleSpec::new("s", vec![0; 10]));
    let i = b.instrument("i", [global, own.sample(s)]);
    b.preset("p", 0, 0, [Zone::new().instrument(i)]);
    let bytes = b.build();
    let started = std::time::Instant::now();
    load(&bytes).unwrap();
    assert!(started.elapsed().as_secs_f64() < 2.0, "{:?}", started.elapsed());
}

/// `{:?}` of a SoundFont, and of a synthesizer playing it, is a summary,
/// not the sample data.
#[test]
fn debug_is_a_summary() {
    let sf = Sf2::single(SampleSpec::new("s", square(100_000, 100, 16000)), Zone::new()).load();
    let text = format!("{sf:?}");
    assert!(text.contains("presets: 1") && text.contains("sample_points: "), "{text}");
    assert!(text.len() < 300, "{text}");
    let synth = crate::Synth::new(std::sync::Arc::new(sf), crate::Config::new(44100)).unwrap();
    let text = format!("{synth:?}");
    assert!(text.contains("active_voices: 0") && text.len() < 1000, "{text}");
}
