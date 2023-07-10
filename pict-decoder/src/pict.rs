use std::io;

use async_stream::try_stream;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeek};
use tokio_stream::{Stream, StreamExt};
use tracing::trace;

use crate::{
    operation::{Opcode, Operation},
    rectangle::Rectangle,
    Error, Result,
};

// https://www.fileformat.info/format/macpict/egff.htm
// https://preterhuman.net/macstuff/insidemac/QuickDraw/QuickDraw-458.html

pub struct PICT(Vec<u8>);

impl PICT {
    async fn expect_op(
        stream: &mut (impl Stream<Item = Result<Operation>> + Unpin),
        expected: Opcode,
    ) -> Result<Operation> {
        let op = stream
            .next()
            .await
            .ok_or(io::Error::from(io::ErrorKind::UnexpectedEof))
            .map_err(Error::Reader)??; // behave as failing read

        if op.opcode() != expected {
            return Err(Error::UnexpectedOpcode(op.opcode() as u16));
        }

        Ok(op)
    }

    fn read_operations(
        mut reader: impl AsyncRead + AsyncSeek + Unpin,
    ) -> impl Stream<Item = Result<Operation>> + Unpin {
        Box::pin(try_stream! {
            loop {
                let op = Operation::parse(&mut reader).await?;

                if let Operation::OpEndPic = op {
                    if reader.read(&mut[0]).await? != 0 {
                        Err(Error::DataRemaining)?
                    }
                }

                yield op;
            }
        })
    }

    pub async fn parse(mut reader: impl AsyncRead + AsyncSeek + Unpin) -> Result<PICT> {
        use Error::*;

        let mut empty_header = [0u8; 512];
        reader.read_exact(&mut empty_header).await?;
        if !empty_header.into_iter().all(|b| b == 0) {
            return Err(NonEmptyHeader);
        }

        let _size = reader.read_u16().await?;
        let _bounding_rect = Rectangle::parse(&mut reader).await;

        let mut opcodes = Self::read_operations(reader);

        Self::expect_op(&mut opcodes, Opcode::VersionOp).await?;
        Self::expect_op(&mut opcodes, Opcode::Version).await?;
        if let Operation::HeaderOp { version: v, .. } =
            Self::expect_op(&mut opcodes, Opcode::HeaderOp).await?
        {
            if v != -2 {
                return Err(UnsupportedHeaderVersion(v));
            }
        }

        let mut raw = None;
        while let Some(res) = opcodes.next().await {
            let op = res?;
            trace!("exec op: {}", op);

            match op {
                Operation::Nop => {}
                Operation::DefHilite
                | Operation::Clip { .. }
                | Operation::TxFont(_)
                | Operation::TxFace(_)
                | Operation::PnSize(_)
                | Operation::TxSize(_)
                | Operation::TxRatio { .. }
                | Operation::LongText { .. }
                | Operation::LongComment { .. } => {} // TODO anything?
                Operation::CompressedQuickTime { data, .. } => {
                    if raw.is_some() {
                        panic!("already got an image")
                    }
                    raw = Some(data)
                }
                Operation::DirectBitsRect { .. } => {
                    panic!("gotta canvas this")
                }
                Operation::VersionOp | Operation::Version | Operation::HeaderOp { .. } => {
                    return Err(UnexpectedOpcode(op.opcode() as u16))
                }
                Operation::OpEndPic => break,
            }
        }

        raw.ok_or(UnableToFindImage).map(PICT)
    }
}

impl AsRef<[u8]> for PICT {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}
