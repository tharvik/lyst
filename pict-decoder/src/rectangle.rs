use tokio::io::AsyncRead;

use crate::{point::Point, Result};

pub(crate) struct Rectangle {
    pub(crate) top_left: Point,
    pub(crate) bottom_right: Point,
}

impl Rectangle {
    pub(crate) async fn parse(reader: &mut (impl AsyncRead + Unpin)) -> Result<Self> {
        Ok(Self {
            top_left: Point::parse(reader).await?,
            bottom_right: Point::parse(reader).await?,
        })
    }
}
