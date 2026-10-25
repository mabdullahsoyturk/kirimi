#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    fast_multipart_core::fuzzing::check_parse(data);
});
