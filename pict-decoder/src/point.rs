use bytes::Buf;

use crate::{utils::ensure_remains_bytes, Result};

#[allow(dead_code)]
pub(crate) struct Point {
    pub(crate) x: u16,
    pub(crate) y: u16,
}

impl Point {
    pub(crate) fn parse(buf: impl Buf) -> Result<Self> {
        let mut buf = ensure_remains_bytes(buf, 4)?;

        Ok(Self {
            x: buf.get_u16(),
            y: buf.get_u16(),
        })
    }
}
