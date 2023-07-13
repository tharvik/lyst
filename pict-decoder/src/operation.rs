use std::{fmt, vec};

use bytes::Buf;
use encoding_rs::WINDOWS_1252;
use strum::FromRepr;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeek, AsyncSeekExt};

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

    pub(crate) async fn parse(reader: &mut (impl AsyncRead + AsyncSeek + Unpin)) -> Result<Self> {
        let pos = reader.stream_position().await?;

        let raw = reader.read_u16().await?;
        let opcode = Opcode::from_repr(raw).ok_or(Error::UnsupportedOpcode(raw))?;

        let op = match opcode {
            Opcode::Nop => Self::Nop,
            Opcode::Clip => Self::Clip {
                size: reader.read_u16().await?,
                bounding: Rectangle::parse(reader).await?,
            },
            Opcode::TxFont => Self::TxFont(reader.read_i16().await?),
            Opcode::TxFace => {
                let ret = Self::TxFace(reader.read_u8().await?);
                skip_filler(reader).await?;
                ret
            }
            Opcode::PnSize => Self::PnSize(Point::parse(reader).await?),
            Opcode::TxSize => Self::TxSize(reader.read_i16().await?),
            Opcode::TxRatio => Self::TxRatio {
                numerator: Point::parse(reader).await?,
                denominator: Point::parse(reader).await?,
            },
            Opcode::VersionOp => Self::VersionOp,
            Opcode::DefHilite => Self::DefHilite,
            Opcode::LongText => {
                let location = Point::parse(reader).await?;
                let count = reader.read_u8().await?;

                // no documentation of text format itself
                // MYST.DAT:4001 isn't UTF-8

                let mut raw = vec![0u8; count as usize];
                reader.read_exact(&mut raw).await?;
                let text = WINDOWS_1252
                    .decode_without_bom_handling_and_without_replacement(&raw)
                    .ok_or(Error::InvalidCP1252Format)?;

                if count % 2 == 0
                // count is a byte so we start at odd bytes
                {
                    skip_filler(reader).await?;
                }

                Self::LongText {
                    location,
                    text: text.into_owned(),
                }
            }
            Opcode::DirectBitsRect => {
                let pix_map = PixMap::parse(reader).await?;
                if pix_map.base_addr != 0xFF {
                    panic!("incompatible base address")
                }

                let source = Rectangle::parse(reader).await?;
                let destination = Rectangle::parse(reader).await?;
                let mode = reader.read_u16().await?;

                let mut odd_bytes_count_read = false;

                let bound_height = pix_map.bounds.bottom - pix_map.bounds.top;
                let pix_data = match pix_map.pack_type {
                    1 => {
                        let size = pix_map.row_bytes as usize * bound_height as usize;

                        let mut ret = vec![0; size];
                        reader.read_exact(&mut ret).await?;
                        odd_bytes_count_read = size % 2 == 1;

                        ret
                    }
                    4 => {
                        let mut ret = Vec::new(); // TODO capacity

                        for _ in 0..bound_height {
                            let encoded_line_size = if pix_map.row_bytes > 250 {
                                reader.read_u16().await? as usize
                            } else {
                                odd_bytes_count_read = !odd_bytes_count_read;
                                reader.read_u8().await? as usize
                            };

                            let mut encoded_line = vec![0; encoded_line_size];
                            reader.read_exact(&mut encoded_line).await?;
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
                    skip_filler(reader).await?;
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
                let kind = reader.read_i16().await?;
                let size = reader.read_u16().await?;

                let mut raw = vec![0u8; size as usize];
                reader.read_exact(&mut raw).await?;
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
                let version = reader.read_i16().await?;
                skip_reserved::<2>(reader).await?;
                let resolution = (reader.read_u32().await?, reader.read_u32().await?);
                let source_rect = Rectangle::parse(reader).await?;
                skip_reserved::<4>(reader).await?;

                Self::HeaderOp {
                    version,
                    resolution,
                    source: source_rect,
                }
            }
            Opcode::CompressedQuickTime => {
                // https://web.archive.org/web/20030827061809/http://developer.apple.com/documentation/QuickTime/INMAC/QT/iqImageCompMgr.a.htm

                let size = reader.read_u32().await?;
                if size % 2 != 0 {
                    panic!("uneven size so padding is wrong")
                }

                let version = reader.read_u16().await?;
                let transformation = Matrix::parse(reader).await?;
                let matte_size = reader.read_u32().await?;
                let matte_rect = Rectangle::parse(reader).await?;
                let mode = reader.read_u16().await?;
                let source = Rectangle::parse(reader).await?;
                let accuracy = reader.read_u32().await?;
                let mask_size = reader.read_u32().await?;

                if matte_size > 0 {
                    panic!("doc not precise on how to handle matte")
                }

                let mask = if mask_size > 0 {
                    let mut buf = vec![0; mask_size as usize];
                    reader.read_exact(&mut buf).await?;
                    Some(buf)
                } else {
                    None
                };

                let img_desc = ImageDescription::parse(reader).await?;

                let mut data = vec![0; img_desc.data_size as usize];
                reader.read_exact(&mut data).await?;

                // img_desc.data_size might not run to the end of the opcode
                let diff =
                    size as usize - img_desc.data_size as usize - 68 - ImageDescription::RAW_SIZE;
                if diff > 1 {
                    panic!("too much padding")
                }
                if diff != 0 {
                    skip_filler(reader).await?;
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
            (reader.stream_position().await? - pos) % 2 == 0,
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
