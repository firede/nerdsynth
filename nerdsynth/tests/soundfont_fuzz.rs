//! The fuzz target's check on stable Rust: inputs kept from fuzzing
//! (`tests/data/soundfont_read/`: none crashed so far; these are corpus
//! inputs that cost the most or reach deep), and random mutations of built
//! files.
//! Any input either fails to load with an error, or loads into a SoundFont
//! whose regions all resolve, without panicking.
#![cfg(feature = "testkit")]

use std::path::Path;

use nerdsynth::SoundFont;
use nerdsynth::soundfont::{GeneratorType as G, ModList, Source};
use nerdsynth::testkit::sf2::{SampleSpec, Sf2, Zone, modulator, raw_modulator, raw_source, source, square};

/// What `fuzz/fuzz_targets/soundfont_read.rs` does with an input.
fn check(data: &[u8]) {
    let Ok(sf) = SoundFont::from_bytes(data) else { return };
    let mut mods = ModList::new();
    for preset in 0..sf.presets().len().min(16) {
        for key in [0, 60, 127] {
            for velocity in [1, 100, 127] {
                for region in sf.regions(preset, key, velocity) {
                    for g in (0..61).filter_map(G::from_number) {
                        let _ = region.generator(g);
                    }
                    let _ = region.sample();
                    region.modulators(&mut mods);
                }
            }
        }
    }
}

/// Small files covering what the parser reads: the fuzzer's seeds.
fn seeds() -> Vec<(&'static str, Vec<u8>)> {
    let mut out = Vec::new();
    out.push(("single", Sf2::single(SampleSpec::new("s", square(200, 20, 9000)), Zone::new()).build()));

    let cc1 = source(1, true, false, false, 0);
    let mut b = Sf2::new();
    let s0 = b.sample(SampleSpec::new("l", vec![1; 40]).kind(4, 1).looped(5, 30));
    let s1 = b.sample(SampleSpec::new("r", vec![2; 40]).kind(2, 0).key(255).low_bytes(vec![3; 40]));
    let i = b.instrument(
        "layered",
        [
            Zone::new().set(G::Pan, 100).modulator(modulator(cc1, G::Pan, 10, Source::NONE)),
            Zone::new().keys(0, 63).vels(1, 100).set(G::ExclusiveClass, 1).sample(s0),
            Zone::new()
                .keys(64, 127)
                .modulator(modulator(raw_source(0x0502), G::InitialAttenuation, 100, cc1))
                .sample(s1),
        ],
    );
    let j = b.instrument("plain", [Zone::new().set(G::CoarseTune, 12).sample(s0)]);
    b.preset(
        "a",
        0,
        0,
        [
            Zone::new().set(G::ReverbEffectsSend, 100),
            Zone::new().keys(0, 100).instrument(i),
            Zone::new().modulator(raw_modulator(cc1, G::Pan as u16, 5, Source::NONE, 2)).instrument(j),
        ],
    );
    b.preset("drums", 128, 0, [Zone::new().instrument(j)]);
    out.push(("layered", b.build()));
    out
}

#[test]
fn inputs_kept_from_fuzzing() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/soundfont_read");
    let mut n = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        check(&std::fs::read(&path).unwrap());
        n += 1;
    }
    assert!(n > 0, "no inputs in {}", dir.display());
}

/// A xorshift generator, so that the mutations are the same every run.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

#[test]
fn random_mutations() {
    let seeds = seeds();
    let mut rng = Rng(0x5EED_CAFE_F00D_1234);
    let interesting = [0u8, 1, 0x7F, 0x80, 0xFF, 0x29, 0x35, 0x2B, 0x2C];
    for _ in 0..20_000 {
        let mut data = seeds[rng.below(seeds.len())].1.clone();
        for _ in 0..1 + rng.below(8) {
            let at = rng.below(data.len());
            match rng.below(5) {
                0 => data[at] = rng.next() as u8,
                1 => data[at] = interesting[rng.below(interesting.len())],
                2 => {
                    // A 16-bit field.
                    let v = [0u16, 1, 500, 0x7FFF, 0x8000, 0xFFFF][rng.below(6)];
                    if at + 2 <= data.len() {
                        data[at..at + 2].copy_from_slice(&v.to_le_bytes());
                    }
                }
                3 => data.truncate(at),
                _ => {
                    let len = rng.below(16).min(data.len() - at);
                    data.drain(at..at + len);
                }
            }
            if data.is_empty() {
                break;
            }
        }
        check(&data);
    }
}

/// Writes the seeds for `cargo fuzz run soundfont_read fuzz/seeds/soundfont_read`.
#[test]
#[ignore]
fn write_fuzz_seeds() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../fuzz/seeds/soundfont_read");
    std::fs::create_dir_all(&dir).unwrap();
    for (name, bytes) in seeds() {
        std::fs::write(dir.join(format!("{name}.sf2")), bytes).unwrap();
    }
}
