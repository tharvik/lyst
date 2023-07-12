use std::fmt;

use bytes::{Buf, BufMut, BytesMut};

const MIN_REPEATED: usize = 3;
const MAX_REPEATED: usize = 128;
const MAX_LITERAL: usize = 128;

#[derive(Debug)]
enum State {
    Idle,
    Literal(BytesMut),
    Repeat { byte: u8, count: usize },
}

impl State {
    fn new_literal_buf() -> BytesMut {
        BytesMut::with_capacity(MAX_LITERAL)
    }
}

/// Encoder for the PackBits algorithm
pub struct Encoder(State);

fn output_literal(output: &mut impl BufMut, buf: &mut (impl Buf + fmt::Debug)) {
    assert!(buf.remaining() <= MAX_LITERAL);

    output.put_i8((buf.remaining() - 1) as i8);
    while buf.has_remaining() {
        let chunk = buf.chunk();
        let chunk_length = chunk.len();
        output.put(chunk);
        buf.advance(chunk_length)
    }
}

fn output_repeating(output: &mut impl BufMut, value: u8, count: usize) {
    assert!(count <= MAX_REPEATED);

    output.put_i8(-((count - 1) as i8));
    output.put_u8(value);
}

fn ends_with_same_byte(slice: &[u8]) -> usize {
    if slice.is_empty() {
        return 0;
    }

    let last_byte = *slice.last().unwrap();
    slice.iter().rev().take_while(|b| **b == last_byte).count()
}

impl Encoder {
    /// New Encoder writing result to given [`BufMut`]
    pub fn new() -> Self {
        Self(State::Idle)
    }

    /// Encode a chunk of data
    pub fn encode(&mut self, input: impl Buf) -> impl Buf {
        let mut ret = BytesMut::with_capacity(input.remaining());
        self.encode_on(input, &mut ret);
        ret
    }

    /// Encode a chunk of data and write result in given [`BufMut`]
    ///
    /// Panic if output hasn't enough space.
    pub fn encode_on(&mut self, mut input: impl Buf, output: &mut impl BufMut) {
        while input.has_remaining() {
            let byte = input.get_u8();

            match &mut self.0 {
                State::Idle => {
                    let mut buf = State::new_literal_buf();
                    buf.put_u8(byte);
                    self.0 = State::Literal(buf)
                }
                State::Literal(ref mut buf) if buf.len() == MAX_LITERAL => {
                    output_literal(output, buf);
                    buf.clear();
                    buf.put_u8(byte);
                }
                State::Literal(ref mut buf) => {
                    buf.put_u8(byte);

                    let repeated_count = ends_with_same_byte(buf);
                    if repeated_count == MIN_REPEATED {
                        let mut literal = buf.split_to(buf.len() - repeated_count);
                        if !literal.is_empty() {
                            output_literal(output, &mut literal);
                        }

                        self.0 = State::Repeat {
                            byte,
                            count: repeated_count,
                        };
                    }
                }
                State::Repeat { byte: value, count } if *count == MAX_REPEATED => {
                    output_repeating(output, *value, *count);

                    let mut buf = State::new_literal_buf();
                    buf.put_u8(byte);
                    self.0 = State::Literal(buf)
                }
                State::Repeat { byte: value, count } => {
                    if *value == byte {
                        *count += 1
                    } else {
                        output_repeating(output, *value, *count);
                        let mut buf = State::new_literal_buf();
                        buf.put_u8(byte);

                        self.0 = State::Literal(buf)
                    }
                }
            }
        }
    }

    /// Finish encoding and return last buffer
    pub fn finalize(&mut self) -> impl Buf {
        let mut ret = BytesMut::with_capacity(2); // at least for repeated
        self.finalize_on(&mut ret);
        ret
    }

    pub fn finalize_on(&mut self, output: &mut impl BufMut) {
        match &mut self.0 {
            State::Idle => {}
            State::Literal(ref mut buf) => output_literal(output, buf),
            State::Repeat { byte: value, count } => output_repeating(output, *value, *count),
        }
    }
}

impl Default for Encoder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::iter;

    use super::*;

    fn test_encode_to(to_encode: &[u8], expected: &[u8]) {
        let mut encoder = Encoder::new();
        let mut encoded = encoder.encode(to_encode).chain(encoder.finalize());

        assert_eq!(encoded.copy_to_bytes(encoded.remaining()), expected);
    }

    #[test]
    fn single_byte() {
        test_encode_to(b"a", b"\x00a")
    }

    #[test]
    fn literal() {
        test_encode_to(b"abcdefg", b"\x06abcdefg")
    }

    #[test]
    fn repeated() {
        test_encode_to(b"aaaaaaaaaaaaaaaaa", b"\xF0a")
    }

    #[test]
    fn long_repeated() {
        test_encode_to(
            iter::repeat(b'a')
                .take(128 + 16)
                .collect::<Vec<_>>()
                .as_slice(),
            b"\x81a\xF1a",
        )
    }

    #[test]
    fn mixed() {
        test_encode_to(b"abcdeeeeeeeeeeeeeeeefg", b"\x03abcd\xF1e\x01fg")
    }

    #[test]
    fn apple_example() {
        test_encode_to(
            &[
                0xAA, 0xAA, 0xAA, 0x80, 0x00, 0x2A, 0xAA, 0xAA, 0xAA, 0xAA, 0x80, 0x00, 0x2A, 0x22,
                0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA,
            ],
            &[
                0xFE, 0xAA, 0x02, 0x80, 0x00, 0x2A, 0xFD, 0xAA, 0x03, 0x80, 0x00, 0x2A, 0x22, 0xF7,
                0xAA,
            ],
        )
    }

    #[test]
    fn empty() {
        test_encode_to(&[], &[])
    }
}
