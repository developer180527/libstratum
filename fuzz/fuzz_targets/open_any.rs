//! Arbitrary bytes through every registered container format: probe, open, section data, info,
//! symbols, summaries.
//! Errors are expected; panics, hangs and out-of-bounds reads are bugs.
#![no_main]

use libfuzzer_sys::fuzz_target;
use libstratum::model::Dimension;
use libstratum::{Input, OpenOptions};

fuzz_target!(|data: &[u8]| {
    let engine = libstratum::default_engine();
    if let Ok(session) = engine.open(Input::Bytes(data.to_vec()), &OpenOptions::default()) {
        let image = session.image();
        for section in image.sections() {
            let _ = image.section_data(section.id);
        }
        let _ = image.symbols().len();
        let _ = image.debug_locations();
        let _ = session.info();
        let _ = session.symbols(&Default::default());
        let _ = session.summary(&[Dimension::Section, Dimension::Symbol]);
        let _ = session.berkeley_sizes();
    }
});
