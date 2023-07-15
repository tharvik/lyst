use std::iter;

use bytes::Buf;
use tracing::trace;

use crate::{
    operation::{Opcode, Operation},
    rectangle::Rectangle,
    Error, Result,
};

// https://www.fileformat.info/format/macpict/egff.htm
// https://preterhuman.net/macstuff/insidemac/QuickDraw/QuickDraw-458.html

pub enum PICT {
    JPEG(Vec<u8>),
    RGB24 {
        width: usize,
        height: usize,
        data: Vec<u8>,
    },
}

impl PICT {
    fn expect_op(
        iter: &mut impl Iterator<Item = Result<Operation>>,
        expected: Opcode,
    ) -> Result<Operation> {
        let op = iter.next().ok_or(Error::UnexpectedEOB)??;

        if op.opcode() != expected {
            return Err(Error::UnexpectedOpcode(op.opcode() as u16));
        }

        Ok(op)
    }

    pub fn parse(mut buf: impl Buf) -> Result<PICT> {
        use Error::*;

        let mut empty_header = [0u8; 512];
        buf.copy_to_slice(&mut empty_header);
        if !empty_header.into_iter().all(|b| b == 0) {
            return Err(NonEmptyHeader);
        }

        let _size = buf.get_u16();
        let _bounding_rect = Rectangle::parse(&mut buf)?;

        let mut opcodes = iter::from_fn(|| buf.has_remaining().then(|| Operation::parse(&mut buf)));

        Self::expect_op(&mut opcodes, Opcode::VersionOp)?;
        Self::expect_op(&mut opcodes, Opcode::Version)?;
        if let Operation::HeaderOp { version: v, .. } =
            Self::expect_op(&mut opcodes, Opcode::HeaderOp)?
        {
            if v != -2 {
                return Err(UnsupportedHeaderVersion(v));
            }
        }

        let mut ret = None;
        for res in opcodes.by_ref() {
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
                    if ret.is_some() {
                        panic!("already got an image")
                    }
                    ret = Some(Self::JPEG(data))
                }
                Operation::DirectBitsRect {
                    pix_data,
                    destination,
                    ..
                } => {
                    if ret.is_some() {
                        panic!("already got an image")
                    }
                    ret = Some(Self::RGB24 {
                        width: (destination.right - destination.left) as usize,
                        height: (destination.bottom - destination.top) as usize,
                        data: pix_data,
                    })
                }
                Operation::VersionOp | Operation::Version | Operation::HeaderOp { .. } => {
                    return Err(UnexpectedOpcode(op.opcode() as u16))
                }
                Operation::OpEndPic => break,
            }
        }

        if opcodes.next().is_some() {
            return Err(DataRemaining);
        }

        ret.ok_or(UnableToFindImage)
    }
}
