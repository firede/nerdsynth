use super::generator::{DEFAULT_GENERATORS, generator_range};
use super::{GENERATOR_COUNT, GeneratorType, InstrumentZone, ModList, PresetZone, Sample, SoundFont, Zone};

/// A region a note plays: an instrument zone reached through a preset zone,
/// both containing the note's key and velocity.
#[derive(Clone, Copy, Debug)]
pub struct Region<'a> {
    pub(crate) sound_font: &'a SoundFont,
    pub(crate) preset_zone: &'a PresetZone,
    pub(crate) instrument_zone: &'a InstrumentZone,
}

const INSTRUMENT: usize = 41;
const KEY_RANGE: usize = 43;
const VEL_RANGE: usize = 44;
const SAMPLE_ID: usize = 53;

impl<'a> Region<'a> {
    /// The sample the region plays.
    pub fn sample(&self) -> &'a Sample {
        &self.sound_font.samples[self.instrument_zone.sample]
    }

    /// The value of a generator in the region (§9.4): the instrument zone's
    /// value (its default where it sets none) plus the preset zone's,
    /// clamped to the generator's range; initialAttenuation is not yet
    /// scaled, and no modulator is added.
    ///
    /// Generators a preset zone may not set (the sample offsets, keynum,
    /// velocity, sampleModes, exclusiveClass, overridingRootKey) were
    /// dropped when the file was read, so they are the instrument zone's
    /// alone. keyRange and velRange are the intersection of both zones'
    /// ranges, packed as `low | high << 8`; instrument and sampleID are the
    /// indices of the instrument and the sample; unused and reserved
    /// generators are 0.
    pub fn generator(&self, generator: GeneratorType) -> i32 {
        self.generators()[generator as usize]
    }

    /// Every generator, by number, as [`generator`](Self::generator) gives
    /// it.
    pub(crate) fn generators(&self) -> [i32; GENERATOR_COUNT] {
        let mut out = self.sums();
        for (i, value) in out.iter_mut().enumerate() {
            if matches!(i, KEY_RANGE | VEL_RANGE | INSTRUMENT | SAMPLE_ID) {
                continue;
            }
            if let Some((lo, hi)) = generator_range(i) {
                *value = (*value).clamp(lo, hi);
            }
        }
        out
    }

    /// The generators as [`generators`](Self::generators) gives them, but
    /// not clamped: a synthesizer adds the modulators first and clamps the
    /// result (§8.1.3; FluidSynth does too, so an instrument's attenuation
    /// that a preset makes negative lets a modulator's attenuation through
    /// only in part).
    pub(crate) fn sums(&self) -> [i32; GENERATOR_COUNT] {
        let instrument = &self.instrument_zone.zone.generators;
        let preset = &self.preset_zone.zone.generators;
        let mut out = [0i32; GENERATOR_COUNT];
        for (i, value) in out.iter_mut().enumerate() {
            *value = match i {
                KEY_RANGE | VEL_RANGE => {
                    let (a, b) = (range(&self.instrument_zone.zone, i), range(&self.preset_zone.zone, i));
                    i32::from(a.0.max(b.0)) | i32::from(a.1.min(b.1)) << 8
                }
                INSTRUMENT => self.preset_zone.instrument as i32,
                SAMPLE_ID => self.instrument_zone.sample as i32,
                _ => i32::from(instrument[i].unwrap_or(DEFAULT_GENERATORS[i])) + i32::from(preset[i].unwrap_or(0)),
            };
        }
        out
    }

    /// Puts the region's modulators in `out`, in place of what it held
    /// (§9.5): the instrument zone's (the default modulators, with the
    /// instrument's own in place of the identical ones), with the preset
    /// zone's added to the identical ones and appended otherwise. Amounts
    /// that add up past the range of `i16` saturate. The modulators a
    /// synthesizer adds as its [`Config`](crate::Config) says (velocity
    /// and CC74 to the filter cutoff) are not among them. Does not
    /// allocate.
    pub fn modulators(&self, out: &mut ModList) {
        out.clear();
        for m in &self.instrument_zone.zone.modulators {
            out.push(*m);
        }
        for m in &self.preset_zone.zone.modulators {
            match out.as_mut_slice().iter_mut().find(|k| k.is_identical(m)) {
                Some(k) => k.amount = k.amount.saturating_add(m.amount),
                None => out.push(*m),
            }
        }
    }
}

/// A zone's key or velocity range, low and high; the whole range when it
/// sets none.
fn range(zone: &Zone, generator: usize) -> (u8, u8) {
    match zone.generators[generator] {
        Some(v) => {
            let [lo, hi] = v.to_le_bytes();
            (lo, hi)
        }
        None => (0, 127),
    }
}

fn contains(zone: &Zone, key: u8, velocity: u8) -> bool {
    let (k0, k1) = range(zone, KEY_RANGE);
    let (v0, v1) = range(zone, VEL_RANGE);
    (k0..=k1).contains(&key) && (v0..=v1).contains(&velocity)
}

/// Every pair of a zone of the preset and a zone of its instrument that
/// both contain `key` and `velocity`, in the file's order. Zones whose
/// sample has nothing to play (ROM samples among them) are left out. An index past the presets gives none.
/// Allocates nothing.
pub(crate) fn regions(
    sound_font: &SoundFont,
    preset: usize,
    key: u8,
    velocity: u8,
) -> impl Iterator<Item = Region<'_>> {
    let zones = sound_font.presets.get(preset).map_or(&[][..], |p| &p.zones[..]);
    zones.iter().filter(move |pz| contains(&pz.zone, key, velocity)).flat_map(move |preset_zone| {
        // The parser checked every instrument and sample index.
        sound_font.instruments[preset_zone.instrument]
            .zones
            .iter()
            .filter(move |iz| iz.plays && contains(&iz.zone, key, velocity))
            .map(move |instrument_zone| Region { sound_font, preset_zone, instrument_zone })
    })
}
