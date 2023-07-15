use bytes::Buf;

use crate::{utils::ensure_remains_bytes, Result};

pub(crate) struct Matrix([[u32; 3]; 3]);

impl Matrix {
    pub(crate) fn parse(buf: impl Buf) -> Result<Self> {
        let mut buf = ensure_remains_bytes(buf, 36)?;
        let mut matrix = [[0u32; 3]; 3];

        for line in matrix.iter_mut() {
            for i in line.iter_mut() {
                *i = buf.get_u32();
            }
        }

        Ok(Self(matrix))
    }
}
