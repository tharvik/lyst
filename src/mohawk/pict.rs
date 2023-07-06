use std::{fmt, string};

use async_stream::try_stream;
use tokio::io::{self, AsyncRead, AsyncReadExt, AsyncSeekExt};
use tokio_stream::{Stream, StreamExt};
use tracing::{trace, warn};

use encoding_rs::WINDOWS_1252;
use strum::FromRepr;

use super::reader::{self, Reader};

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error(transparent)]
    Reader(#[from] reader::Error),

    #[error("non empty header")]
    NonEmptyHeader,
    #[error("unsupported version: {0}")]
    UnsupportedVersion(u8),
    #[error("unsupported header version: {0}")]
    UnsupportedHeaderVersion(i16),
    #[error("unsupported opcode {0:04x}")]
    UnsupportedOpcode(u16),
    #[error("unexpected opcode {0:04x}")]
    UnexpectedOpcode(u16),
    #[error("unable to parse as CP-1252")]
    InvalidCP1252Format,
    #[error("unable to parse as UTF-8: {0}")]
    InvalidUTF8Format(#[from] string::FromUtf8Error),
    #[error("end of picture found but reader is not empty")]
    DataRemaining,
}
type Result<T> = std::result::Result<T, Error>;

// https://www.fileformat.info/format/macpict/egff.htm
// https://preterhuman.net/macstuff/insidemac/QuickDraw/QuickDraw-458.html
// quicktime: https://developer.apple.com/library/archive/documentation/QuickTime/QTFF/QTFFChap1/qtff1.html

pub struct PICT(Vec<u8>);

struct Point(u16, u16);

impl Point {
    async fn parse(reader: &mut (impl AsyncRead + Unpin)) -> Result<Self> {
        Ok(Self(reader.read_u16().await?, reader.read_u16().await?))
    }
}

#[allow(dead_code)]
struct Rectangle {
    top_left: Point,
    bottom_right: Point,
}

impl Rectangle {
    async fn parse(reader: &mut (impl AsyncReadExt + Unpin)) -> Result<Self> {
        Ok(Self {
            top_left: Point::parse(reader).await?,
            bottom_right: Point::parse(reader).await?,
        })
    }
}

// for CompressedQuickTime
struct Matrix([[u32; 3]; 3]);
impl Matrix {
    async fn parse(reader: &mut (impl AsyncReadExt + Unpin)) -> Result<Self> {
        let mut matrix = [[0u32; 3]; 3];

        for line in matrix.iter_mut() {
            for i in line.iter_mut() {
                *i = reader.read_u32().await?;
            }
        }

        Ok(Self(matrix))
    }
}

// for CompressedQuickTime
#[allow(dead_code)]
struct ImageDescription {
    compressor_type: [u8; 4],
    version: u16,
    revision: u16,
    vendor: [u8; 4],
    temporal_quality: u32,
    spatial_quality: u32,
    width: u16,
    height: u16,
    horizontal_resolution: u32,
    vertical_resolution: u32,
    data_size: u32,
    frame_count: u16,
    name: String,
    depth: u16,
    color_table_id: u16,
}
impl ImageDescription {
    const RAW_SIZE: usize = 86;

    async fn parse(reader: &mut Reader) -> Result<Self> {
        let struct_size = reader.read_u32().await?;
        if struct_size != 86 {
            panic!("unexpected struct size: {}", struct_size)
        }
        let compressor_type = reader.read_4_bytes().await?;
        skip_reserved::<8>(reader).await?;
        let version = reader.read_u16().await?;
        let revision = reader.read_u16().await?;
        let vendor = reader.read_4_bytes().await?;
        let temporal_quality = reader.read_u32().await?;
        let spatial_quality = reader.read_u32().await?;
        let width = reader.read_u16().await?;
        let height = reader.read_u16().await?;
        let horizontal_resolution = reader.read_u32().await?;
        let vertical_resolution = reader.read_u32().await?;
        let data_size = reader.read_u32().await?;
        let frame_count = reader.read_u16().await?;

        // pascal string
        let name_size = reader.read_u8().await?;
        let mut raw = vec![0; 31]; // 31 bytes every time
        reader.read_exact(&mut raw).await?;
        let rem = raw.split_off(name_size as usize);
        if rem.into_iter().any(|c| c != 0) {
            warn!("got data after string's end")
        }
        let name = String::from_utf8(raw)?;

        let depth = reader.read_u16().await?;
        let color_table_id = reader.read_u16().await?;

        Ok(Self {
            compressor_type,
            version,
            revision,
            vendor,
            temporal_quality,
            spatial_quality,
            width,
            height,
            horizontal_resolution,
            vertical_resolution,
            data_size,
            frame_count,
            name,
            depth,
            color_table_id,
        })
    }
}

#[derive(PartialEq, Eq, Debug, FromRepr, strum::Display)]
#[repr(u16)]
enum Opcode {
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
    LongComment = 0x00A1,
    OpEndPic = 0x00FF,
    Version = 0x02FF,
    HeaderOp = 0x0C00,
    CompressedQuickTime = 0x8200,
}

#[allow(dead_code)]
enum Operation {
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
    const fn opcode(&self) -> Opcode {
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
            Self::LongComment { .. } => Opcode::LongComment,
            Self::OpEndPic => Opcode::OpEndPic,
            Self::Version => Opcode::Version,
            Self::HeaderOp { .. } => Opcode::HeaderOp,
            Self::CompressedQuickTime { .. } => Opcode::CompressedQuickTime,
        }
    }
}

async fn skip_filler(reader: &mut (impl AsyncReadExt + Unpin)) -> Result<()> {
    let fill = reader.read_u8().await?;
    if fill != 0 {
        panic!("invalid filler: {}", fill)
    }

    Ok(())
}

async fn skip_reserved<const N: usize>(reader: &mut (impl AsyncRead + Unpin)) -> Result<()> {
    // TODO iff not debug => consume

    let mut buf = [0u8; N];
    reader.read_exact(&mut buf).await?;

    if buf.into_iter().any(|b| b != 0) {
        warn!("{} bytes of a reserved field are not zero", N);
    }

    Ok(())
}

impl Operation {
    async fn parse(reader: &mut Reader) -> Result<Self> {
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

    fn read_operations(mut reader: Reader) -> impl Stream<Item = Result<Operation>> + Unpin {
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

    pub async fn parse(mut reader: Reader) -> Result<PICT> {
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
                | Operation::LongComment { .. } => {} // TODO?
                Operation::CompressedQuickTime { data, .. } => {
                    if raw.is_some() {
                        panic!("already got an image")
                    }
                    raw = Some(data)
                }
                Operation::VersionOp | Operation::Version | Operation::HeaderOp { .. } => {
                    return Err(UnexpectedOpcode(op.opcode() as u16))
                }
                Operation::OpEndPic => break,
            }
        }

        Ok(PICT(raw.unwrap())) // TODO checks
    }
}

impl AsRef<[u8]> for PICT {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}
