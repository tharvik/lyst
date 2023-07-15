use bytes::Buf;
use tracing::warn;

use crate::{
    utils::{ensure_remains_bytes, skip_reserved},
    Result,
};

fn read_4_bytes(buf: impl Buf) -> Result<[u8; 4]> {
    let mut buf = ensure_remains_bytes(buf, 4)?;

    let mut buffer = [0u8; 4];
    buf.copy_to_slice(&mut buffer);

    Ok(buffer)
}

#[allow(dead_code)]
pub(crate) struct ImageDescription {
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
    pub(crate) data_size: u32,
    frame_count: u16,
    name: String,
    depth: u16,
    color_table_id: u16,
}

impl ImageDescription {
    pub(crate) const RAW_SIZE: usize = 86;

    pub(crate) fn parse(buf: impl Buf) -> Result<Self> {
        let mut buf = ensure_remains_bytes(buf, Self::RAW_SIZE)?;

        let struct_size = buf.get_u32();
        if struct_size as usize != Self::RAW_SIZE {
            panic!("unexpected struct size: {}", struct_size)
        }
        let compressor_type = read_4_bytes(&mut buf)?;
        skip_reserved(&mut buf, 8)?;
        let version = buf.get_u16();
        let revision = buf.get_u16();
        let vendor = read_4_bytes(&mut buf)?;
        let temporal_quality = buf.get_u32();
        let spatial_quality = buf.get_u32();
        let width = buf.get_u16();
        let height = buf.get_u16();
        let horizontal_resolution = buf.get_u32();
        let vertical_resolution = buf.get_u32();
        let data_size = buf.get_u32();
        let frame_count = buf.get_u16();

        // pascal string
        let name_size = buf.get_u8();
        let mut raw = vec![0; 31]; // 31 bytes every time
        buf.copy_to_slice(&mut raw);
        let rem = raw.split_off(name_size as usize);
        if rem.into_iter().any(|c| c != 0) {
            warn!("got data after string's end")
        }
        let name = String::from_utf8(raw)?;

        let depth = buf.get_u16();
        let color_table_id = buf.get_u16();

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
