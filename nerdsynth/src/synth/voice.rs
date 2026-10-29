//! A voice: one region of a note, from its generators and modulators to
//! the sound it mixes into the buses.
//!
//! The control values (envelopes, LFOs, modulators, filter cutoff) are
//! computed at the start of each segment of 64 frames, for the segment's
//! end; the gains move linearly across the segment and the filter glides
//! to the cutoff (see `dsp::filter`). Each voice keeps its own segments: an event that
//! changes the voice (a note off, a fade) or its channel cuts the segment
//! at the event's frame, and other events leave it alone. The state is
//! committed at the start of each segment by the frames actually rendered,
//! so a segment cut short leaves no trace.
//!
//! The semantics are FluidSynth's (`src/synth/fluid_voice.c`,
//! `src/rvoice/fluid_rvoice.c`) but for the frame accuracy and the fades.

use super::channel::Channel;
use super::dsp::conv::{PEAK_ATTENUATION, cb2amp, ct2hz, pan, tc2sec, tc2sec_attack, tc2sec_delay};
use super::dsp::envelope::{Envelope, Params, Stage};
use super::dsp::filter::{Filter, SinCos, q_linear};
use super::dsp::lfo::Lfo;
use super::dsp::oscillator::{Bounds, Data, Oscillator, Positions};
use super::modulate::{self, ModInput};
use super::{Interpolation, VelocityToFilter};
use crate::soundfont::GeneratorType as G;
use crate::soundfont::{GENERATOR_COUNT, ModList, Modulator, Region, Sample, generator_range};

const N: usize = GENERATOR_COUNT;

/// FluidSynth's noise floor: a voice whose gain can no longer rise above it
/// is done (`FLUID_NOISE_FLOOR`).
const NOISE_FLOOR: f64 = 2e-7;

/// The fade of a voice stopped by stealing or All Sound Off (E-06).
pub(crate) const FADE_SECONDS: f64 = 0.002;

/// The volume envelope's release, in timecents, of a note stopped by an
/// exclusive class: about 315 ms (FluidSynth's `fluid_voice_kill_excl`).
const EXCLUSIVE_RELEASE: f64 = -2000.0;

/// Where a voice is in its note.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Status {
    /// Not sounding; the slot is free.
    Free,
    /// The key is down.
    On,
    /// The key is up, the hold pedal down.
    Sustained,
    /// The key is up, held by the sostenuto pedal.
    Sostenuto,
    /// Releasing.
    Released,
}

/// What a voice's control computations need of the synthesizer.
pub(crate) struct Context {
    pub(crate) sample_rate: f64,
    pub(crate) interpolation: Interpolation,
    pub(crate) velocity_to_filter: VelocityToFilter,
    pub(crate) cc74_to_filter: bool,
    /// The filter's table at the sample rate.
    pub(crate) sincos: SinCos,
    /// The frames a note sounds at least before it releases.
    pub(crate) min_note_frames: u64,
}

#[derive(Clone)]
pub(crate) struct Voice {
    pub(crate) status: Status,
    /// Fading out over [`FADE_SECONDS`]; it then stops.
    pub(crate) fading: bool,
    fade_left: u64,
    fade_total: u64,
    pub(crate) channel: u8,
    /// The key of the note on, which its note off matches.
    pub(crate) key: u8,
    /// The note on's number: the voices of a note share it.
    pub(crate) id: u64,
    pub(crate) preset: usize,
    /// The frame it started at.
    pub(crate) start: u64,
    pub(crate) exclusive_class: i32,
    velocity: u8,
    /// The key and velocity after the keynum and velocity generators.
    actual_key: u8,
    actual_velocity: u8,
    /// The region's generators, initialAttenuation scaled by 0.4.
    base: [f64; N],
    /// With the modulators and the channel's offsets added, clamped.
    pub(crate) gens: [f64; N],
    mods: ModList,
    seen_version: u64,
    seen_poly: u8,
    fresh: bool,
    /// The lowest the attenuation may go with the modulators that follow the
    /// channel (FluidSynth's `fluid_voice_get_lower_boundary_for_attenuation`).
    min_attenuation: f64,

    osc: Oscillator,
    /// The sample's positions; the offsets come from the generators, with
    /// their modulators, when the voice first renders.
    positions: Positions,
    data_len: usize,
    sample_mode: u8,
    /// The sample's rate over the output's.
    rate_ratio: f64,
    /// The cents the sample sounds at its rate: the root key less the
    /// correction.
    root_cents: f64,
    /// The pitch of the key in cents, before tuning (FluidSynth's
    /// GEN_PITCH): scaleTuning scales only the distance from the root.
    nominal_pitch: f64,

    vol: Envelope,
    vol_p: Params,
    modenv: Envelope,
    mod_p: Params,
    mod_lfo: Lfo,
    vib_lfo: Lfo,
    mod_lfo_delay: u64,
    mod_lfo_step: f64,
    vib_lfo_delay: u64,
    vib_lfo_step: f64,

    filter: Filter,

    amp: f32,
    amp_step: f32,
    bus: [f32; 4],
    bus_step: [f32; 4],
    step: u64,
    /// Frames rendered since the state was last committed.
    pending: u64,
    /// Frames left in the segment.
    segment_left: usize,
    /// Changed by an event since the segment began: the segment is cut.
    dirty: bool,
    /// The frames committed since the start.
    played: u64,
    /// Released before it had sounded the shortest note: the frame since
    /// the start its envelopes release at.
    release_at: Option<u64>,
}

/// The frames of `seconds` at a sample rate.
fn frames(sample_rate: f64, seconds: f64) -> u64 {
    let f = sample_rate * seconds;
    if f.is_finite() && f > 0.0 { f as u64 } else { 0 }
}

/// FluidSynth counts the envelopes' stages in blocks of 64 frames; the
/// stage lengths are its, in frames, so that notes start and end as there.
const ENV_BLOCK: f64 = 64.0;

/// A delay: whole blocks, rounded down (so the default 1 ms is none below
/// 65.5 kHz).
fn delay_frames(sample_rate: f64, seconds: f64) -> u64 {
    frames(sample_rate / ENV_BLOCK, seconds) * ENV_BLOCK as u64
}

/// An attack or a release: one block more than the whole blocks.
fn ramp_frames(sample_rate: f64, seconds: f64) -> u64 {
    (1 + frames(sample_rate / ENV_BLOCK, seconds)) * ENV_BLOCK as u64
}

/// A hold or a decay: the nearest whole number of blocks.
fn hold_frames(sample_rate: f64, seconds: f64) -> u64 {
    frames(sample_rate / ENV_BLOCK, seconds + 0.5 * ENV_BLOCK / sample_rate) * ENV_BLOCK as u64
}

/// A sample's positions, without the offsets.
fn sample_positions(sample: &Sample) -> Positions {
    Positions {
        start: i64::from(sample.start),
        end: i64::from(sample.end),
        loop_start: i64::from(sample.loop_start),
        loop_end: i64::from(sample.loop_end),
        start_offset: 0,
        end_offset: 0,
        loop_start_offset: 0,
        loop_end_offset: 0,
    }
}

/// Whether an instrument zone of `sample` and `generators` (its merged
/// generators) has anything to play from `data_len` points of data, with
/// its offset generators before modulation; whether it is in ROM aside.
/// Only instrument zones set the offsets, so a SoundFont works this out
/// once for each zone when it loads, and a note on takes no voice for a
/// zone that has not.
pub(crate) fn zone_plays(sample: &Sample, generators: &[Option<i16>; N], data_len: usize) -> bool {
    let g = |generator: G| i64::from(generators[generator as usize].unwrap_or(0));
    let offset = |fine: G, coarse: G| g(fine) + 32768 * g(coarse);
    let p = Positions {
        start_offset: offset(G::StartAddrsOffset, G::StartAddrsCoarseOffset),
        end_offset: offset(G::EndAddrsOffset, G::EndAddrsCoarseOffset),
        loop_start_offset: offset(G::StartloopAddrsOffset, G::StartloopAddrsCoarseOffset),
        loop_end_offset: offset(G::EndloopAddrsOffset, G::EndloopAddrsCoarseOffset),
        ..sample_positions(sample)
    };
    Bounds::new(p, data_len).is_some()
}

impl Voice {
    pub(crate) fn new() -> Voice {
        Voice {
            status: Status::Free,
            fading: false,
            fade_left: 0,
            fade_total: 1,
            channel: 0,
            key: 0,
            id: 0,
            preset: 0,
            start: 0,
            exclusive_class: 0,
            velocity: 0,
            actual_key: 0,
            actual_velocity: 0,
            base: [0.0; N],
            gens: [0.0; N],
            mods: ModList::new(),
            seen_version: 0,
            seen_poly: 0,
            fresh: true,
            min_attenuation: 0.0,
            osc: Oscillator::new(Bounds { start: 0, end: 0, loop_start: 0, loop_end: 0, can_loop: false }),
            positions: Positions {
                start: 0,
                end: 0,
                loop_start: 0,
                loop_end: 0,
                start_offset: 0,
                end_offset: 0,
                loop_start_offset: 0,
                loop_end_offset: 0,
            },
            data_len: 0,
            sample_mode: 0,
            rate_ratio: 1.0,
            root_cents: 6000.0,
            nominal_pitch: 6000.0,
            vol: Envelope::new(),
            vol_p: Params::default(),
            modenv: Envelope::new(),
            mod_p: Params::default(),
            mod_lfo: Lfo::new(),
            vib_lfo: Lfo::new(),
            mod_lfo_delay: 0,
            mod_lfo_step: 0.0,
            vib_lfo_delay: 0,
            vib_lfo_step: 0.0,
            filter: Filter::default(),
            amp: 0.0,
            amp_step: 0.0,
            bus: [0.0; 4],
            bus_step: [0.0; 4],
            step: 0,
            pending: 0,
            segment_left: 0,
            dirty: false,
            played: 0,
            release_at: None,
        }
    }

    pub(crate) fn is_free(&self) -> bool {
        self.status == Status::Free
    }

    /// Sounding and not fading: one of the voices the polyphony counts.
    pub(crate) fn is_live(&self) -> bool {
        self.status != Status::Free && !self.fading
    }

    /// Sets the voice up to play `region`. Gives false, leaving the voice
    /// free, when the region's sample has nothing to play.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn start(
        &mut self,
        region: &Region<'_>,
        data_len: usize,
        channel: u8,
        key: u8,
        velocity: u8,
        id: u64,
        preset: usize,
        now: u64,
        ctx: &Context,
    ) -> bool {
        // Clamped once modulated, in `refresh`.
        let g = region.sums();
        let sample = region.sample();
        self.positions = sample_positions(sample);
        self.data_len = data_len;
        // Nothing to play even before the offsets.
        let Some(bounds) = Bounds::new(self.positions, data_len) else {
            self.status = Status::Free;
            return false;
        };
        self.osc = Oscillator::new(bounds);
        for (b, v) in self.base.iter_mut().zip(g) {
            *b = f64::from(v);
        }
        self.base[G::InitialAttenuation as usize] *= 0.4;
        region.modulators(&mut self.mods);
        let extra: &[Modulator] = match ctx.velocity_to_filter {
            VelocityToFilter::Off => &[],
            VelocityToFilter::Switched => &[modulate::FLUID_VEL_TO_FILTER],
            VelocityToFilter::Linear => &[modulate::LINEAR_VEL_TO_FILTER],
        };
        let cc74: &[Modulator] = if ctx.cc74_to_filter { &[modulate::CC74_TO_FILTER] } else { &[] };
        for m in extra.iter().chain(cc74) {
            if !self.mods.as_slice().iter().any(|k| k.is_identical(m)) {
                self.mods.push(*m);
            }
        }

        self.status = Status::On;
        self.fading = false;
        self.fade_left = 0;
        self.fade_total = frames(ctx.sample_rate, FADE_SECONDS).max(1);
        self.channel = channel;
        self.key = key;
        self.velocity = velocity;
        self.id = id;
        self.preset = preset;
        self.start = now;
        // The index-like generators, which no modulator changes: clamped.
        let c = region.generators();
        self.exclusive_class = c[G::ExclusiveClass as usize];
        self.actual_key = if c[G::Keynum as usize] >= 0 { c[G::Keynum as usize].clamp(0, 127) as u8 } else { key };
        self.actual_velocity =
            if c[G::Velocity as usize] > 0 { c[G::Velocity as usize].clamp(0, 127) as u8 } else { velocity };
        self.fresh = true;

        self.sample_mode = (c[G::SampleModes as usize] & 3) as u8;
        let rate = if sample.sample_rate == 0 { ctx.sample_rate } else { f64::from(sample.sample_rate) };
        self.rate_ratio = rate / ctx.sample_rate;
        let root = if c[G::OverridingRootKey as usize] > -1 {
            c[G::OverridingRootKey as usize]
        } else {
            i32::from(sample.original_pitch)
        };
        self.root_cents = f64::from(root) * 100.0 - f64::from(sample.pitch_correction);
        // scaleTuning summed and clamped to its range (E-03), as the other
        // generators; no modulator changes it.
        let scale = self.base[G::ScaleTuning as usize].clamp(0.0, 1200.0);
        self.nominal_pitch = scale * (f64::from(self.actual_key) - f64::from(root)) + f64::from(root) * 100.0;

        self.vol = Envelope::new();
        self.modenv = Envelope::new();
        self.mod_lfo = Lfo::new();
        self.vib_lfo = Lfo::new();
        self.filter = Filter::default();
        self.amp = 0.0;
        self.amp_step = 0.0;
        self.bus = [0.0; 4];
        self.bus_step = [0.0; 4];
        self.step = 0;
        self.pending = 0;
        self.segment_left = 0;
        self.dirty = false;
        self.played = 0;
        self.release_at = None;
        true
    }

    /// Recomputes the generators from the modulators, the channel's state and
    /// its generator offsets, and what depends on them.
    fn refresh(&mut self, ch: &Channel, ctx: &Context) {
        let input = ModInput {
            cc: &ch.cc,
            key: self.actual_key,
            velocity: self.actual_velocity,
            poly_pressure: ch.key_pressure[self.key as usize & 127],
            channel_pressure: ch.channel_pressure,
            pitch_bend: ch.pitch_bend,
            bend_range: ch.bend_range,
            mod_depth_range: ch.mod_depth_range,
            mute_fluid_vel_to_filter: ctx.velocity_to_filter == VelocityToFilter::Off,
        };
        let mut sum = [0.0f64; N];
        let mut reduction = 0.0;
        for m in self.mods.as_slice() {
            let dest = m.destination as usize;
            if m.destination & 0x8000 != 0 || dest >= N {
                continue;
            }
            let v = modulate::value(m, &input);
            if v.is_finite() {
                sum[dest] += v;
            }
            if dest == G::InitialAttenuation as usize && modulate::is_dynamic(m) {
                let bipolar = m.source.is_bipolar() || m.amount_source.is_bipolar();
                let min = if bipolar || m.amount < 0 { -f64::from(m.amount).abs() } else { 0.0 };
                if v > min {
                    reduction += v - min;
                }
            }
        }
        for (i, s) in sum.iter().enumerate() {
            let v = self.base[i] + s + ch.gen_offsets[i];
            self.gens[i] = match generator_range(i) {
                Some((lo, hi)) => v.clamp(f64::from(lo), f64::from(hi)),
                None => v,
            };
        }
        self.min_attenuation = (self.gens[G::InitialAttenuation as usize] - reduction).max(0.0);
        self.seen_version = ch.version;
        self.seen_poly = input.poly_pressure;
        self.derive(ctx.sample_rate);
        self.place();
    }

    /// The sample positions with the offset generators (FluidSynth applies
    /// their modulators too). The start counts when the voice starts; the
    /// end and the loop follow the generators while it plays.
    fn place(&mut self) {
        let g = &self.gens;
        let offset = |fine: usize, coarse: usize| g[fine] as i64 + 32768 * g[coarse] as i64;
        let mut p = self.positions;
        p.start_offset = offset(G::StartAddrsOffset as usize, G::StartAddrsCoarseOffset as usize);
        p.end_offset = offset(G::EndAddrsOffset as usize, G::EndAddrsCoarseOffset as usize);
        p.loop_start_offset = offset(G::StartloopAddrsOffset as usize, G::StartloopAddrsCoarseOffset as usize);
        p.loop_end_offset = offset(G::EndloopAddrsOffset as usize, G::EndloopAddrsCoarseOffset as usize);
        match Bounds::new(p, self.data_len) {
            Some(b) if self.fresh => self.osc = Oscillator::new(b),
            Some(b) => self.osc.bounds = Bounds { start: self.osc.bounds.start, ..b },
            None if self.fresh => self.osc.finished = true,
            None => {}
        }
    }

    /// The envelope and LFO parameters from the generators.
    fn derive(&mut self, sr: f64) {
        let g = &self.gens;
        let keysteps = 60.0 - f64::from(self.actual_key);
        let hold = |base: usize, key: usize| {
            let tc = (g[base] + g[key] * keysteps).clamp(-12000.0, 5000.0);
            // FluidSynth: the most negative hold is none.
            if g[base] + g[key] * keysteps <= -32768.0 { 0 } else { hold_frames(sr, tc2sec(tc)) }
        };
        let decay = |base: usize, key: usize| {
            let tc = (g[base] + g[key] * keysteps).clamp(-12000.0, 8000.0);
            hold_frames(sr, tc2sec(tc))
        };
        self.vol_p = Params {
            delay: delay_frames(sr, tc2sec_delay(g[G::DelayVolEnv as usize])),
            attack: ramp_frames(sr, tc2sec_attack(g[G::AttackVolEnv as usize])),
            hold: hold(G::HoldVolEnv as usize, G::KeynumToVolEnvHold as usize),
            decay: decay(G::DecayVolEnv as usize, G::KeynumToVolEnvDecay as usize),
            sustain: (1.0 - 0.001 * g[G::SustainVolEnv as usize]).clamp(0.0, 1.0),
            // FluidSynth: no release shorter than -7200 timecents (16 ms).
            release: ramp_frames(sr, tc2sec_attack(g[G::ReleaseVolEnv as usize].max(-7200.0))),
        };
        self.mod_p = Params {
            delay: delay_frames(sr, tc2sec_delay(g[G::DelayModEnv as usize])),
            attack: ramp_frames(sr, tc2sec_attack(g[G::AttackModEnv as usize])),
            hold: hold(G::HoldModEnv as usize, G::KeynumToModEnvHold as usize),
            decay: decay(G::DecayModEnv as usize, G::KeynumToModEnvDecay as usize),
            sustain: (1.0 - 0.001 * g[G::SustainModEnv as usize]).clamp(0.0, 1.0),
            release: ramp_frames(sr, tc2sec_attack(g[G::ReleaseModEnv as usize])),
        };
        self.mod_lfo_delay = frames(sr, tc2sec_delay(g[G::DelayModLfo as usize]));
        self.mod_lfo_step = ct2hz(g[G::FreqModLfo as usize].clamp(-16000.0, 4500.0)) / sr;
        self.vib_lfo_delay = frames(sr, tc2sec_delay(g[G::DelayVibLfo as usize]));
        self.vib_lfo_step = ct2hz(g[G::FreqVibLfo as usize].clamp(-16000.0, 4500.0)) / sr;
    }

    /// Advances the envelopes, LFOs and fade by the frames rendered since
    /// the last commit.
    fn commit(&mut self) {
        let n = std::mem::take(&mut self.pending);
        if n == 0 {
            return;
        }
        self.played += n;
        self.vol.advance(&self.vol_p, n);
        self.modenv.advance(&self.mod_p, n);
        self.mod_lfo.advance(n, self.mod_lfo_delay, self.mod_lfo_step);
        self.vib_lfo.advance(n, self.vib_lfo_delay, self.vib_lfo_step);
        if self.fading {
            self.fade_left = self.fade_left.saturating_sub(n);
        }
    }

    /// The gain of the volume envelope, the attenuation and the modulation
    /// LFO, as FluidSynth's `fluid_rvoice_calc_amp`.
    fn amplitude(&self, vol: &Envelope, mod_lfo: f64) -> f64 {
        let att = self.gens[G::InitialAttenuation as usize].clamp(0.0, 1440.0);
        let lfo_cb = mod_lfo * -self.gens[G::ModLfoToVolume as usize];
        match vol.stage {
            Stage::Delay | Stage::Finished => 0.0,
            Stage::Attack => cb2amp(att) * cb2amp(lfo_cb) * vol.value,
            _ => cb2amp(att + PEAK_ATTENUATION * (1.0 - vol.value) + lfo_cb),
        }
    }

    /// The modulation envelope's output: convex in the attack (§8.1.2 #26,
    /// as FluidSynth).
    fn modenv_out(e: &Envelope) -> f64 {
        if e.stage == Stage::Attack { super::dsp::conv::convex(127.0 * e.value) } else { e.value }
    }

    /// Whether a segment must begin before the next frame: the segment is
    /// over, or an event changed the voice or its channel `ch`.
    pub(crate) fn needs_begin(&self, ch: &Channel) -> bool {
        self.segment_left == 0
            || self.dirty
            || self.fresh
            || self.seen_version != ch.version
            || self.seen_poly != ch.key_pressure[self.key as usize & 127]
    }

    /// The frames left in the segment.
    pub(crate) fn segment_left(&self) -> usize {
        self.segment_left
    }

    /// The modulators it plays.
    pub(crate) fn modulators(&self) -> usize {
        self.mods.as_slice().len()
    }

    /// Starts a segment of `len` frames: commits the frames rendered, stops
    /// the voice if it is done, recomputes the modulators if the channel
    /// changed, and sets the gains and filter moving to their values at the
    /// segment's end.
    pub(crate) fn begin(&mut self, len: usize, ch: &Channel, ctx: &Context) {
        if self.status == Status::Free {
            return;
        }
        self.commit();
        self.dirty = false;
        let mut len = len.max(1);
        if let Some(at) = self.release_at {
            if self.played >= at {
                self.release_at = None;
                self.release_envelopes();
            } else {
                len = len.min((at - self.played) as usize);
            }
        }
        self.segment_left = len;
        let done = self.osc.finished
            || self.vol.stage == Stage::Finished
            || (self.fading && self.fade_left == 0)
            || (self.vol.stage > Stage::Attack
                && self.vol.stage != Stage::Hold
                && cb2amp(self.min_attenuation) * self.vol.value < NOISE_FLOOR);
        if done {
            self.status = Status::Free;
            self.fading = false;
            return;
        }
        if self.fresh || self.seen_version != ch.version || self.seen_poly != ch.key_pressure[self.key as usize & 127] {
            self.refresh(ch, ctx);
        }
        let n = len as u64;
        let (mut vol, mut modenv, mut mod_lfo, mut vib_lfo) = (self.vol, self.modenv, self.mod_lfo, self.vib_lfo);
        vol.advance(&self.vol_p, n);
        modenv.advance(&self.mod_p, n);
        mod_lfo.advance(n, self.mod_lfo_delay, self.mod_lfo_step);
        vib_lfo.advance(n, self.vib_lfo_delay, self.vib_lfo_step);
        let g = &self.gens;

        // Pitch: the middle of the segment.
        let mod_lfo_mid = 0.5 * (self.mod_lfo.value() + mod_lfo.value());
        let vib_lfo_mid = 0.5 * (self.vib_lfo.value() + vib_lfo.value());
        let modenv_mid = 0.5 * (Self::modenv_out(&self.modenv) + Self::modenv_out(&modenv));
        let cents = self.nominal_pitch
            + 100.0 * g[G::CoarseTune as usize]
            + g[G::FineTune as usize]
            + mod_lfo_mid * g[G::ModLfoToPitch as usize]
            + vib_lfo_mid * g[G::VibLfoToPitch as usize]
            + modenv_mid * g[G::ModEnvToPitch as usize];
        self.step = Oscillator::step(((cents - self.root_cents) / 1200.0).exp2() * self.rate_ratio);

        // Filter: the end of the segment.
        let fc = g[G::InitialFilterFc as usize].clamp(1500.0, 13500.0)
            + mod_lfo.value() * g[G::ModLfoToFilterFc as usize]
            + Self::modenv_out(&modenv) * g[G::ModEnvToFilterFc as usize];
        self.filter.set(fc, q_linear(g[G::InitialFilterQ as usize]), &ctx.sincos);

        // Gains: the end of the segment.
        let mut amp = self.amplitude(&vol, mod_lfo.value());
        if self.fading {
            amp *= self.fade_left.saturating_sub(n) as f64 / self.fade_total as f64;
        }
        let amp = if amp.is_finite() { amp as f32 } else { 0.0 };
        self.amp_step = if self.fading && self.fade_left < n {
            // The fade ends within the segment: reach 0 at its end, and stay
            // there (the filter's gain does not go below 0).
            -self.amp / self.fade_left.max(1) as f32
        } else {
            (amp - self.amp) / n as f32
        };
        let target = [
            pan(g[G::Pan as usize], true) as f32,
            pan(g[G::Pan as usize], false) as f32,
            (g[G::ReverbEffectsSend as usize] / 1000.0).clamp(0.0, 1.0) as f32,
            (g[G::ChorusEffectsSend as usize] / 1000.0).clamp(0.0, 1.0) as f32,
        ];
        for (i, t) in target.iter().enumerate() {
            if self.fresh {
                self.bus[i] = *t;
            }
            self.bus_step[i] = (t - self.bus[i]) / n as f32;
        }
        self.fresh = false;
    }

    /// Renders `len` frames, at most what is left of the segment, into the
    /// buses: left, right, reverb and chorus.
    pub(crate) fn render(&mut self, len: usize, data: Data<'_>, ctx: &Context, buf: &mut [f32], out: [&mut [f32]; 4]) {
        if self.status == Status::Free {
            return;
        }
        let buf = &mut buf[..len];
        let looping = self.sample_mode == 1 || (self.sample_mode == 3 && self.vol.stage < Stage::Release);
        self.osc.render(data, buf, self.step, looping, ctx.interpolation);
        if !self.filter.process(buf, &mut self.amp, self.amp_step, &ctx.sincos) {
            // A runaway filter: stop rather than make noise.
            self.stop();
            return;
        }
        for (k, bus) in out.into_iter().enumerate() {
            let mut gain = self.bus[k];
            let step = self.bus_step[k];
            if gain == 0.0 && step == 0.0 {
                continue;
            }
            for (o, x) in bus[..len].iter_mut().zip(buf.iter()) {
                *o += x * gain;
                gain += step;
            }
            self.bus[k] = gain;
        }
        self.pending += len as u64;
        self.segment_left = self.segment_left.saturating_sub(len);
    }

    /// Releases the note. The envelopes go to their release once the note
    /// has sounded [`Context::min_note_frames`], at once if it has
    /// (FluidSynth's `fluid_rvoice_noteoff_LOCAL` with `min_ticks`).
    pub(crate) fn release(&mut self, ctx: &Context) {
        if self.status == Status::Free {
            return;
        }
        self.commit();
        self.status = Status::Released;
        // The segment may go past the frame to release at.
        self.dirty = true;
        if self.played < ctx.min_note_frames {
            self.release_at = Some(ctx.min_note_frames);
        } else {
            self.release_at = None;
            self.release_envelopes();
        }
    }

    /// The envelopes go to their release, from the same gain.
    fn release_envelopes(&mut self) {
        if self.vol.stage == Stage::Attack && self.vol.value > 0.0 {
            let lfo = self.mod_lfo.value() * -self.gens[G::ModLfoToVolume as usize];
            let amp = self.vol.value * cb2amp(lfo);
            let v = -((-200.0 / std::f64::consts::LN_10 * amp.ln() - lfo) / PEAK_ATTENUATION - 1.0);
            self.vol.value = if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
        }
        if self.modenv.stage == Stage::Attack && self.modenv.value > 0.0 {
            self.modenv.value = super::dsp::conv::convex(127.0 * self.modenv.value).clamp(0.0, 1.0);
        }
        self.vol.release();
        self.modenv.release();
    }

    /// Stops the note for an exclusive class (E-05), as FluidSynth's
    /// `fluid_voice_kill_excl`: the volume envelope's release becomes
    /// [`EXCLUSIVE_RELEASE`], its modulators and the channel's offset still
    /// added, and the note releases whatever the pedals; a note already
    /// releasing goes on from its level at the new rate. The class goes to
    /// 0, so it happens once.
    pub(crate) fn cut(&mut self, ch: &Channel, ctx: &Context) {
        if !self.is_live() {
            return;
        }
        self.commit();
        self.exclusive_class = 0;
        self.base[G::ReleaseVolEnv as usize] = EXCLUSIVE_RELEASE;
        self.refresh(ch, ctx);
        self.release(ctx);
    }

    /// Fades the voice out over [`FADE_SECONDS`] and stops it.
    pub(crate) fn fade(&mut self) {
        if self.status == Status::Free || self.fading {
            return;
        }
        self.commit();
        self.fading = true;
        self.dirty = true;
        self.fade_left = self.fade_total;
        if self.status != Status::Released {
            self.status = Status::Released;
        }
    }

    /// Stops the voice at once.
    pub(crate) fn stop(&mut self) {
        self.status = Status::Free;
        self.fading = false;
        self.pending = 0;
    }

    /// FluidSynth's overflow priority (`fluid_voice_get_overflow_prio` with
    /// the default scores): the lowest is stolen first. Released voices are
    /// cheap, sustained ones less so, drums dear; young and loud voices are
    /// dear.
    pub(crate) fn priority(&self, now: u64, sample_rate: f64, drum: bool) -> f64 {
        let mut p = if drum {
            4000.0
        } else if self.status == Status::Released {
            -2000.0
        } else if matches!(self.status, Status::Sustained | Status::Sostenuto) {
            -1000.0
        } else {
            0.0
        };
        let age = now.saturating_sub(self.start).max(1) as f64;
        p += 1000.0 * sample_rate / age;
        p += 500.0 / self.gens[G::InitialAttenuation as usize].clamp(0.0, 1440.0).max(0.1);
        p
    }

    /// The fade frames left, for choosing which fading voice to cut when no
    /// slot is free.
    pub(crate) fn fade_left(&self) -> u64 {
        self.fade_left
    }

    /// For tests: the volume envelope.
    #[cfg(test)]
    pub(crate) fn volume_envelope(&self) -> (Stage, f64) {
        (self.vol.stage, self.vol.value)
    }

    /// For tests: sets the frames since the start, as if it had played that
    /// long (E-08).
    #[cfg(test)]
    pub(crate) fn age_by(&mut self, frames: u64) {
        self.start = self.start.saturating_sub(frames);
    }
}
