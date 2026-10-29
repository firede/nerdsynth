//! Loads GeneralUser GS 2.0.3, a real 30 MB SoundFont, and resolves every
//! region of every preset. Skipped when the file is not there
//! (`~/.nerdice/soundfonts/GeneralUser-GS-2.0.3.sf2`, or the path in
//! `NERDSYNTH_GU_SF2`).

use std::alloc::{GlobalAlloc, Layout, System};
use std::fs::File;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use nerdsynth::SoundFont;
use nerdsynth::soundfont::{GeneratorType, ModList, Modulator};

/// Counts the bytes allocated now and at most. This test binary has one
/// test, so nothing else allocates while it measures.
struct Counting;

static NOW: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

fn add(n: usize) {
    let now = NOW.fetch_add(n, Ordering::Relaxed) + n;
    PEAK.fetch_max(now, Ordering::Relaxed);
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            add(layout.size());
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        unsafe { System.dealloc(p, layout) };
        NOW.fetch_sub(layout.size(), Ordering::Relaxed);
    }
    unsafe fn realloc(&self, p: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let q = unsafe { System.realloc(p, layout, new_size) };
        if !q.is_null() {
            // Counted as if the old block lived until the new one was
            // filled, the worst an allocator can do.
            add(new_size);
            NOW.fetch_sub(layout.size(), Ordering::Relaxed);
        }
        q
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("NERDSYNTH_GU_SF2") {
        return Some(p.into());
    }
    let home = std::env::var_os("HOME")?;
    let p = PathBuf::from(home).join(".nerdice/soundfonts/GeneralUser-GS-2.0.3.sf2");
    p.exists().then_some(p)
}

#[test]
fn generaluser_gs() {
    let Some(path) = path() else {
        eprintln!("GeneralUser GS not found; skipped");
        return;
    };
    let file_size = std::fs::metadata(&path).unwrap().len() as usize;

    let base = NOW.load(Ordering::Relaxed);
    PEAK.store(base, Ordering::Relaxed);
    let started = Instant::now();
    let sf = SoundFont::read(File::open(&path).unwrap()).expect("GeneralUser GS loads");
    let elapsed = started.elapsed();
    let peak = PEAK.load(Ordering::Relaxed) - base;
    let kept = NOW.load(Ordering::Relaxed) - base;

    let data_bytes = sf.sample_data().len() * 2 + sf.sample_data24().map_or(0, |d| d.len());
    let zones: usize = sf.instruments().iter().map(|i| i.zone_count()).sum();
    let preset_zones: usize = sf.presets().iter().map(|p| p.zone_count()).sum();
    eprintln!("file {file_size} bytes, sample data {data_bytes} bytes");
    eprintln!(
        "loaded in {elapsed:?}; heap peak {peak} bytes ({:.3}x the sample data), kept {kept}",
        peak as f64 / data_bytes as f64
    );
    eprintln!(
        "{} presets ({preset_zones} zones), {} instruments ({zones} zones), {} samples, version {:?}, name {:?}",
        sf.presets().len(),
        sf.instruments().len(),
        sf.samples().len(),
        sf.info().version(),
        sf.info().name()
    );

    assert!(sf.presets().len() > 200, "{} presets", sf.presets().len());
    assert!(sf.find_preset(0, 0).is_some());
    assert!(sf.find_preset(128, 0).is_some());
    assert!(peak as f64 <= data_bytes as f64 * 1.2 + (4 << 20) as f64, "heap peak {peak} for {data_bytes} of samples");

    // The default modulators are not public; the testkit has them.
    #[cfg(feature = "testkit")]
    let defaults: &[Modulator] = nerdsynth::testkit::sf2::DEFAULT_MODULATORS;
    #[cfg(not(feature = "testkit"))]
    let defaults: &[Modulator] = &[];

    // Every region of every preset, at every key and a few velocities.
    let mut mods = ModList::new();
    let (mut regions, mut with_preset_mods, mut max_mods, mut dropped) = (0usize, 0usize, 0usize, 0usize);
    let mut distinct = std::collections::HashSet::new();
    let started = Instant::now();
    for p in 0..sf.presets().len() {
        for key in 0..=127u8 {
            for velocity in [1u8, 64, 127] {
                for r in sf.regions(p, key, velocity) {
                    assert!((0..61).filter_map(GeneratorType::from_number).all(|g| r.generator(g).abs() <= 1 << 20));
                    let s = r.sample();
                    assert!(s.start() as usize <= sf.sample_data().len() || s.start() > s.end());
                    r.modulators(&mut mods);
                    regions += 1;
                    max_mods = max_mods.max(mods.as_slice().len());
                    dropped += mods.dropped();
                    if mods.as_slice() != defaults {
                        with_preset_mods += 1;
                    }
                    for m in mods.as_slice() {
                        distinct.insert((m.source(), m.destination(), m.amount_source(), m.transform()));
                    }
                }
            }
        }
    }
    eprintln!(
        "{regions} regions resolved in {:?}; {with_preset_mods} with modulators other than the defaults, at most {max_mods} modulators, {dropped} dropped, {} distinct modulators",
        started.elapsed(),
        distinct.len()
    );
    assert_eq!(dropped, 0);
}
