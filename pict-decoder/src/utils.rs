use tracing::warn;

use bytes::Buf;

use crate::{Error, Result};

pub(crate) fn ensure_remains_bytes(buf: impl Buf, amount: usize) -> Result<impl Buf> {
    if buf.remaining() < amount {
        return Err(Error::UnexpectedEOB);
    }

    Ok(buf.take(amount))
}

pub(crate) fn skip_filler(mut buf: impl Buf) -> Result<()> {
    let fill = buf.get_u8();
    if fill != 0 {
        return Err(Error::InvalidFiller(fill));
    }

    Ok(())
}

pub(crate) fn skip_reserved(buf: impl Buf, amount: usize) -> Result<()> {
    let mut buf = ensure_remains_bytes(buf, amount)?;

    let unexpected_count: usize = (0..amount)
        .map(|_| {
            let byte = buf.get_u8();
            (byte != 0) as usize
        })
        .sum();

    if unexpected_count > 0 {
        warn!(
            "{} bytes of a reserved field are not zero",
            unexpected_count
        );
    }

    Ok(())
}
