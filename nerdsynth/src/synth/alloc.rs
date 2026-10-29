//! Voice slots for the regions of a note on: free slots, the voices to take
//! when the polyphony is reached, and the fading voices to cut when every
//! slot is busy. The first voice a note on takes is found by a scan; the
//! lists are made once a note on needs more, in memory allocated with the
//! synthesizer. A note on thus costs O(V) for V slots when it takes at most
//! one voice, O(V log V) however many regions it has, and allocates
//! nothing.

use super::channel::Channel;
use super::voice::Voice;

pub(crate) struct Allocator {
    /// The free slots, the lowest index last.
    free: Vec<usize>,
    /// The live voices by priority, the one to take first last.
    victims: Vec<(f64, usize)>,
    victims_ready: bool,
    /// Voices taken by this note on.
    taken: usize,
    /// The fading voices, the one closest to its end last.
    fading: Vec<(u64, usize)>,
    /// The voices the polyphony counts.
    live: usize,
    /// The exclusive classes of the note on's voices.
    pub(crate) classes: Vec<i32>,
}

impl Allocator {
    /// Room for `slots` voice slots and `max_voices` voices a note.
    pub(crate) fn new(slots: usize, max_voices: usize) -> Allocator {
        Allocator {
            free: Vec::with_capacity(slots),
            victims: Vec::with_capacity(slots),
            victims_ready: false,
            taken: 0,
            fading: Vec::with_capacity(slots),
            live: 0,
            classes: Vec::with_capacity(max_voices),
        }
    }

    /// Starts a note on: takes stock of the slots.
    pub(crate) fn begin(&mut self, voices: &[Voice]) {
        self.free.clear();
        self.free.extend((0..voices.len()).rev().filter(|&i| voices[i].is_free()));
        self.live = voices.iter().filter(|v| v.is_live()).count();
        self.victims_ready = false;
        self.taken = 0;
        self.fading.clear();
        self.classes.clear();
    }

    /// A free slot for a voice. If the polyphony is reached, the voice of
    /// the lowest priority fades out (FluidSynth's overflow priority, the
    /// lowest index first on a tie); if every slot is busy, the fading voice
    /// closest to its end stops at once.
    pub(crate) fn slot(
        &mut self,
        voices: &mut [Voice],
        max_voices: usize,
        now: u64,
        sample_rate: f64,
        channels: &[Channel; 16],
    ) -> usize {
        if self.live >= max_voices && self.taken == 0 {
            // The lowest priority, the lowest index on a tie.
            let mut victim = None;
            let mut lowest = f64::INFINITY;
            for (i, v) in voices.iter().enumerate() {
                if v.is_live() {
                    let p = v.priority(now, sample_rate, channels[v.channel as usize & 15].drum);
                    if p < lowest {
                        lowest = p;
                        victim = Some(i);
                    }
                }
            }
            if let Some(i) = victim {
                voices[i].fade();
                self.live -= 1;
                self.taken += 1;
            }
        } else if self.live >= max_voices {
            for pass in 0..2 {
                if pass == 1 || !self.victims_ready {
                    self.victims.clear();
                    for (i, v) in voices.iter().enumerate() {
                        if v.is_live() {
                            let drum = channels[v.channel as usize & 15].drum;
                            self.victims.push((v.priority(now, sample_rate, drum), i));
                        }
                    }
                    self.victims.sort_unstable_by(|a, b| b.0.total_cmp(&a.0).then(b.1.cmp(&a.1)));
                    self.victims_ready = true;
                }
                if let Some(i) = self.take_victim(voices) {
                    voices[i].fade();
                    self.live -= 1;
                    self.taken += 1;
                    break;
                }
            }
        }
        if let Some(i) = self.free.pop() {
            return i;
        }
        if self.fading.is_empty() {
            for (i, v) in voices.iter().enumerate() {
                if v.fading {
                    self.fading.push((v.fade_left(), i));
                }
            }
            self.fading.sort_unstable_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
        }
        let i = self.fading.pop().map_or(0, |(_, i)| i);
        voices[i].stop();
        i
    }

    /// The next voice to take that is still live.
    fn take_victim(&mut self, voices: &[Voice]) -> Option<usize> {
        while let Some((_, i)) = self.victims.pop() {
            if voices[i].is_live() {
                return Some(i);
            }
        }
        None
    }

    /// The slot given by [`slot`](Self::slot) holds a voice now, or, if
    /// `started` is false, is free again.
    pub(crate) fn used(&mut self, slot: usize, started: bool) {
        if started {
            self.live += 1;
        } else {
            self.free.push(slot);
        }
    }
}
