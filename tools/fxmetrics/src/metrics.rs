//! The metrics, measured on one synthesizer's renders of the tests. Each
//! is a named number; [`crate::spec`] says what kind it is and how it is
//! judged.

use crate::dsp::{self, db};
use crate::render::{self, Pair};

/// Octave bands, by centre frequency.
pub const BANDS: [f64; 7] = [125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0];

/// The bands whose upper edge is below 0.45 times the rate.
pub fn bands(rate: f64) -> Vec<f64> {
    BANDS.iter().copied().filter(|f| f * std::f64::consts::SQRT_2 * 2f64.powf(1.0 / 6.0) < 0.45 * rate).collect()
}

pub type Values = Vec<(String, f64)>;

fn ms(frames: usize, rate: f64) -> usize {
    (frames as f64 * rate / 1000.0) as usize
}

/// The index of the largest magnitude in `x[a..b]`.
fn argmax_abs(x: &[f32], a: usize, b: usize) -> usize {
    (a..b.min(x.len())).max_by(|&i, &j| x[i].abs().total_cmp(&x[j].abs())).unwrap_or(a)
}

/// The reverb's metrics, from the renders of [`render::reverb_test`].
pub fn reverb(pair: &Pair) -> Values {
    let rate = f64::from(pair.on.rate);
    let wet = pair.wet();
    let mut v = Values::new();
    // The click, and the response to it normalized by its height.
    let c = argmax_abs(&pair.off.left, 0, ms(1000, rate));
    let a = f64::from(pair.off.left[c]);
    let len = ms((render::TONE_START_MS - render::CLICK_MS - 300) as usize, rate);
    let (mut hl, mut hr) = wet.slice(c, c + len);
    hl.iter_mut().chain(hr.iter_mut()).for_each(|x| *x /= a);
    let energy: Vec<f64> = hl.iter().zip(&hr).map(|(l, r)| l * l + r * r).collect();
    v.push(("rev.level".into(), db(energy.iter().sum::<f64>() / 2.0)));
    let peak = hl.iter().chain(&hr).fold(0.0f64, |m, x| m.max(x.abs()));
    let onset = (0..hl.len()).find(|&i| hl[i].abs().max(hr[i].abs()) >= 0.05 * peak).unwrap_or(0);
    v.push(("rev.predelay".into(), onset as f64 * 1000.0 / rate));
    // The end of the analysis: the last 10 ms window within 90 dB of the
    // loudest (below every floor).
    let win = ms(10, rate).max(1);
    let env = dsp::envelope_db(&energy, win, win);
    let top = env.iter().copied().fold(f64::MIN, f64::max);
    let end = (env.iter().rposition(|&e| e >= top - 90.0).unwrap_or(0) + 1) * win;
    let energy = &energy[..end.min(energy.len())];
    let (hl, hr) = (&hl[..energy.len()], &hr[..energy.len()]);

    // Echo density: 20 ms windows every millisecond, from the onset.
    let dwin = ms(20, rate);
    let mut mix = Vec::new();
    let mut late = Vec::new();
    for h in [hl, hr] {
        let d = dsp::echo_density(&h[onset..], dwin, ms(1, rate).max(1));
        let t = d.iter().find(|p| p.0 >= dwin / 2 && p.1 >= 0.95).map_or(f64::NAN, |p| p.0 as f64 * 1000.0 / rate);
        mix.push(t);
        let l: Vec<f64> = d.iter().filter(|p| p.0 >= ms(150, rate) && p.0 < ms(500, rate)).map(|p| p.1).collect();
        late.push(dsp::mean_sd(&l).0);
    }
    v.push(("rev.mixing_time".into(), (mix[0] + mix[1]) / 2.0));
    v.push(("rev.ned_late".into(), (late[0] + late[1]) / 2.0));

    // Decay per band: Schroeder's curve fitted from -5 to -35 dB.
    let full = dsp::edc_db(energy);
    let t_of = |i: usize| i as f64 / rate;
    let step = ms(1, rate).max(1);
    let mut devs = Vec::new();
    let mut flucts = Vec::new();
    for f in bands(rate) {
        let bl = dsp::band(hl, rate, f / std::f64::consts::SQRT_2, f * std::f64::consts::SQRT_2);
        let br = dsp::band(hr, rate, f / std::f64::consts::SQRT_2, f * std::f64::consts::SQRT_2);
        let e: Vec<f64> = bl.iter().zip(&br).map(|(l, r)| l * l + r * r).collect();
        let edc = dsp::edc_db(&e);
        let (Some(a), Some(b)) = (dsp::first_below(&edc, -5.0), dsp::first_below(&edc, -35.0)) else { continue };
        let x: Vec<f64> = (a..b).step_by(step).map(t_of).collect();
        let y: Vec<f64> = (a..b).step_by(step).map(|i| edc[i]).collect();
        let (slope, _, dev) = dsp::fit(&x, &y);
        v.push((format!("rev.t30.{f}"), -60.0 / slope));
        v.push((format!("rev.edc_dev.{f}"), dev));
        devs.push(dev);
        // Energy in 20 ms windows over the same stretch, about its line.
        let w = ms(20, rate);
        let hop = ms(5, rate);
        let env = dsp::envelope_db(&e[a..b], w, hop);
        let x: Vec<f64> = (0..env.len()).map(|i| i as f64).collect();
        let fl = dsp::fit(&x, &env).2;
        v.push((format!("rev.fluct.{f}"), fl));
        flucts.push(fl);
        let z = dsp::first_below(&edc, -10.0).unwrap_or(1).max(1);
        let x: Vec<f64> = (0..z).step_by(step).map(t_of).collect();
        let y: Vec<f64> = (0..z).step_by(step).map(|i| edc[i]).collect();
        v.push((format!("rev.edt.{f}"), -60.0 / dsp::fit(&x, &y).0));
    }
    if std::env::var_os("FXMETRICS_DEBUG").is_some() {
        let mut t = Vec::new();
        for (lo, hi) in [(5600.0, 8000.0), (8000.0, 11300.0), (11300.0, 16000.0), (16000.0, 20000.0)] {
            if hi > 0.49 * rate {
                continue;
            }
            let bl = dsp::band(hl, rate, lo, hi);
            let br = dsp::band(hr, rate, lo, hi);
            let e: Vec<f64> = bl.iter().zip(&br).map(|(l, r)| l * l + r * r).collect();
            let edc = dsp::edc_db(&e);
            let (Some(a), Some(b)) = (dsp::first_below(&edc, -5.0), dsp::first_below(&edc, -25.0)) else { continue };
            t.push((
                lo,
                ((b - a) as f64 / rate * 3.0 * 100.0).round() / 100.0,
                (db(e.iter().take(ms(300, rate)).sum::<f64>()) * 10.0).round() / 10.0,
            ));
        }
        eprintln!("{rate} hf T20, early level: {t:?}");
    }
    v.push(("rev.edc_dev".into(), dsp::mean_sd(&devs).0));
    v.push(("rev.fluct".into(), dsp::mean_sd(&flucts).0));

    // The late tail, from 100 ms after the onset to -40 dB or a second on.
    let t40 = dsp::first_below(&full, -40.0).unwrap_or(full.len());
    let ta = onset + ms(100, rate);
    let tb = t40.min(onset + ms(1100, rate)).max(ta + 1);
    // Broadband decay, to make the tail stationary before its spectrum.
    let (a5, a35) = (dsp::first_below(&full, -5.0).unwrap_or(0), dsp::first_below(&full, -35.0).unwrap_or(1));
    let x: Vec<f64> = (a5..a35).step_by(step).map(t_of).collect();
    let y: Vec<f64> = (a5..a35).step_by(step).map(|i| full[i]).collect();
    let slope = dsp::fit(&x, &y).0;
    v.push(("rev.t30".into(), -60.0 / slope));
    let n = (0.1 * rate).round() as usize;
    let n = n.next_power_of_two();
    let mut rough = Vec::new();
    let mut centroid = Vec::new();
    for h in [hl, hr] {
        let seg: Vec<f64> =
            (ta..tb.min(h.len())).map(|i| h[i] * 10f64.powf(-slope * (i - ta) as f64 / rate / 20.0)).collect();
        let p = dsp::welch(&seg, n);
        rough.push(roughness(&p, rate, n));
        if std::env::var_os("FXMETRICS_DEBUG").is_some() {
            let mut bands = Vec::new();
            for f in [125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0] {
                let e: f64 = p
                    .iter()
                    .enumerate()
                    .filter(|(k, _)| {
                        let x = *k as f64 * rate / n as f64;
                        x >= f / std::f64::consts::SQRT_2 && x < f * std::f64::consts::SQRT_2
                    })
                    .map(|(_, v)| v)
                    .sum();
                bands.push((db(e) * 10.0).round() / 10.0);
            }
            eprintln!("{rate} tail octaves 125..16k dB: {bands:?}");
        }
        let (mut num, mut den) = (0.0, 0.0);
        for (k, &pk) in p.iter().enumerate() {
            let f = k as f64 * rate / n as f64;
            if (50.0..0.45 * rate).contains(&f) {
                num += f * pk;
                den += pk;
            }
        }
        centroid.push(num / den.max(1e-300));
    }
    v.push(("rev.roughness".into(), (rough[0] + rough[1]) / 2.0));
    v.push(("rev.centroid".into(), (centroid[0] + centroid[1]) / 2.0));
    let late = onset + ms(80, rate);
    let (iacc, rho0) = dsp::cross_correlation(&hl[late..t40.max(late + 1)], &hr[late..t40.max(late + 1)], ms(1, rate));
    v.push(("rev.iacc".into(), iacc));
    v.push(("rev.rho0".into(), rho0));
    let el: f64 = hl.iter().map(|x| x * x).sum();
    let er: f64 = hr.iter().map(|x| x * x).sum();
    v.push(("rev.balance".into(), db(el / er)));

    // The tones' tails: from 150 ms after each note off, a second long.
    let mut tails = Vec::new();
    let mut beats = Vec::new();
    let mut widths = Vec::new();
    for i in 0..render::TONES {
        let off = render::TONE_START_MS + i as u32 * render::TONE_STEP_MS + render::TONE_ON_MS;
        let a = ms(off as usize + 150, rate);
        let (l, r) = wet.slice(a, a + ms(1000, rate));
        // How far the tail spreads about the tone's frequency: the swing of
        // its delays (and its modes' bandwidths), in cents.
        let f0 = f64::from(render::tone_hz(i));
        if f0 >= 600.0 {
            let half = 0.015 * f0 + 5.0;
            let mut m = Vec::new();
            for x in [&l, &r] {
                let (p, n) = dsp::spectrum(x);
                let bin = rate / n as f64;
                let (mut inside, mut m2) = (0.0, 0.0);
                for (k, &pk) in p.iter().enumerate() {
                    let f = k as f64 * bin;
                    if (f - f0).abs() <= half {
                        inside += pk;
                        m2 += pk * (f - f0).powi(2);
                    }
                }
                m.push((m2 / inside.max(1e-300)).sqrt());
            }
            widths.push(1200.0 * (1.0 + (m[0] + m[1]) / 2.0 / f0).log2());
        }
        let e: Vec<f64> = l.iter().zip(&r).map(|(l, r)| l * l + r * r).collect();
        tails.push(e.iter().sum::<f64>());
        let env = dsp::envelope_db(&e, ms(10, rate), ms(10, rate));
        let x: Vec<f64> = (0..env.len()).map(|i| i as f64).collect();
        beats.push(dsp::fit(&x, &env).2);
    }
    // Levels about their trend over log frequency (the tilt is the
    // tail's colour, measured by the centroid; this is its unevenness).
    let levels: Vec<f64> = tails.iter().map(|&e| db(e)).collect();
    let octave: Vec<f64> = (0..render::TONES).map(|i| f64::from(render::tone_hz(i)).log2()).collect();
    let (slope, icpt, _) = dsp::fit(&octave, &levels);
    let mut dev: Vec<f64> = levels.iter().zip(&octave).map(|(l, o)| l - (icpt + slope * o)).collect();
    if std::env::var_os("FXMETRICS_DEBUG").is_some() {
        eprintln!("{rate} tone tails dB: {:?}", dev.iter().map(|l| (l * 10.0).round() / 10.0).collect::<Vec<_>>());
    }
    v.push(("rev.tone_spread".into(), dsp::mean_sd(&dev).1));
    dev.sort_by(f64::total_cmp);
    let at = |q: f64| dev[((q * (dev.len() - 1) as f64).round() as usize).min(dev.len() - 1)];
    v.push(("rev.tone_peak".into(), at(0.95) - at(0.5)));
    v.push(("rev.tail_width".into(), dsp::mean_sd(&widths).0));
    v.push(("rev.tone_beat".into(), dsp::mean_sd(&beats).0));

    v.push(("rev.cut".into(), cut_level(&wet)));
    v
}

/// The spread, in dB, of a power spectrum about its third-octave
/// smoothing, from 200 Hz to 8 kHz (or 0.4 times the rate).
fn roughness(p: &[f64], rate: f64, n: usize) -> f64 {
    let bin = rate / n as f64;
    let hi = 8000f64.min(0.4 * rate);
    let mut dev = Vec::new();
    for (k, &pk) in p.iter().enumerate() {
        let f = k as f64 * bin;
        if f < 200.0 || f > hi {
            continue;
        }
        let a = ((f * 2f64.powf(-1.0 / 6.0)) / bin).floor() as usize;
        let b = (((f * 2f64.powf(1.0 / 6.0)) / bin).ceil() as usize).min(p.len() - 1);
        let smooth = p[a..=b].iter().sum::<f64>() / (b - a + 1) as f64;
        dev.push(db(pk / smooth));
    }
    dsp::mean_sd(&dev).1
}

/// Where the wet signal stops dead (a run of exact zeros of 100 ms or more
/// after sound), the loudest sample of the 10 ms before, in dBFS; the
/// loudest of all such stops. Without any, the level of the last 10 ms.
fn cut_level(wet: &crate::wav::Audio) -> f64 {
    let rate = f64::from(wet.rate);
    let run = ms(100, rate);
    let w = ms(10, rate);
    let mag: Vec<f32> = wet.left.iter().zip(&wet.right).map(|(l, r)| l.abs().max(r.abs())).collect();
    let mut worst = f64::NEG_INFINITY;
    let mut zeros = 0;
    let mut any = false;
    for i in 0..mag.len() {
        if mag[i] == 0.0 {
            zeros += 1;
            if zeros == run {
                let end = i + 1 - run;
                if end > 0 && mag[..end].iter().any(|&m| m != 0.0) {
                    let peak = mag[end.saturating_sub(w)..end].iter().copied().fold(0.0f32, f32::max);
                    worst = worst.max(20.0 * f64::from(peak).max(1e-30).log10());
                    any = true;
                }
            }
        } else {
            zeros = 0;
        }
    }
    if !any {
        let peak = mag[mag.len().saturating_sub(w)..].iter().copied().fold(0.0f32, f32::max);
        worst = 20.0 * f64::from(peak).max(1e-30).log10();
    }
    worst
}

/// The chorus's metrics, from the renders of [`render::chorus_test`].
pub fn chorus(pair: &Pair) -> Values {
    let rate = f64::from(pair.on.rate);
    let wet = pair.wet();
    let mut v = Values::new();

    // The click train: the taps after each click, past the attack.
    let period = render::train_period(pair.on.rate);
    let c1 = argmax_abs(&pair.off.left, ms(1000, rate), ms(1000, rate) + period);
    let end = ms(render::TRAIN_MS.1 as usize - 500, rate);
    let mut taps: Vec<(f64, f64, f64, f64)> = Vec::new(); // time s, delay ms, left, right
    let mut c = c1;
    while c + period < end {
        let a = f64::from(pair.off.left[c]);
        let (l, r) = wet.slice(c, c + period);
        let e: Vec<f64> = l.iter().zip(&r).map(|(l, r)| l * l + r * r).collect();
        let top = e.iter().copied().fold(0.0, f64::max);
        let mut peaks: Vec<usize> =
            (1..e.len() - 1).filter(|&j| e[j] >= e[j - 1] && e[j] > e[j + 1] && e[j] > 0.05 * top).collect();
        peaks.sort_by(|&x, &y| e[y].total_cmp(&e[x]));
        peaks.truncate(6);
        for j in peaks {
            let (y0, y1, y2) = (e[j - 1], e[j], e[j + 1]);
            let den = y0 - 2.0 * y1 + y2;
            let frac = if den != 0.0 { 0.5 * (y0 - y2) / den } else { 0.0 };
            taps.push((c as f64 / rate, (j as f64 + frac) * 1000.0 / rate, l[j] / a, r[j] / a));
        }
        c += period;
    }
    let mut delays: Vec<f64> = taps.iter().map(|t| t.1).collect();
    delays.sort_by(f64::total_cmp);
    let pct = |p: f64| delays.get(((delays.len() as f64 - 1.0) * p).round() as usize).copied().unwrap_or(f64::NAN);
    v.push(("cho.delay_min".into(), pct(0.01)));
    v.push(("cho.depth".into(), pct(0.99) - pct(0.01)));
    // A voice: the taps of one pan and polarity, the strongest a click.
    let key = |t: &(f64, f64, f64, f64)| {
        let pan = (t.2.abs() - t.3.abs()) / (t.2.abs() + t.3.abs()).max(1e-30);
        (t.2 >= 0.0, t.3 >= 0.0, (pan * 4.0).round() as i32)
    };
    let mut keys: Vec<(bool, bool, i32)> = taps.iter().map(key).collect();
    keys.sort();
    keys.dedup();
    let voice = keys.iter().max_by_key(|k| taps.iter().filter(|t| key(t) == **k).count()).copied();
    let mut track: Vec<(f64, f64, f64)> = Vec::new();
    for t in taps.iter().filter(|t| Some(key(t)) == voice) {
        let s = t.2 * t.2 + t.3 * t.3;
        match track.last_mut() {
            Some(last) if last.0 == t.0 => {
                if s > last.2 {
                    *last = (t.0, t.1, s);
                }
            }
            _ => track.push((t.0, t.1, s)),
        }
    }
    let mean = track.iter().map(|t| t.1).sum::<f64>() / track.len().max(1) as f64;
    let mut best = (0.0, f64::NAN);
    let mut f = 0.02;
    while f < 3.0 {
        let (mut re, mut im) = (0.0, 0.0);
        for t in &track {
            let (s, c) = (std::f64::consts::TAU * f * t.0).sin_cos();
            re += (t.1 - mean) * c;
            im += (t.1 - mean) * s;
        }
        if re * re + im * im > best.0 {
            best = (re * re + im * im, f);
        }
        f += 0.001;
    }
    v.push(("cho.rate".into(), best.1));
    v.push((
        "cho.voices_seen".into(),
        keys.iter().filter(|k| taps.iter().filter(|t| key(t) == **k).count() * 4 > track.len()).count() as f64,
    ));

    // The sines: what stays near the sine, what falls elsewhere.
    for (i, &hz) in render::CHORUS_SINES.iter().enumerate() {
        let on = render::SINE_START_MS + i as u32 * render::SINE_STEP_MS;
        let (l, r) = wet.slice(ms(on as usize + 1000, rate), ms((on + render::SINE_MS) as usize - 1000, rate));
        let f0 = f64::from(hz);
        let half = 0.015 * f0 + 5.0;
        let mut snr = Vec::new();
        let mut spread = Vec::new();
        for x in [&l, &r] {
            let (p, n) = dsp::spectrum(x);
            let bin = rate / n as f64;
            let (mut inside, mut outside, mut m2) = (0.0, 0.0, 0.0);
            for (k, &pk) in p.iter().enumerate() {
                let f = k as f64 * bin;
                if (f - f0).abs() <= half {
                    inside += pk;
                    m2 += pk * (f - f0).powi(2);
                } else if f >= 20.0 && f <= 0.45 * rate {
                    outside += pk;
                }
            }
            snr.push(db(inside / outside.max(1e-300)));
            spread.push((m2 / inside.max(1e-300)).sqrt());
        }
        v.push((format!("cho.snr.{hz}"), snr[0].min(snr[1])));
        let sigma = (spread[0] + spread[1]) / 2.0;
        v.push((format!("cho.pitch_dev.{hz}"), 1200.0 * (1.0 + std::f64::consts::SQRT_2 * sigma / f0).log2()));
    }

    // Noise: the stereo image and the level.
    let (a, b) = (ms(render::NOISE_MS.0 as usize + 1000, rate), ms(render::NOISE_MS.1 as usize - 1000, rate));
    let (l, r) = wet.slice(a, b);
    let (dl, dr) = pair.off.slice(a, b);
    let (iacc, rho0) = dsp::cross_correlation(&l, &r, ms(1, rate));
    v.push(("cho.iacc".into(), iacc));
    v.push(("cho.rho0".into(), rho0));
    let e = |x: &[f64]| x.iter().map(|v| v * v).sum::<f64>();
    v.push(("cho.balance".into(), db(e(&l) / e(&r))));
    v.push(("cho.level".into(), db((e(&l) + e(&r)) / (e(&dl) + e(&dr)))));
    let mono: Vec<f64> = l.iter().zip(&r).map(|(a, b)| (a + b) / 2.0).collect();
    v.push(("cho.mono".into(), db(2.0 * e(&mono) / (e(&l) + e(&r)))));
    // Only where the input ends cleanly: the click train. (A voice that
    // stops dead stops the chorus's input dead too.)
    let mut train = wet.clone();
    train.fit(ms(render::SINE_START_MS as usize, rate));
    v.push(("cho.cut".into(), cut_level(&train)));
    v
}
