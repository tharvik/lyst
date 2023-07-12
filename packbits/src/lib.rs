mod decode;
mod encode;

pub use decode::Decoder;
pub use encode::Encoder;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("unfinished literal")]
    DanglingLiteral,
    #[error("unfinished repeated")]
    DanglingRepeated,
}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use std::iter;

    use bytes::Buf;

    use crate::{Decoder, Encoder};

    const INPUT_SIZE: usize = 512;

    // outside buffer to avoid benchmarking allocations
    fn roundtrip(data: impl AsRef<[u8]>) {
        let data = data.as_ref();

        let mut encoder = Encoder::new();
        let encoded = encoder.encode(data).chain(encoder.finalize());

        let mut decoder = Decoder::new();
        let mut decoded = decoder.decode(encoded);
        decoder.finalize().unwrap();

        assert_eq!(data, decoded.copy_to_bytes(decoded.remaining()))
    }

    #[test]
    fn roundtrip_single_byte() {
        roundtrip([0xAB]) // random
    }

    #[test]
    fn roundtrip_repeated_byte() {
        roundtrip(iter::repeat(0xAB).take(INPUT_SIZE).collect::<Vec<_>>())
    }

    #[test]
    fn roundtrip_incrementing_counter() {
        roundtrip(
            (0..INPUT_SIZE)
                .map(|b| (b & 0xFF) as u8)
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn roundtrip_mixed_load() {
        const REPEAT_SIZE: usize = 5; // size of chunk of repeated byte
        const RANDOM_SIZE: u8 = 5; // size of unrepeated bytes

        roundtrip(
            iter::successors(Some(true), |prev| Some(!prev))
                .flat_map(|repeat| {
                    if repeat {
                        [0u8; REPEAT_SIZE].to_vec()
                    } else {
                        (0u8..RANDOM_SIZE).collect::<Vec<u8>>()
                    }
                })
                .take(INPUT_SIZE)
                .collect::<Vec<_>>(),
        )
    }
}
