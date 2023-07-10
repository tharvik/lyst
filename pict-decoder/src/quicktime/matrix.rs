use tokio::io::{AsyncRead, AsyncReadExt};

use crate::Result;

pub(crate) struct Matrix([[u32; 3]; 3]);

impl Matrix {
    pub(crate) async fn parse(reader: &mut (impl AsyncRead + Unpin)) -> Result<Self> {
        let mut matrix = [[0u32; 3]; 3];

        for line in matrix.iter_mut() {
            for i in line.iter_mut() {
                *i = reader.read_u32().await?;
            }
        }

        Ok(Self(matrix))
    }
}
