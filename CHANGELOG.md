# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2026-10-30

The first release.

- `SoundFont`: loads SoundFont 2.04 files, 24-bit samples included, checking
  everything; immutable once loaded, shared between synthesizers.
- `Synth`: every generator and modulator, default modulators, sample modes,
  exclusive classes; every MIDI channel message, RPNs 0, 1, 2 and 5, GM, GM2
  and GS resets, GS drum parts and NRPNs; events at exact frames; reverb and
  chorus; rendering that never allocates, locks or panics; a work budget
  against floods of events.
- `Smf` and `Sequencer`: Standard MIDI Files of format 0 and 1, loops with
  loop markers, seeking, playing through a `Synth` or handing events to a
  host.
- `Error`: structured errors telling invalid files, unsupported features and
  settings out of range apart.

[0.1.0]: https://github.com/firede/nerdsynth/releases/tag/v0.1.0
