use std::cmp;

use bytes::{Buf, BufMut, BytesMut};

use crate::{Error, Result};

enum State {
    Idle,
    Literal { remaining: usize },
    Repeat { count: usize },
}

/// Decoder for the PackBits algorithm
pub struct Decoder(State);

impl Decoder {
    /// New Decoder writing result to given [`BufMut`]
    pub fn new() -> Self {
        Self(State::Idle)
    }

    /// Decode a chunk of data
    pub fn decode(&mut self, input: impl Buf) -> impl Buf {
        let mut ret = BytesMut::with_capacity(input.remaining());
        self.decode_on(input, &mut ret);
        ret
    }

    /// Decode a chunk of data and write result in given [`BufMut`]
    ///
    /// Panic if output hasn't enough space.
    pub fn decode_on(&mut self, mut input: impl Buf, output: &mut impl BufMut) {
        while input.has_remaining() {
            match &mut self.0 {
                State::Idle => {
                    let byte = input.get_i8();
                    self.0 = match byte {
                        0..=127 => State::Literal {
                            remaining: 1 + byte as usize,
                        },
                        -127..=-1 => State::Repeat {
                            count: (-byte) as usize + 1,
                        },
                        -128 => State::Idle, // NOP
                    }
                }
                State::Repeat { count } => {
                    let byte = input.get_u8();
                    output.put_bytes(byte, *count);
                    self.0 = State::Idle;
                }
                State::Literal { ref mut remaining } => {
                    let chunk = input.chunk();
                    let size = cmp::min(chunk.len(), *remaining);

                    output.put(&chunk[..size]);

                    input.advance(size);
                    *remaining -= size;

                    if *remaining == 0 {
                        self.0 = State::Idle;
                    }
                }
            }
        }
    }

    /// Finish decoding, returns Ok if the stream was correctly ended
    ///
    /// There is no data returned as [`Self::decode`] processes data eagerly
    pub fn finalize(&mut self) -> Result<()> {
        match self.0 {
            State::Idle => Ok(()),
            State::Repeat { .. } => Err(Error::DanglingRepeated),
            State::Literal { remaining } => Err(Error::DanglingLiteral(remaining)),
        }
    }
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}
