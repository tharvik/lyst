#![no_main]

use libfuzzer_sys::fuzz_target;
use packbits::Decoder;

fuzz_target!(|data: &[u8]| {
    let mut decoder = Decoder::new();
    decoder.decode(data);
    let _ = decoder.finalize(); // likely invalid data
});
