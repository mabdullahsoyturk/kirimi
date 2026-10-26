#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    kirimi_core::fuzzing::check_search(data);
});
