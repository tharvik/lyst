use std::io;

use tokio::io::{AsyncRead, AsyncReadExt};
use tracing::warn;

use crate::{utils::skip_reserved, Result};

async fn read_4_bytes(reader: &mut (impl AsyncRead + Unpin)) -> io::Result<[u8; 4]> {
    let mut buffer = [0u8; 4];
    reader.read_exact(&mut buffer).await?;

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

    pub(crate) async fn parse(reader: &mut (impl AsyncRead + Unpin)) -> Result<Self> {
        let struct_size = reader.read_u32().await?;
        if struct_size != 86 {
            panic!("unexpected struct size: {}", struct_size)
        }
        let compressor_type = read_4_bytes(reader).await?;
        skip_reserved::<8>(reader).await?;
        let version = reader.read_u16().await?;
        let revision = reader.read_u16().await?;
        let vendor = read_4_bytes(reader).await?;
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
