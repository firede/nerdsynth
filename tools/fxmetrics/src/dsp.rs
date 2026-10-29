//! Signal analysis: FFT, band filters, decay curves, fits, echo density.

use std::f64::consts::{PI, TAU};

/// In-place radix-2 FFT; `re.len()` must be a power of two.
pub fn fft(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    assert!(n.is_power_of_two() && im.len() == n);
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let (ws, wc) = (-TAU / len as f64).sin_cos();
        for start in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0, 0.0);
            for k in 0..len / 2 {
                let (a, b) = (start + k, start + k + len / 2);
                let tr = re[b] * cr - im[b] * ci;
                let ti = re[b] * ci + im[b] * cr;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
                let nc = cr * wc - ci * ws;
                ci = cr * ws + ci * wc;
                cr = nc;
            }
        }
        len <<= 1;
    }
}

/// The inverse of [`fft`], scaled.
pub fn ifft(re: &mut [f64], im: &mut [f64]) {
    im.iter_mut().for_each(|x| *x = -*x);
    fft(re, im);
    let n = re.len() as f64;
    re.iter_mut().for_each(|x| *x /= n);
    im.iter_mut().for_each(|x| *x = -*x / n);
}

pub fn hann(n: usize) -> Vec<f64> {
    (0..n).map(|i| 0.5 - 0.5 * (TAU * i as f64 / n as f64).cos()).collect()
}

/// A zero-phase band-pass from `lo` to `hi` Hz, done in the frequency
/// domain, with raised-cosine edges a third of an octave wide.
pub fn band(x: &[f64], rate: f64, lo: f64, hi: f64) -> Vec<f64> {
    let n = (x.len() * 2).next_power_of_two();
    let mut re = vec![0.0; n];
    let mut im = vec![0.0; n];
    re[..x.len()].copy_from_slice(x);
    fft(&mut re, &mut im);
    let edge = |f: f64, at: f64| {
        // 0 below at/2^(1/6), 1 above at·2^(1/6).
        let o = (f / at).log2() * 3.0; // in thirds of an octave
        if o <= -0.5 {
            0.0
        } else if o >= 0.5 {
            1.0
        } else {
            0.5 - 0.5 * (PI * (o + 0.5)).cos()
        }
    };
    for k in 0..=n / 2 {
        let f = k as f64 * rate / n as f64;
        let g = if f <= 0.0 { 0.0 } else { edge(f, lo) * (1.0 - edge(f, hi)) };
        re[k] *= g;
        im[k] *= g;
        if k > 0 && k < n / 2 {
            re[n - k] *= g;
            im[n - k] *= g;
        }
    }
    ifft(&mut re, &mut im);
    re.truncate(x.len());
    re
}

pub fn db(power: f64) -> f64 {
    10.0 * power.max(1e-300).log10()
}

/// The least-squares line through `(x, y)`: slope, intercept, and the RMS
/// of the residuals.
pub fn fit(x: &[f64], y: &[f64]) -> (f64, f64, f64) {
    let n = x.len() as f64;
    if x.len() < 2 {
        return (0.0, y.first().copied().unwrap_or(0.0), 0.0);
    }
    let mx = x.iter().sum::<f64>() / n;
    let my = y.iter().sum::<f64>() / n;
    let sxy: f64 = x.iter().zip(y).map(|(a, b)| (a - mx) * (b - my)).sum();
    let sxx: f64 = x.iter().map(|a| (a - mx).powi(2)).sum();
    let slope = if sxx > 0.0 { sxy / sxx } else { 0.0 };
    let icpt = my - slope * mx;
    let rms = (x.iter().zip(y).map(|(a, b)| (b - (icpt + slope * a)).powi(2)).sum::<f64>() / n).sqrt();
    (slope, icpt, rms)
}

/// Schroeder's energy decay curve of an energy signal, in dB from its start.
pub fn edc_db(energy: &[f64]) -> Vec<f64> {
    let mut acc = 0.0;
    let mut out = vec![0.0; energy.len()];
    for i in (0..energy.len()).rev() {
        acc += energy[i];
        out[i] = acc;
    }
    let total = out.first().copied().unwrap_or(0.0).max(1e-300);
    out.iter().map(|e| db(e / total)).collect()
}

/// The first index from which `curve` is at or below `level`.
pub fn first_below(curve: &[f64], level: f64) -> Option<usize> {
    curve.iter().position(|&v| v <= level)
}

/// Energy in windows of `win` frames, every `hop` frames, as dB.
pub fn envelope_db(energy: &[f64], win: usize, hop: usize) -> Vec<f64> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + win <= energy.len() {
        out.push(db(energy[i..i + win].iter().sum::<f64>() / win as f64));
        i += hop;
    }
    out
}

/// Abel and Huang's normalized echo density profile: in a Hann window of
/// `win` frames around each frame (every `hop`), the weighted fraction of
/// frames beyond one standard deviation, over that of a Gaussian
/// (erfc(1/√2)). Gives (centre frame, density) pairs.
pub fn echo_density(h: &[f64], win: usize, hop: usize) -> Vec<(usize, f64)> {
    const GAUSS: f64 = 0.317_310_507_862_914;
    let w = hann(win);
    let wsum: f64 = w.iter().sum();
    let mut out = Vec::new();
    let mut i = 0;
    while i + win <= h.len() {
        let seg = &h[i..i + win];
        let var: f64 = seg.iter().zip(&w).map(|(x, w)| w * x * x).sum::<f64>() / wsum;
        let sd = var.sqrt();
        let beyond: f64 = seg.iter().zip(&w).filter(|(x, _)| x.abs() > sd).map(|(_, w)| w).sum::<f64>() / wsum;
        out.push((i + win / 2, if sd > 0.0 { beyond / GAUSS } else { 0.0 }));
        i += hop;
    }
    out
}

/// Welch's power spectrum: Hann frames of `n` (a power of two), half
/// overlapping, averaged. Bin k is at k·rate/n.
pub fn welch(x: &[f64], n: usize) -> Vec<f64> {
    let w = hann(n);
    let mut acc = vec![0.0; n / 2 + 1];
    let mut frames = 0;
    let mut i = 0;
    while i + n <= x.len() {
        let mut re: Vec<f64> = x[i..i + n].iter().zip(&w).map(|(a, b)| a * b).collect();
        let mut im = vec![0.0; n];
        fft(&mut re, &mut im);
        for k in 0..=n / 2 {
            acc[k] += re[k] * re[k] + im[k] * im[k];
        }
        frames += 1;
        i += n / 2;
    }
    acc.iter_mut().for_each(|v| *v /= frames.max(1) as f64);
    acc
}

/// The power spectrum of all of `x` under one Hann window, zero-padded to
/// a power of two.
pub fn spectrum(x: &[f64]) -> (Vec<f64>, usize) {
    let n = x.len().next_power_of_two();
    let w = hann(x.len());
    let mut re = vec![0.0; n];
    for (i, v) in x.iter().enumerate() {
        re[i] = v * w[i];
    }
    let mut im = vec![0.0; n];
    fft(&mut re, &mut im);
    ((0..=n / 2).map(|k| re[k] * re[k] + im[k] * im[k]).collect(), n)
}

/// The normalized cross-correlation of `a` and `b` at lags -max..=max:
/// (the largest magnitude, the value at lag 0).
pub fn cross_correlation(a: &[f64], b: &[f64], max: usize) -> (f64, f64) {
    let ea: f64 = a.iter().map(|x| x * x).sum();
    let eb: f64 = b.iter().map(|x| x * x).sum();
    let norm = (ea * eb).sqrt().max(1e-300);
    let n = a.len().min(b.len());
    let mut best: f64 = 0.0;
    let mut zero = 0.0;
    for lag in -(max as isize)..=max as isize {
        // a[i]·b[i + lag] over the overlap.
        let (a, b) =
            if lag >= 0 { (&a[..n], &b[(lag as usize).min(n)..n]) } else { (&a[((-lag) as usize).min(n)..n], &b[..n]) };
        let s: f64 = a.iter().zip(b).map(|(x, y)| x * y).sum();
        let r = s / norm;
        if lag == 0 {
            zero = r;
        }
        if r.abs() > best.abs() {
            best = r;
        }
    }
    (best.abs(), zero)
}

/// Mean and standard deviation.
pub fn mean_sd(x: &[f64]) -> (f64, f64) {
    let n = x.len().max(1) as f64;
    let m = x.iter().sum::<f64>() / n;
    (m, (x.iter().map(|v| (v - m).powi(2)).sum::<f64>() / n).sqrt())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fft_round_trip_and_sine() {
        let n = 64;
        let x: Vec<f64> = (0..n).map(|i| (TAU * 5.0 * i as f64 / n as f64).sin()).collect();
        let (mut re, mut im) = (x.clone(), vec![0.0; n]);
        fft(&mut re, &mut im);
        assert!((im[5] + 32.0).abs() < 1e-9 && re[3].abs() < 1e-9);
        ifft(&mut re, &mut im);
        assert!(re.iter().zip(&x).all(|(a, b)| (a - b).abs() < 1e-12));
    }

    #[test]
    fn noise_has_unit_echo_density() {
        let x = crate::render::noise(20000, 3);
        let d = echo_density(&x, 882, 441);
        let (m, _) = mean_sd(&d.iter().map(|p| p.1).collect::<Vec<_>>());
        assert!((m - 1.0).abs() < 0.05, "{m}");
        let mut sparse = vec![0.0; 20000];
        for i in (0..20000).step_by(300) {
            sparse[i] = 1.0;
        }
        assert!(echo_density(&sparse, 882, 441).iter().all(|p| p.1 < 0.3));
    }

    #[test]
    fn fit_and_edc() {
        let e: Vec<f64> = (0..1000).map(|i| (-(i as f64) / 100.0).exp()).collect();
        let c = edc_db(&e);
        let x: Vec<f64> = (100..500).map(|i| i as f64).collect();
        let y: Vec<f64> = (100..500).map(|i| c[i]).collect();
        let (slope, _, rms) = fit(&x, &y);
        assert!((slope + 10.0 / (100.0 * std::f64::consts::LN_10)).abs() < 1e-3, "{slope}");
        assert!(rms < 0.05);
    }
}
