use tokio::io::{AsyncRead, AsyncReadExt};

use crate::Result;

#[allow(dead_code)]
pub(crate) struct Point {
    pub(crate) x: u16,
    pub(crate) y: u16,
}

impl Point {
    pub(crate) async fn parse(reader: &mut (impl AsyncRead + Unpin)) -> Result<Self> {
        Ok(Self {
            x: reader.read_u16().await?,
            y: reader.read_u16().await?,
        })
    }
}
