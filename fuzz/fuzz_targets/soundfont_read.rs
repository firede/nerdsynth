//! Any input either fails to load with an error, or loads into a SoundFont
//! whose regions all resolve, without panicking.
#![no_main]

use libfuzzer_sys::fuzz_target;
use nerdsynth::SoundFont;
use nerdsynth::soundfont::{GeneratorType, ModList};

fuzz_target!(|data: &[u8]| {
    let Ok(sf) = SoundFont::from_bytes(data) else { return };
    let mut mods = ModList::new();
    for preset in 0..sf.presets().len().min(16) {
        for key in [0, 60, 127] {
            for velocity in [1, 100, 127] {
                for region in sf.regions(preset, key, velocity) {
                    for g in (0..61).filter_map(GeneratorType::from_number) {
                        let _ = region.generator(g);
                    }
                    let _ = region.sample();
                    region.modulators(&mut mods);
                }
            }
        }
    }
});
