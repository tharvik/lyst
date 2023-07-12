#![no_main]

use libfuzzer_sys::fuzz_target;

use packbits::Encoder;

fuzz_target!(|data: &[u8]| {
    let mut encoder = Encoder::new();
    encoder.encode(data);
    encoder.finalize();
});
