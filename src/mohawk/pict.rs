use tokio::io::AsyncReadExt;

use crate::{
    errors::{InvalidHeaderError, PICTError},
    Result,
};

use super::reader::MohawkReader;

pub struct PICT;

impl PICT {
    pub async fn parse(mut reader: MohawkReader) -> Result<Self> {
        let mut empty_header = [0u8; 512];
        reader.read_exact(&mut empty_header).await?;
        if !empty_header.into_iter().all(|b| b == 0) {
            Err(InvalidHeaderError::PICT(PICTError::NonEmptyHeader))?
        }

        Ok(PICT)
    }
}
