use bytes::Buf;
use strum::FromRepr;
use tracing::{trace_span, warn};

use crate::{
    rectangle::Rectangle,
    utils::{ensure_remains_bytes, skip_reserved},
    Result,
};

#[derive(FromRepr)]
#[repr(u16)]
enum PixelType {
    Indexed = 0,
    Direct = 16,
}

#[allow(dead_code)]
pub(crate) struct PixMap {
    pub(crate) base_addr: u32,
    pub(crate) row_bytes: u16,
    pointed_is_pixmap_record: bool,
    pub(crate) bounds: Rectangle,
    version: u16,
    pub(crate) pack_type: u16,
    pub(crate) pack_size: u32,
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
    pub(crate) fn parse(buf: impl Buf) -> Result<Self> {
        let _span_ = trace_span!("parse PixMap").entered();

        let mut buf = ensure_remains_bytes(buf, 50)?;

        let base_addr = buf.get_u32();
        if base_addr % 4 != 0 {
            warn!(
                "suboptimal base addr: 0x{:08x} is not a 4 bytes aligned",
                base_addr
            )
        }

        let row_bytes_and_flag = buf.get_u16();
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

        let bounds = Rectangle::parse(&mut buf)?;
        let version = buf.get_u16();
        if version != 0 {
            panic!("unexpected version: {}", version)
        }

        let pack_type = buf.get_u16();
        if pack_type >= 5 {
            panic!("unsupported pack_type: {}", pack_type)
        }
        let pack_size = buf.get_u32();
        if pack_type == 0 && pack_size != 0 {
            panic!("image not packed but pack size not zero: {}", pack_size)
        }
        let horizontal_resolution = buf.get_u32();
        let vertical_resolution = buf.get_u32();
        let pixel_type = PixelType::from_repr(buf.get_u16()).expect("to be a valid pixel type");
        let pixel_size = buf.get_u16();
        let components_count = buf.get_u16();
        let components_size = buf.get_u16();
        let plane_offset = buf.get_u32();
        let color_table_addr = buf.get_u32();
        skip_reserved(buf, 4)?;

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
