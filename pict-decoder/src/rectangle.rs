use bytes::Buf;
use tracing::trace_span;

use crate::{utils::ensure_remains_bytes, Result};

#[derive(Debug)]
#[allow(dead_code)]
pub(crate) struct Rectangle {
    pub(crate) top: u16,
    pub(crate) left: u16,
    pub(crate) bottom: u16,
    pub(crate) right: u16,
}

impl Rectangle {
    pub(crate) fn parse(buf: impl Buf) -> Result<Self> {
        let _span_ = trace_span!("parse Rectangle").entered();

        let mut buf = ensure_remains_bytes(buf, 8)?;

        Ok(Self {
            top: buf.get_u16(),
            left: buf.get_u16(),
            bottom: buf.get_u16(),
            right: buf.get_u16(),
        })
    }
}
