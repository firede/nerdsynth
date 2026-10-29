# NerdSynth

English | [简体中文](README.zh.md)

[![CI](https://github.com/firede/nerdsynth/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/firede/nerdsynth/actions/workflows/ci.yml)
[![docs.rs](https://img.shields.io/docsrs/nerdsynth)](https://docs.rs/nerdsynth)

A SoundFont 2 synthesizer in Rust: complete, sample-accurate and real-time
safe. No dependencies beyond the standard library.

- **SoundFont 2.04**: every generator with its range, modulators (default
  ones included, merged across zones as the specification says), sample
  modes, exclusive classes, key and velocity overrides, 24-bit samples.
- **MIDI**: every channel message, polyphonic and channel pressure, RPNs
  0, 1, 2 and 5, GM, GM2 and GS resets, GS drum parts and common NRPNs.
  A Standard MIDI File player with loops (whole file, or CC111 and other
  loop markers), seeking, and a way to hand its events to any host frame
  by frame.
- **Sample-accurate**: events carry a frame and take effect on that frame,
  not at the next block.
- **Real-time safe**: rendering never allocates, locks or panics, and no
  input makes it produce NaN or infinity. A work budget keeps floods of
  events from making rendering fall behind; they are delayed instead.
- **Reverb and chorus** send effects.

> [!WARNING]
> NerdSynth is an early, personal project, and its API may change in any 0.x
> release.

Where the specification is silent, NerdSynth behaves as FluidSynth 2.6 does,
and a differential test bench (`tools/diff`) renders the same MIDI with both
and compares them. Where it departs from either, the reason is written down
in [`spec/decisions.md`](spec/decisions.md) (in Chinese).

## Usage

```toml
[dependencies]
nerdsynth = "0.1"
```

Playing a note:

```rust,no_run
use std::sync::Arc;
use nerdsynth::{Config, Event, SoundFont, Synth};

let sound_font = Arc::new(SoundFont::read(std::fs::File::open("piano.sf2")?)?);
let mut synth = Synth::new(sound_font, Config::new(44100))?;
synth.send(Event::NoteOn { channel: 0, key: 60, velocity: 100 });
// Released half a second in.
synth.schedule(22050, Event::NoteOff { channel: 0, key: 60 }).unwrap();

let (mut left, mut right) = (vec![0.0; 512], vec![0.0; 512]);
synth.render(&mut left, &mut right);
```

Playing a MIDI file:

```rust,no_run
use std::sync::Arc;
use nerdsynth::midi::LoopMarkers;
use nerdsynth::{Config, Sequencer, Smf, SoundFont, Synth};

let sound_font = Arc::new(SoundFont::read(std::fs::File::open("gm.sf2")?)?);
let smf = Arc::new(Smf::read(std::fs::File::open("song.mid")?, LoopMarkers::None)?);
let mut synth = Synth::new(sound_font, Config::new(48000))?;
let mut sequencer = Sequencer::new(smf, &synth);

let (mut left, mut right) = (vec![0.0; 512], vec![0.0; 512]);
while !sequencer.finished() || !synth.is_silent() {
    sequencer.render(&mut synth, &mut left, &mut right);
    // Hand `left` and `right` to the audio device.
}
```

Loading files and making synthesizers and sequencers allocate: do it off the
audio thread, then move them to it. A `SoundFont` is immutable and can be
shared by any number of synthesizers.

NerdSynth renders audio; it does not talk to audio or MIDI devices. Pair it
with a crate such as `cpal` for output and `midir` for input.

## Not supported (yet)

- SoundFont 3 (Ogg Vorbis compressed samples) and linked modulators
  (modulators whose destination is another modulator); files using them
  are rejected or the links ignored.
- XG, and MIDI files of format 2.
- SFZ and DLS.

## Robustness

Files are untrusted input: malformed SoundFonts and MIDI files are rejected
with an error or played as far as they make sense, never with a panic, and
every parser is fuzzed. The work budget is best effort: a pathological file
or event stream is delayed rather than allowed to stall the audio, but no
strict bound is promised for the time of any single operation.

## Documentation

The API documentation is on [docs.rs](https://docs.rs/nerdsynth). How NerdSynth
is built, how each behavior is sourced and tested, and why it departs from the
specification or FluidSynth where it does, is in [`spec/`](spec) (in Chinese):
[design](spec/design.md), [behavior](spec/behavior.md) and
[decisions](spec/decisions.md).

## Development

The tasks are in [`mise.toml`](mise.toml):

```sh
mise run check        # the tests, rustfmt and clippy
mise run msrv         # builds with Rust 1.85
mise run adversarial  # timed probes against hostile input, in release mode
mise run diff         # renders every case with NerdSynth and FluidSynth, and compares
```

Some tests play GeneralUser GS 2.0.3 (at `$NERDSYNTH_GU_SF2` or
`~/.nerdice/soundfonts/GeneralUser-GS-2.0.3.sf2`) and are skipped without it.
The differential test bench plays GeneralUser GS and FluidR3 GM (in
`$NERDSYNTH_SOUNDFONTS`, `~/.nerdsynth/soundfonts` or `~/.nerdice/soundfonts`)
and needs FluidSynth (`brew install fluid-synth`); see
[`tools/diff`](tools/diff/README.md) (in Chinese).
The fuzz targets are in [`fuzz/`](fuzz) (`cargo +nightly fuzz run synth_play`).

## Minimum supported Rust version

1.85. Raising it is not considered a breaking change, but it will not be
raised lightly.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.
