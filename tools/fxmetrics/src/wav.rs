//! Stereo 32-bit float WAV files (as `tools/diff` has them).

use std::path::Path;

/// Stereo audio, one buffer per channel.
#[derive(Clone, Debug, Default)]
pub struct Audio {
    pub rate: u32,
    pub left: Vec<f32>,
    pub right: Vec<f32>,
}

impl Audio {
    pub fn silent(rate: u32, frames: usize) -> Audio {
        Audio { rate, left: vec![0.0; frames], right: vec![0.0; frames] }
    }

    pub fn frames(&self) -> usize {
        self.left.len()
    }

    /// Cuts or pads with silence to `frames`.
    pub fn fit(&mut self, frames: usize) {
        self.left.resize(frames, 0.0);
        self.right.resize(frames, 0.0);
    }

    /// Frames `a..b`, as f64.
    pub fn slice(&self, a: usize, b: usize) -> (Vec<f64>, Vec<f64>) {
        let b = b.min(self.frames());
        let a = a.min(b);
        (
            self.left[a..b].iter().map(|&x| f64::from(x)).collect(),
            self.right[a..b].iter().map(|&x| f64::from(x)).collect(),
        )
    }
}

pub fn write(path: &Path, audio: &Audio) -> std::io::Result<()> {
    let n = audio.frames();
    let mut b = Vec::with_capacity(44 + n * 8);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + n as u32 * 8).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&3u16.to_le_bytes()); // IEEE float
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&audio.rate.to_le_bytes());
    b.extend_from_slice(&(audio.rate * 8).to_le_bytes());
    b.extend_from_slice(&8u16.to_le_bytes());
    b.extend_from_slice(&32u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&(n as u32 * 8).to_le_bytes());
    for i in 0..n {
        b.extend_from_slice(&audio.left[i].to_le_bytes());
        b.extend_from_slice(&audio.right[i].to_le_bytes());
    }
    std::fs::write(path, b)
}

/// Reads a stereo WAV of 32-bit floats or 16-bit integers, plain or
/// WAVE_FORMAT_EXTENSIBLE, skipping other chunks.
pub fn read(path: &Path) -> Result<Audio, String> {
    let b = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let bad = |m: &str| format!("{}: {m}", path.display());
    if b.len() < 12 || &b[0..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        return Err(bad("not a WAV file"));
    }
    let u16_at = |p: usize| u16::from_le_bytes([b[p], b[p + 1]]);
    let u32_at = |p: usize| u32::from_le_bytes([b[p], b[p + 1], b[p + 2], b[p + 3]]);
    let mut fmt: Option<(u16, u16, u32, u16)> = None;
    let mut p = 12;
    while p + 8 <= b.len() {
        let id = &b[p..p + 4];
        let size = u32_at(p + 4) as usize;
        let body = p + 8;
        let end = (body + size).min(b.len());
        if id == b"fmt " && size >= 16 {
            let mut format = u16_at(body);
            if format == 0xFFFE && size >= 26 {
                // The subformat GUID starts with the format code.
                format = u16_at(body + 24);
            }
            fmt = Some((format, u16_at(body + 2), u32_at(body + 4), u16_at(body + 14)));
        } else if id == b"data" {
            let (format, channels, rate, bits) = fmt.ok_or_else(|| bad("data before fmt"))?;
            if channels != 2 {
                return Err(bad("not stereo"));
            }
            let data = &b[body..end];
            let mut audio = Audio { rate, ..Audio::default() };
            match (format, bits) {
                (3, 32) => {
                    for f in data.chunks_exact(8) {
                        audio.left.push(f32::from_le_bytes([f[0], f[1], f[2], f[3]]));
                        audio.right.push(f32::from_le_bytes([f[4], f[5], f[6], f[7]]));
                    }
                }
                (1, 16) => {
                    for f in data.chunks_exact(4) {
                        audio.left.push(i16::from_le_bytes([f[0], f[1]]) as f32 / 32768.0);
                        audio.right.push(i16::from_le_bytes([f[2], f[3]]) as f32 / 32768.0);
                    }
                }
                _ => return Err(bad(&format!("unsupported format {format}, {bits} bits"))),
            }
            return Ok(audio);
        }
        p = body + size + size % 2;
    }
    Err(bad("no data chunk"))
}
