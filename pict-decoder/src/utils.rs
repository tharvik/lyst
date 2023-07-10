use tracing::warn;

use tokio::io::{AsyncRead, AsyncReadExt};

use crate::Result;

pub async fn skip_filler(reader: &mut (impl AsyncRead + Unpin)) -> Result<()> {
    let fill = reader.read_u8().await?;
    if fill != 0 {
        panic!("invalid filler: {}", fill)
    }

    Ok(())
}

pub async fn skip_reserved<const N: usize>(reader: &mut (impl AsyncRead + Unpin)) -> Result<()> {
    let mut buf = [0u8; N];
    reader.read_exact(&mut buf).await?;

    if buf.into_iter().any(|b| b != 0) {
        warn!("{} bytes of a reserved field are not zero", N);
    }

    Ok(())
}
