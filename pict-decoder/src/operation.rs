use std::{cmp, fmt, vec};

use bytes::Buf;
use encoding_rs::WINDOWS_1252;
use strum::FromRepr;
use tracing::warn;

use crate::{
    pixmap::PixMap,
    point::Point,
    quicktime::{ImageDescription, Matrix},
    rectangle::Rectangle,
    utils::{skip_filler, skip_reserved},
    Error, Result,
};

#[derive(PartialEq, Eq, Debug, FromRepr, strum::Display)]
#[repr(u16)]
pub(crate) enum Opcode {
    Nop = 0x0000,
    Clip = 0x0001,
    TxFont = 0x0003,
    TxFace = 0x0004,
    PnSize = 0x0007,
    TxSize = 0x000D,
    TxRatio = 0x0010,
    VersionOp = 0x0011,
    DefHilite = 0x001E,
    LongText = 0x0028,
    DirectBitsRect = 0x009A,
    LongComment = 0x00A1,
    OpEndPic = 0x00FF,
    Version = 0x02FF,
    HeaderOp = 0x0C00,
    CompressedQuickTime = 0x8200,
}

#[allow(dead_code)]
pub(crate) enum Operation {
    Nop,
    Clip {
        size: u16,
        bounding: Rectangle,
    },
    TxFont(i16),
    TxFace(u8),
    PnSize(Point),
    TxSize(i16),
    TxRatio {
        numerator: Point,
        denominator: Point,
    },
    VersionOp,
    DefHilite,
    LongText {
        location: Point,
        text: String,
    },
    DirectBitsRect {
        pix_map: PixMap,
        source: Rectangle,
        destination: Rectangle,
        mode: u16,
        pix_data: Vec<u8>,
    },
    LongComment {
        kind: i16,
        text: String,
    },
    OpEndPic,
    Version,
    HeaderOp {
        version: i16,
        resolution: (u32, u32),
        source: Rectangle,
    },
    CompressedQuickTime {
        version: u16,
        transformation: Matrix,
        matte_rect: Rectangle,
        mode: u16,
        source: Rectangle,
        accuracy: u32,
        mask: Option<Vec<u8>>,
        data: Vec<u8>,
    },
}

impl Operation {
    pub(crate) const fn opcode(&self) -> Opcode {
        match self {
            Self::Nop => Opcode::Nop,
            Self::Clip { .. } => Opcode::Clip,
            Self::PnSize(_) => Opcode::PnSize,
            Self::TxFont(_) => Opcode::TxFont,
            Self::TxFace(_) => Opcode::TxFace,
            Self::VersionOp => Opcode::VersionOp,
            Self::TxSize(_) => Opcode::TxSize,
            Self::TxRatio { .. } => Opcode::TxRatio,
            Self::DefHilite => Opcode::DefHilite,
            Self::LongText { .. } => Opcode::LongText,
            Self::DirectBitsRect { .. } => Opcode::DirectBitsRect,
            Self::LongComment { .. } => Opcode::LongComment,
            Self::OpEndPic => Opcode::OpEndPic,
            Self::Version => Opcode::Version,
            Self::HeaderOp { .. } => Opcode::HeaderOp,
            Self::CompressedQuickTime { .. } => Opcode::CompressedQuickTime,
        }
    }

    pub(crate) fn parse(mut buf: impl Buf) -> Result<Self> {
        let pos = buf.remaining();

        let raw = buf.get_u16();
        let opcode = Opcode::from_repr(raw).ok_or(Error::UnsupportedOpcode(raw))?;

        let op = match opcode {
            Opcode::Nop => Self::Nop,
            Opcode::Clip => Self::Clip {
                size: buf.get_u16(),
                bounding: Rectangle::parse(&mut buf)?,
            },
            Opcode::TxFont => Self::TxFont(buf.get_i16()),
            Opcode::TxFace => {
                let ret = Self::TxFace(buf.get_u8());
                skip_filler(&mut buf)?;
                ret
            }
            Opcode::PnSize => Self::PnSize(Point::parse(&mut buf)?),
            Opcode::TxSize => Self::TxSize(buf.get_i16()),
            Opcode::TxRatio => Self::TxRatio {
                numerator: Point::parse(&mut buf)?,
                denominator: Point::parse(&mut buf)?,
            },
            Opcode::VersionOp => Self::VersionOp,
            Opcode::DefHilite => Self::DefHilite,
            Opcode::LongText => {
                let location = Point::parse(&mut buf)?;
                let mut count = buf.get_u8() as usize;
                let need_filler = count % 2 == 0;

                // no documentation of text format itself
                // MYST.DAT:4001 isn't UTF-8

                let mut decoder = WINDOWS_1252.new_decoder_without_bom_handling();
                let mut text = String::with_capacity(count);
                loop {
                    let chunk = buf.chunk();
                    let chunk_size = cmp::min(chunk.len(), count);
                    let last_chunk = chunk_size == count;

                    let (ret, read) = decoder.decode_to_string_without_replacement(
                        &chunk[..chunk_size],
                        &mut text,
                        last_chunk,
                    );

                    buf.advance(read);
                    count -= read;

                    match ret {
                        encoding_rs::DecoderResult::InputEmpty if last_chunk => break,
                        encoding_rs::DecoderResult::InputEmpty => return Err(Error::UnexpectedEOB),
                        encoding_rs::DecoderResult::OutputFull => text.reserve(1),
                        encoding_rs::DecoderResult::Malformed(_, _) => {
                            return Err(Error::InvalidCP1252Format)
                        }
                    }
                }
                text.shrink_to_fit();

                if need_filler
                // count is a byte so we start at odd bytes
                {
                    skip_filler(&mut buf)?;
                }

                Self::LongText { location, text }
            }
            Opcode::DirectBitsRect => {
                let pix_map = PixMap::parse(&mut buf)?;
                if pix_map.base_addr != 0xFF {
                    panic!("incompatible base address")
                }

                let source = Rectangle::parse(&mut buf)?;
                let destination = Rectangle::parse(&mut buf)?;
                let mode = buf.get_u16();

                let mut odd_bytes_count_read = false;

                let bound_height = pix_map.bounds.bottom - pix_map.bounds.top;
                let pix_data = match pix_map.pack_type {
                    1 => {
                        let size = pix_map.row_bytes as usize * bound_height as usize;

                        let mut ret = vec![0; size];
                        buf.copy_to_slice(&mut ret);
                        odd_bytes_count_read = size % 2 == 1;

                        ret
                    }
                    4 => {
                        let mut ret = Vec::new(); // TODO capacity

                        for _ in 0..bound_height {
                            let encoded_line_size = if pix_map.row_bytes > 250 {
                                buf.get_u16() as usize
                            } else {
                                odd_bytes_count_read = !odd_bytes_count_read;
                                buf.get_u8() as usize
                            };

                            let mut encoded_line = vec![0; encoded_line_size];
                            buf.copy_to_slice(&mut encoded_line);
                            odd_bytes_count_read ^= encoded_line_size % 2 == 1;

                            let mut decoder = packbits::Decoder::new();
                            let mut decoded = decoder.decode(encoded_line.as_slice());
                            let mut line = Vec::with_capacity(encoded_line_size);
                            while decoded.has_remaining() {
                                let chunk = decoded.chunk();
                                line.extend_from_slice(chunk);
                                decoded.advance(chunk.len());
                            }
                            decoder.finalize().unwrap(); // TODO don't panic

                            // each line is somewhat planar encoding of color
                            // first all the red, then all the green, then blue
                            let (r, gb) = line.split_at(line.len() / 3);
                            let (g, b) = gb.split_at(line.len() / 3);
                            assert_eq!(r.len(), g.len());
                            assert_eq!(g.len(), b.len());

                            ret.extend(
                                r.iter()
                                    .zip(g.iter())
                                    .zip(b.iter())
                                    .flat_map(|((a, b), c)| [a, b, c]),
                            );
                        }

                        ret
                    }
                    _ => todo!("unsupported pack type: {}", pix_map.pack_type),
                };

                if odd_bytes_count_read {
                    skip_filler(&mut buf)?;
                }

                Self::DirectBitsRect {
                    pix_map,
                    source,
                    destination,
                    mode,
                    pix_data,
                }
            }
            Opcode::LongComment => {
                let kind = buf.get_i16();
                let size = buf.get_u16();

                let mut raw = vec![0u8; size as usize];
                buf.copy_to_slice(&mut raw);
                let text = WINDOWS_1252
                    .decode_without_bom_handling_and_without_replacement(&raw)
                    .ok_or(Error::InvalidCP1252Format)?;

                Operation::LongComment {
                    kind,
                    text: text.into_owned(),
                }
            }
            Opcode::OpEndPic => Self::OpEndPic, // doc says 2 bytes extra but not reality
            Opcode::Version => Self::Version,
            Opcode::HeaderOp => {
                let version = buf.get_i16();
                skip_reserved(&mut buf, 2)?;
                let resolution = (buf.get_u32(), buf.get_u32());
                let source_rect = Rectangle::parse(&mut buf)?;
                skip_reserved(&mut buf, 4)?;

                Self::HeaderOp {
                    version,
                    resolution,
                    source: source_rect,
                }
            }
            Opcode::CompressedQuickTime => {
                // https://web.archive.org/web/20030827061809/http://developer.apple.com/documentation/QuickTime/INMAC/QT/iqImageCompMgr.a.htm

                let size = buf.get_u32();
                if size % 2 != 0 {
                    panic!("uneven size so padding is wrong")
                }

                let version = buf.get_u16();
                let transformation = Matrix::parse(&mut buf)?;
                let matte_size = buf.get_u32();
                let matte_rect = Rectangle::parse(&mut buf)?;
                let mode = buf.get_u16();
                let source = Rectangle::parse(&mut buf)?;
                let accuracy = buf.get_u32();
                let mask_size = buf.get_u32();

                if matte_size > 0 {
                    panic!("doc not precise on how to handle matte")
                }

                let mask = if mask_size > 0 {
                    let mut ret = vec![0; mask_size as usize];
                    buf.copy_to_slice(&mut ret);
                    Some(ret)
                } else {
                    None
                };

                let img_desc = ImageDescription::parse(&mut buf)?;

                let mut data = vec![0; img_desc.data_size as usize];
                buf.copy_to_slice(&mut data);

                // img_desc.data_size might not run to the end of the opcode
                let diff =
                    size as usize - img_desc.data_size as usize - 68 - ImageDescription::RAW_SIZE;
                if diff > 1 {
                    panic!("too much padding")
                }
                if diff != 0 {
                    skip_filler(&mut buf)?;
                }

                Self::CompressedQuickTime {
                    version,
                    transformation,
                    matte_rect,
                    mode,
                    source,
                    accuracy,
                    mask,
                    data,
                }
            }
        };

        assert_eq!(op.opcode(), opcode, "wrong operation returned for opcode",);
        assert!(
            (pos - buf.remaining()) % 2 == 0,
            "{} should be 2 bytes aligned",
            op.opcode(),
        );

        Ok(op)
    }
}

impl fmt::Display for Operation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_fmt(format_args!("{:04X}", self.opcode() as u16))
    }
}
