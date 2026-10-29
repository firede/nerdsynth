// Renders cases with SpessaSynth, offline.
//
// Usage: node render.mjs jobs.json
// jobs.json: [{ "case": "<case file>", "out": "<wav>" }, ...]; a case file
// (written by `nerdsynth-diff gen`) has the sample rate, the length in
// frames, the SoundFont, the polyphony, whether effects run, and the events
// with the frame each plays at.
//
// Events play at exactly their frame: rendering stops at each event's
// frame, the event is sent, and rendering goes on.

import { readFileSync, writeFileSync } from 'node:fs';
import { SoundBankLoader, SpessaSynthProcessor, SPESSASYNTH_GAIN_FACTOR, InterpolationTypes, SpessaLog } from 'spessasynth_core';

const BLOCK = 128; // SpessaSynth's largest render call (maxBufferSize)

SpessaLog.setLogLevel(false, false, false);

const jobs = JSON.parse(readFileSync(process.argv[2], 'utf8'));
const banks = new Map();

function bank(path) {
  let b = banks.get(path);
  if (!b) {
    const bytes = readFileSync(path);
    b = SoundBankLoader.fromArrayBuffer(bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength));
    banks.set(path, b);
  }
  return b;
}

for (const job of jobs) {
  const c = JSON.parse(readFileSync(job.case, 'utf8'));
  const synth = new SpessaSynthProcessor(c.sampleRate, { effectsEnabled: c.effects, maxBufferSize: BLOCK });
  await synth.processorInitialized;
  // Undo SpessaSynth's own 0.6 master gain, so that the output is at the
  // unity gain FluidSynth renders at with -g 1.
  synth.setSystemParameter('gain', 1 / SPESSASYNTH_GAIN_FACTOR);
  synth.setSystemParameter('voiceCap', c.polyphony);
  synth.setSystemParameter('interpolationType', InterpolationTypes.hermite);
  synth.soundBankManager.addSoundBank(bank(c.soundfont), 'main');

  const L = new Float32Array(c.frames);
  const R = new Float32Array(c.frames);
  let pos = 0;
  let next = 0;
  while (pos < c.frames) {
    while (next < c.events.length && c.events[next].frame <= pos) {
      synth.processMessage(Uint8Array.from(c.events[next].bytes));
      next++;
    }
    const until = next < c.events.length ? Math.min(c.events[next].frame, c.frames) : c.frames;
    const n = Math.min(BLOCK, until - pos);
    synth.process(L, R, pos, n);
    pos += n;
  }
  writeFileSync(job.out, wav(L, R, c.sampleRate));
}

function wav(L, R, rate) {
  const n = L.length;
  const b = Buffer.alloc(44 + n * 8);
  b.write('RIFF', 0);
  b.writeUInt32LE(36 + n * 8, 4);
  b.write('WAVEfmt ', 8);
  b.writeUInt32LE(16, 16);
  b.writeUInt16LE(3, 20); // IEEE float
  b.writeUInt16LE(2, 22);
  b.writeUInt32LE(rate, 24);
  b.writeUInt32LE(rate * 8, 28);
  b.writeUInt16LE(8, 32);
  b.writeUInt16LE(32, 34);
  b.write('data', 36);
  b.writeUInt32LE(n * 8, 40);
  for (let i = 0; i < n; i++) {
    b.writeFloatLE(L[i], 44 + i * 8);
    b.writeFloatLE(R[i], 48 + i * 8);
  }
  return b;
}
