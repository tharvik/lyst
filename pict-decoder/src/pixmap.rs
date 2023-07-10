use tokio::io::{AsyncRead, AsyncReadExt};

use strum::FromRepr;
use tracing::warn;

use crate::{rectangle::Rectangle, Result};

#[derive(FromRepr)]
#[repr(u16)]
enum PixelType {
    Indexed = 0,
    Direct = 16,
}

#[allow(dead_code)]
pub(crate) struct PixMap {
    base_addr: u32,
    pub(crate) row_bytes: u16,
    pointed_is_pixmap_record: bool,
    pub(crate) bounds: Rectangle,
    version: u16,
    pub(crate) pack_type: u16,
    pack_size: u32,
    horizontal_resolution: u32,
    vertical_resolution: u32,
    pixel_type: PixelType,
    pixel_size: u16,
    components_count: u16,
    components_size: u16,
    plane_offset: u32,
    color_table_addr: u32,
}

impl PixMap {
    pub(crate) async fn parse(reader: &mut (impl AsyncRead + Unpin)) -> Result<Self> {
        let base_addr = reader.read_u32().await?;
        if base_addr % 4 != 0 {
            warn!("unoptimal PixMap base addr")
        }

        let row_bytes_and_flag = reader.read_u16().await?;
        let row_bytes = row_bytes_and_flag & 0x3fff;
        if row_bytes % 2 != 0 || row_bytes >= 0x4000 {
            panic!("invalid row bytes")
        }
        if row_bytes % 4 != 0 {
            warn!("unoptimal row bytes")
        }
        let flags = row_bytes_and_flag >> 14;
        if flags & 0b01 != 0 {
            panic!("unexpected row bytes 15th bit: {:b}", row_bytes_and_flag)
        }
        let pointed_is_pixmap_record = flags == 0b10;

        let bounds = Rectangle::parse(reader).await?;
        let version = reader.read_u16().await?;
        if version != 0 {
            panic!("unexpected version: {}", version)
        }

        let pack_type = reader.read_u16().await?;
        if pack_type >= 5 {
            panic!("unsupported pack_type: {}", pack_type)
        }
        let pack_size = reader.read_u32().await?;
        if pack_type == 0 && pack_size != 0 {
            panic!("image not packed but pack size not zero: {}", pack_size)
        }
        let horizontal_resolution = reader.read_u32().await?;
        let vertical_resolution = reader.read_u32().await?;
        let pixel_type =
            PixelType::from_repr(reader.read_u16().await?).expect("to be a valid pixel type");
        let pixel_size = reader.read_u16().await?;
        let components_count = reader.read_u16().await?;
        let components_size = reader.read_u16().await?;
        let plane_offset = reader.read_u32().await?;
        let color_table_addr = reader.read_u32().await?;

        Ok(Self {
            base_addr,
            row_bytes,
            pointed_is_pixmap_record,
            bounds,
            version,
            pack_type,
            pack_size,
            horizontal_resolution,
            vertical_resolution,
            pixel_type,
            pixel_size,
            components_count,
            components_size,
            plane_offset,
            color_table_addr,
        })
    }
}
