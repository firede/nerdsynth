/// The number of generator types SoundFont 2.04 defines, 0 to 60.
pub const GENERATOR_COUNT: usize = 61;

/// A generator type, by its number in the SoundFont specification
/// (`GeneratorType::Pan as u16` is 17).
///
/// The list is the specification's, 0 to 60, unused and reserved numbers
/// included; SoundFont 2.04 is its last version. A file's generators of
/// other numbers are ignored when it is read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum GeneratorType {
    /// Offset of the sample start, in sample points.
    StartAddrsOffset = 0,
    /// Offset of the sample end, in sample points.
    EndAddrsOffset = 1,
    /// Offset of the loop start, in sample points.
    StartloopAddrsOffset = 2,
    /// Offset of the loop end, in sample points.
    EndloopAddrsOffset = 3,
    /// Offset of the sample start, in 32768 sample points.
    StartAddrsCoarseOffset = 4,
    /// Modulation LFO to pitch, in cents at full excursion.
    ModLfoToPitch = 5,
    /// Vibrato LFO to pitch, in cents at full excursion.
    VibLfoToPitch = 6,
    /// Modulation envelope to pitch, in cents at its peak.
    ModEnvToPitch = 7,
    /// Low-pass filter cutoff, in absolute cents.
    InitialFilterFc = 8,
    /// Low-pass filter resonance, in centibels above DC gain.
    InitialFilterQ = 9,
    /// Modulation LFO to filter cutoff, in cents at full excursion.
    ModLfoToFilterFc = 10,
    /// Modulation envelope to filter cutoff, in cents at its peak.
    ModEnvToFilterFc = 11,
    /// Offset of the sample end, in 32768 sample points.
    EndAddrsCoarseOffset = 12,
    /// Modulation LFO to volume, in centibels at full excursion.
    ModLfoToVolume = 13,
    /// Unused.
    Unused1 = 14,
    /// Chorus send, in tenths of a percent.
    ChorusEffectsSend = 15,
    /// Reverb send, in tenths of a percent.
    ReverbEffectsSend = 16,
    /// Pan, in tenths of a percent from center (−500 is full left).
    Pan = 17,
    /// Unused.
    Unused2 = 18,
    /// Unused.
    Unused3 = 19,
    /// Unused.
    Unused4 = 20,
    /// Delay of the modulation LFO, in timecents.
    DelayModLfo = 21,
    /// Frequency of the modulation LFO, in absolute cents.
    FreqModLfo = 22,
    /// Delay of the vibrato LFO, in timecents.
    DelayVibLfo = 23,
    /// Frequency of the vibrato LFO, in absolute cents.
    FreqVibLfo = 24,
    /// Delay of the modulation envelope, in timecents.
    DelayModEnv = 25,
    /// Attack of the modulation envelope, in timecents.
    AttackModEnv = 26,
    /// Hold of the modulation envelope, in timecents.
    HoldModEnv = 27,
    /// Decay of the modulation envelope, in timecents.
    DecayModEnv = 28,
    /// Sustain of the modulation envelope, in tenths of a percent below the peak.
    SustainModEnv = 29,
    /// Release of the modulation envelope, in timecents.
    ReleaseModEnv = 30,
    /// Change of the modulation envelope hold per key, in timecents per key from key 60.
    KeynumToModEnvHold = 31,
    /// Change of the modulation envelope decay per key, in timecents per key from key 60.
    KeynumToModEnvDecay = 32,
    /// Delay of the volume envelope, in timecents.
    DelayVolEnv = 33,
    /// Attack of the volume envelope, in timecents.
    AttackVolEnv = 34,
    /// Hold of the volume envelope, in timecents.
    HoldVolEnv = 35,
    /// Decay of the volume envelope, in timecents.
    DecayVolEnv = 36,
    /// Sustain of the volume envelope, in centibels of attenuation.
    SustainVolEnv = 37,
    /// Release of the volume envelope, in timecents.
    ReleaseVolEnv = 38,
    /// Change of the volume envelope hold per key, in timecents per key from key 60.
    KeynumToVolEnvHold = 39,
    /// Change of the volume envelope decay per key, in timecents per key from key 60.
    KeynumToVolEnvDecay = 40,
    /// The instrument of a preset zone, by index.
    Instrument = 41,
    /// Reserved.
    Reserved1 = 42,
    /// The keys a zone plays: lowest in the low byte, highest in the high byte.
    KeyRange = 43,
    /// The velocities a zone plays: lowest in the low byte, highest in the high byte.
    VelRange = 44,
    /// Offset of the loop start, in 32768 sample points.
    StartloopAddrsCoarseOffset = 45,
    /// A fixed key the zone plays as, whatever key is pressed.
    Keynum = 46,
    /// A fixed velocity the zone plays with, whatever velocity is sent.
    Velocity = 47,
    /// Attenuation, in centibels.
    InitialAttenuation = 48,
    /// Reserved.
    Reserved2 = 49,
    /// Offset of the loop end, in 32768 sample points.
    EndloopAddrsCoarseOffset = 50,
    /// Pitch offset, in semitones.
    CoarseTune = 51,
    /// Pitch offset, in cents.
    FineTune = 52,
    /// The sample of an instrument zone, by index.
    SampleId = 53,
    /// How the sample loops: 0 not at all, 1 continuously, 3 until released.
    SampleModes = 54,
    /// Reserved.
    Reserved3 = 55,
    /// Pitch change per key, in cents (100 is the equal-tempered scale).
    ScaleTuning = 56,
    /// An exclusive class: a note stops the notes of the same class on its channel.
    ExclusiveClass = 57,
    /// The key the sample sounds at its own pitch, instead of the sample's.
    OverridingRootKey = 58,
    /// Unused.
    Unused5 = 59,
    /// Ends a list of generators; never applied.
    EndOper = 60,
}

impl GeneratorType {
    /// The generator type of `number`, 0 to 60, such as a modulator's
    /// [destination](super::Modulator::destination).
    pub fn from_number(number: u16) -> Option<GeneratorType> {
        ALL.get(usize::from(number)).copied()
    }
}

/// Every generator type, by number.
const ALL: [GeneratorType; GENERATOR_COUNT] = [
    GeneratorType::StartAddrsOffset,
    GeneratorType::EndAddrsOffset,
    GeneratorType::StartloopAddrsOffset,
    GeneratorType::EndloopAddrsOffset,
    GeneratorType::StartAddrsCoarseOffset,
    GeneratorType::ModLfoToPitch,
    GeneratorType::VibLfoToPitch,
    GeneratorType::ModEnvToPitch,
    GeneratorType::InitialFilterFc,
    GeneratorType::InitialFilterQ,
    GeneratorType::ModLfoToFilterFc,
    GeneratorType::ModEnvToFilterFc,
    GeneratorType::EndAddrsCoarseOffset,
    GeneratorType::ModLfoToVolume,
    GeneratorType::Unused1,
    GeneratorType::ChorusEffectsSend,
    GeneratorType::ReverbEffectsSend,
    GeneratorType::Pan,
    GeneratorType::Unused2,
    GeneratorType::Unused3,
    GeneratorType::Unused4,
    GeneratorType::DelayModLfo,
    GeneratorType::FreqModLfo,
    GeneratorType::DelayVibLfo,
    GeneratorType::FreqVibLfo,
    GeneratorType::DelayModEnv,
    GeneratorType::AttackModEnv,
    GeneratorType::HoldModEnv,
    GeneratorType::DecayModEnv,
    GeneratorType::SustainModEnv,
    GeneratorType::ReleaseModEnv,
    GeneratorType::KeynumToModEnvHold,
    GeneratorType::KeynumToModEnvDecay,
    GeneratorType::DelayVolEnv,
    GeneratorType::AttackVolEnv,
    GeneratorType::HoldVolEnv,
    GeneratorType::DecayVolEnv,
    GeneratorType::SustainVolEnv,
    GeneratorType::ReleaseVolEnv,
    GeneratorType::KeynumToVolEnvHold,
    GeneratorType::KeynumToVolEnvDecay,
    GeneratorType::Instrument,
    GeneratorType::Reserved1,
    GeneratorType::KeyRange,
    GeneratorType::VelRange,
    GeneratorType::StartloopAddrsCoarseOffset,
    GeneratorType::Keynum,
    GeneratorType::Velocity,
    GeneratorType::InitialAttenuation,
    GeneratorType::Reserved2,
    GeneratorType::EndloopAddrsCoarseOffset,
    GeneratorType::CoarseTune,
    GeneratorType::FineTune,
    GeneratorType::SampleId,
    GeneratorType::SampleModes,
    GeneratorType::Reserved3,
    GeneratorType::ScaleTuning,
    GeneratorType::ExclusiveClass,
    GeneratorType::OverridingRootKey,
    GeneratorType::Unused5,
    GeneratorType::EndOper,
];

/// The default value of a generator, and the range of its value in a
/// voice (the instrument's value plus the preset's, plus the modulators'),
/// from the SoundFont specification (§8.1.3). Generators without a range
/// (sample offsets, indices, ranges, unused ones) give `None`.
pub fn generator_range(generator: usize) -> Option<(i32, i32)> {
    let range = match generator {
        5..=7 | 10 | 11 => (-12000, 12000),
        8 => (1500, 13500),
        9 => (0, 960),
        13 => (-960, 960),
        15 | 16 => (0, 1000),
        17 => (-500, 500),
        21 | 23 | 25 | 33 => (-12000, 5000),
        22 | 24 => (-16000, 4500),
        26 | 28 | 30 | 34 | 36 | 38 => (-12000, 8000),
        27 | 35 => (-12000, 5000),
        29 => (0, 1000),
        31 | 32 | 39 | 40 => (-1200, 1200),
        37 | 48 => (0, 1440),
        51 => (-120, 120),
        // The specification's range is -99 to 99; the default pitch wheel
        // modulator drives fine tuning far beyond it.
        52 => (-12700, 12700),
        56 => (0, 1200),
        46 | 47 | 58 => (-1, 127),
        57 => (0, 127),
        _ => return None,
    };
    Some(range)
}

/// The default value of each generator in an instrument zone (§8.1.3);
/// preset zones default to 0, but for their key and velocity ranges.
pub(crate) const DEFAULT_GENERATORS: [i16; GENERATOR_COUNT] = {
    let mut g = [0i16; GENERATOR_COUNT];
    g[8] = 13500;
    g[21] = -12000;
    g[23] = -12000;
    g[25] = -12000;
    g[26] = -12000;
    g[27] = -12000;
    g[28] = -12000;
    g[30] = -12000;
    g[33] = -12000;
    g[34] = -12000;
    g[35] = -12000;
    g[36] = -12000;
    g[38] = -12000;
    g[43] = 0x7F00;
    g[44] = 0x7F00;
    g[46] = -1;
    g[47] = -1;
    g[56] = 100;
    g[58] = -1;
    g
};
