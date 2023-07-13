use tokio::io::{AsyncRead, AsyncReadExt};

use crate::Result;

#[derive(Debug)]
#[allow(dead_code)]
pub(crate) struct Rectangle {
    pub(crate) top: u16,
    pub(crate) left: u16,
    pub(crate) bottom: u16,
    pub(crate) right: u16,
}

impl Rectangle {
    pub(crate) async fn parse(reader: &mut (impl AsyncRead + Unpin)) -> Result<Self> {
        Ok(Self {
            top: reader.read_u16().await?,
            left: reader.read_u16().await?,
            bottom: reader.read_u16().await?,
            right: reader.read_u16().await?,
        })
    }
}
