/// A modulator source: a controller, its direction, polarity and curve,
/// as the SoundFont specification packs them in 16 bits (§8.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct Source(pub(crate) u16);

impl Source {
    /// No controller: a source of this is taken as 1.
    pub const NONE: Source = Source(0);

    /// The 16 bits, as the file gives them.
    pub fn bits(self) -> u16 {
        self.0
    }

    /// The controller's index: a MIDI CC when [`Source::is_cc`], else one of
    /// the general controllers (0 none, 2 velocity, 3 key, 10 poly pressure,
    /// 13 channel pressure, 14 pitch wheel, 16 pitch wheel sensitivity).
    pub fn index(self) -> u8 {
        (self.0 & 0x7F) as u8
    }

    /// Whether [`index`](Self::index) is a MIDI CC.
    pub fn is_cc(self) -> bool {
        self.0 & 0x80 != 0
    }
    /// From the maximum to the minimum.
    pub fn is_negative(self) -> bool {
        self.0 & 0x100 != 0
    }
    /// -1 to 1, not 0 to 1.
    pub fn is_bipolar(self) -> bool {
        self.0 & 0x200 != 0
    }
    /// 0 linear, 1 concave, 2 convex, 3 switch.
    pub fn curve(self) -> u8 {
        (self.0 >> 10) as u8
    }
}

/// A modulator (§8.2): adds [`amount`](Self::amount) times the values of
/// its sources to the generator [`destination`](Self::destination).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Modulator {
    pub(crate) source: Source,
    pub(crate) destination: u16,
    pub(crate) amount: i16,
    pub(crate) amount_source: Source,
    pub(crate) transform: u16,
}

impl Modulator {
    /// The primary source.
    pub fn source(&self) -> Source {
        self.source
    }

    /// The generator it modulates, by number (see [`GeneratorType`](super::GeneratorType)).
    /// A link to another modulator (bit 15) is kept here but not played yet.
    pub fn destination(&self) -> u16 {
        self.destination
    }

    /// How much the destination changes at the source's full value, in the
    /// destination's units.
    pub fn amount(&self) -> i16 {
        self.amount
    }

    /// The source the amount is scaled by; [`Source::NONE`] for none.
    pub fn amount_source(&self) -> Source {
        self.amount_source
    }

    /// 0 linear, 2 absolute value.
    pub fn transform(&self) -> u16 {
        self.transform
    }

    /// Whether a modulator read from a file can be kept (§7.4, §8.2): its
    /// sources are defined controllers with defined curves, its amount
    /// source is not a link, its destination is a generator or a link, and
    /// its transform is defined. The others are ignored.
    pub(crate) fn is_valid(&self) -> bool {
        let destination_ok = self.destination & 0x8000 != 0 || (self.destination as usize) < super::GENERATOR_COUNT;
        let amount_source_ok = valid_source(self.amount_source) && self.amount_source.0 & 0x00FF != 0x007F;
        destination_ok && valid_source(self.source) && amount_source_ok && matches!(self.transform, 0 | 2)
    }

    /// Whether `self` and `other` are the same modulator but for their
    /// amounts (§9.5.1): a zone's own replaces the identical one it
    /// inherits, a preset's adds to the instrument's.
    pub fn is_identical(&self, other: &Modulator) -> bool {
        self.source == other.source
            && self.destination == other.destination
            && self.amount_source == other.amount_source
            && self.transform == other.transform
    }
}

/// The default modulators every instrument zone starts with (§8.4), in the
/// specification's order.
///
/// This is the specification's list but for "velocity to filter cutoff"
/// (§8.4.2), whose form implementations disagree on. The synthesizer adds
/// it to a voice's modulators as its configuration says: by default not
/// at all, as FluidSynth 2.4–2.6 actually play (see
/// `VelocityToFilter::Off`, and the "引擎" section of `spec/decisions.md`).
pub(crate) const DEFAULT_MODULATORS: &[Modulator] = &[
    // §8.4.1 velocity to initial attenuation: negative unipolar concave.
    Modulator { source: Source(0x0502), destination: 48, amount: 960, amount_source: Source::NONE, transform: 0 },
    // §8.4.3 channel pressure to vibrato LFO pitch depth.
    Modulator { source: Source(0x000D), destination: 6, amount: 50, amount_source: Source::NONE, transform: 0 },
    // §8.4.4 CC1 (modulation wheel) to vibrato LFO pitch depth.
    Modulator { source: Source(0x0081), destination: 6, amount: 50, amount_source: Source::NONE, transform: 0 },
    // §8.4.5 CC7 (volume) to initial attenuation. The specification prints
    // the source as 0x0582 but describes index 7, which is 0x0587.
    Modulator { source: Source(0x0587), destination: 48, amount: 960, amount_source: Source::NONE, transform: 0 },
    // §8.4.6 CC10 (pan) to pan: positive bipolar linear. The specification
    // says 1000 (and prints "initial attenuation" as the destination); with
    // a bipolar source that pans fully left already at CC10 = 32. 500, as
    // in FluidSynth, maps CC10 0..127 onto the whole pan range.
    Modulator { source: Source(0x028A), destination: 17, amount: 500, amount_source: Source::NONE, transform: 0 },
    // §8.4.7 CC11 (expression) to initial attenuation.
    Modulator { source: Source(0x058B), destination: 48, amount: 960, amount_source: Source::NONE, transform: 0 },
    // §8.4.8 CC91 to reverb send.
    Modulator { source: Source(0x00DB), destination: 16, amount: 200, amount_source: Source::NONE, transform: 0 },
    // §8.4.9 CC93 to chorus send.
    Modulator { source: Source(0x00DD), destination: 15, amount: 200, amount_source: Source::NONE, transform: 0 },
    // §8.4.10 pitch wheel to "initial pitch", scaled by the pitch wheel
    // sensitivity. There is no initial pitch generator; fine tune is the
    // destination, as in FluidSynth.
    Modulator { source: Source(0x020E), destination: 52, amount: 12700, amount_source: Source(0x0010), transform: 0 },
];

/// Whether a source names a defined controller with a defined curve.
fn valid_source(source: Source) -> bool {
    // Curves 0 to 3 are defined; the higher bits are reserved.
    if source.0 >> 10 > 3 {
        return false;
    }
    let index = source.index();
    if source.is_cc() {
        // §8.2.1: these are MIDI functions, not controllers. The
        // specification also reserves 33 to 63 for the LSBs of 1 to 31;
        // like FluidSynth, they are accepted as controllers of their own.
        !matches!(index, 0 | 6 | 32 | 38 | 98..=101 | 120..=127)
    } else {
        // 127 is the output of a linked modulator.
        matches!(index, 0 | 2 | 3 | 10 | 13 | 14 | 16 | 127)
    }
}

/// The modulators of a region, filled by [`Region::modulators`](super::Region::modulators).
///
/// It holds a fixed number of modulators, far more than real files give a
/// region, so that resolving a region does not allocate; modulators past
/// that are dropped and counted, as the synthesizer drops them. Make one
/// and reuse it.
#[derive(Clone)]
pub struct ModList {
    len: usize,
    items: [Modulator; ModList::CAPACITY],
    dropped: usize,
}

impl ModList {
    pub(crate) const CAPACITY: usize = 64;

    /// An empty list.
    pub fn new() -> ModList {
        const EMPTY: Modulator =
            Modulator { source: Source::NONE, destination: 0, amount: 0, amount_source: Source::NONE, transform: 0 };
        ModList { len: 0, items: [EMPTY; ModList::CAPACITY], dropped: 0 }
    }

    /// The modulators, in order.
    pub fn as_slice(&self) -> &[Modulator] {
        &self.items[..self.len]
    }

    /// How many modulators did not fit, and are not played.
    pub fn dropped(&self) -> usize {
        self.dropped
    }

    pub(crate) fn clear(&mut self) {
        self.len = 0;
        self.dropped = 0;
    }

    pub(crate) fn push(&mut self, m: Modulator) {
        if self.len == Self::CAPACITY {
            self.dropped += 1;
        } else {
            self.items[self.len] = m;
            self.len += 1;
        }
    }

    pub(crate) fn as_mut_slice(&mut self) -> &mut [Modulator] {
        &mut self.items[..self.len]
    }
}

impl Default for ModList {
    fn default() -> Self {
        ModList::new()
    }
}

impl std::fmt::Debug for ModList {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModList").field("modulators", &self.as_slice()).field("dropped", &self.dropped).finish()
    }
}
