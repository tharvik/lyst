pub mod pict;

use core::fmt;
use std::{collections::HashMap, fmt::Write, io::SeekFrom, path::Path, string};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncSeekExt};
use tracing::{trace, trace_span, warn};

use async_stream::try_stream;
use tokio_stream::StreamExt;

mod reader;
use reader::Reader;

use self::pict::PICT;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error(transparent)]
    Reader(#[from] reader::Error),

    #[error("unexpected IFF signature")]
    IFFSignature,
    #[error("unexpected RSRC signature")]
    RSRCSignature,
    #[error("unsupported version: {0}")]
    UnsupportedVersion(u16),
    #[error("unsupported compaction: {0}")]
    UnsupportedCompaction(u16),
    #[error("uncoherent file size")]
    UncoherentFileSize,
    #[error("file id not found in table")]
    UnknownFileID,
    #[error("too big file table")]
    TooBigFileTable,
    #[error("uncoherent file table size")]
    UncoherentFileTableSize,
    #[error("unable to parse as UTF-8: {0}")]
    InvalidUTF8Format(#[from] string::FromUtf8Error),

    #[error("parse pict: {0}")]
    PICT(#[from] pict::Error),
}
pub type Result<T> = std::result::Result<T, Error>;

pub struct Resource {
    pub name: Option<String>,
    pub file: File,
    reader: Reader,
}

pub struct File {
    offset: u64,
    pub size: u32,
    pub flag: u8,
    pub unknown: u16,
}

impl Resource {
    pub async fn new(name: Option<String>, file: File, mut reader: Reader) -> Result<Self> {
        reader.seek(SeekFrom::Start(file.offset)).await?;

        Ok(Self { name, file, reader })
    }

    pub fn read(&self) -> Reader {
        Reader::take(&self.reader, self.file.size as usize)
    }
}

#[derive(PartialEq, Eq, Hash, PartialOrd, Ord, Clone)]
pub enum TypeID {
    PICT,
    MSND,
    Unknown([u8; 4]),
}

impl From<[u8; 4]> for TypeID {
    fn from(value: [u8; 4]) -> Self {
        match &value {
            b"PICT" => Self::PICT,
            b"MSND" => Self::MSND,
            _ => Self::Unknown(value),
        }
    }
}

impl fmt::Display for TypeID {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PICT => f.write_str("PICT"),
            Self::MSND => f.write_str("MSND"),
            Self::Unknown(arr) => {
                for b in arr {
                    f.write_char(char::from(*b))?
                }
                Ok(())
            }
        }
    }
}

pub type ResourceID = u16;

type FileID = u16;

// http://insidethelink.ortiche.net/wiki/index.php/Mohawk_archive_format
pub struct Mohawk {
    //pub msnd: Option<HashMap<ResourceID, Resource>>,
    pub types: HashMap<TypeID, HashMap<ResourceID, Resource>>,
}

impl Mohawk {
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        let _span_ = trace_span!("open", "path={}", path.as_ref().display()).entered();

        let mut reader = Reader::open(path).await?;

        let total_file_size = parse_iff_header(&mut reader).await?;
        trace!(total_file_size, "iff parsed");
        let RSRCHeader {
            resource_dir_offset,
            file_table_offset_in_resource_dir,
            file_table_size,
        } = parse_rsrc_header(&mut reader, total_file_size).await?;
        trace!(
            resource_dir_offset,
            file_table_offset_in_resource_dir,
            file_table_size,
            "rsrc parsed"
        );

        reader
            .seek(SeekFrom::Start(resource_dir_offset.into()))
            .await?;
        let name_list_offset_in_resource_dir = reader.read_u16().await?;
        let mut type_tables = parse_type_table(&mut reader).await?;
        trace!("types table parsed: {} found", type_tables.len());
        // reduce backward seek
        type_tables.sort_unstable_by_key(|(_, t)| t.resource_table_offset_in_resource_dir);

        reader
            .seek(SeekFrom::Start(
                resource_dir_offset as u64 + file_table_offset_in_resource_dir as u64,
            ))
            .await?;
        let mut files = parse_file_table(&mut reader, file_table_size).await?;

        let reader_dup = reader.clone();
        let types_without_files = try_stream! {
            let mut reader = reader_dup;
            for (resource_type, entry) in type_tables {
                let _span_ = trace_span!("parse", "type" = %resource_type).entered();

                reader
                    .seek(SeekFrom::Start(
                        resource_dir_offset as u64 + entry.resource_table_offset_in_resource_dir as u64,
                    ))
                    .await?;
                let resource_table = parse_resource_table(&mut reader).await?;
                trace!("got {} resources", resource_table.len());

                reader
                    .seek(SeekFrom::Start(
                        resource_dir_offset as u64 + entry.name_table_offset_in_resource_dir as u64,
                    ))
                    .await?;
                let mut name_table = parse_name_table(&mut reader).await?;
                trace!("got {} names", name_table.len());
                name_table.sort_unstable_by_key(|(_, off)| *off); // try to make linear access

                let mut reader = reader.clone();
                let mut resource_id_to_name: HashMap<ResourceID, String> = try_stream! {
                    for (resource_id, name_offset_in_name_list) in name_table {
                        reader
                            .seek(SeekFrom::Start(
                                resource_dir_offset as u64
                                    + name_list_offset_in_resource_dir as u64
                                    + name_offset_in_name_list as u64,
                            ))
                            .await?;

                        let mut c_string = vec![];
                        reader.read_until(0u8, &mut c_string).await?;
                        c_string.remove(c_string.len() - 1);

                        let name = String::from_utf8(c_string)?;

                        yield (resource_id, name)
                    }
                }
                .collect::<Result<Vec<_>>>()
                .await?
                .into_iter()
                .collect();

                let resources = resource_table
                    .into_iter()
                    .map(|(resource_id, file_id)| (
                        resource_id,
                        file_id,
                        resource_id_to_name.remove(&file_id),
                    ))
                    .collect::<Vec<_>>();
                if !resource_id_to_name.is_empty() {
                    warn!("{} names unmatched to resources", resource_id_to_name.len())
                }

                yield (resource_type, resources);
            }
        }
        .collect::<Result<Vec<_>>>().await?;

        let types_without_resources = types_without_files
            .into_iter()
            .map(|(resource_type, resources)| {
                Ok((
                    resource_type,
                    resources
                        .into_iter()
                        .map(|(resource_id, file_id, name)| {
                            Ok((
                                resource_id,
                                files.remove(&file_id).ok_or(Error::UnknownFileID)?,
                                name,
                            ))
                        })
                        .collect::<Result<Vec<_>>>()?,
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        if !files.is_empty() {
            warn!("{} files unmatched to resources", files.len())
        }

        let types = try_stream! {
            for (resource_type, resources) in types_without_resources {
                let reader = reader.clone();
                yield (
                    resource_type,
                    try_stream! {
                        for (id, file, name) in resources {
                            yield (
                                id,
                                Resource::new(name, file, reader.clone()).await?,
                            );
                        }
                    }
                    .collect::<Result<Vec<_>>>().await?
                    .into_iter().collect::<HashMap<ResourceID, Resource>>(),
                );
            }

        }
        .collect::<Result<Vec<_>>>()
        .await?
        .into_iter()
        .collect::<HashMap<TypeID, _>>();

        Ok(Self { types })
    }

    pub async fn get_pict(&self, id: &ResourceID) -> Option<Result<PICT>> {
        trace!("get pict");

        let res = self.types.get(&TypeID::PICT).and_then(|m| m.get(id))?;

        Some(PICT::parse(res.read()).await.map_err(Error::PICT))
    }
}

/// parse IFF header and return total file size
async fn parse_iff_header(reader: &mut Reader) -> Result<u32> {
    if reader.read_4_bytes().await? != *b"MHWK" {
        Err(Error::IFFSignature)?;
    }

    reader
        .read_u32()
        .await
        .map(|size| size + 8)
        .map_err(Error::Reader)
}

struct RSRCHeader {
    pub resource_dir_offset: u32,
    pub file_table_offset_in_resource_dir: u16,
    pub file_table_size: u16,
}

/// parse RSRC header and return its content
async fn parse_rsrc_header(reader: &mut Reader, total_file_size: u32) -> Result<RSRCHeader> {
    if reader.read_4_bytes().await? != *b"RSRC" {
        Err(Error::RSRCSignature)?;
    }
    let version = reader.read_u16().await?;
    if version != 0x100 {
        Err(Error::UnsupportedVersion(version))?;
    }
    let compaction = reader.read_u16().await?;
    if compaction != 0x1 {
        Err(Error::UnsupportedCompaction(compaction))?;
    }
    if reader.read_u32().await? != total_file_size {
        Err(Error::UncoherentFileSize)?;
    }

    Ok(RSRCHeader {
        resource_dir_offset: reader.read_u32().await?,
        file_table_offset_in_resource_dir: reader.read_u16().await?,
        file_table_size: reader.read_u16().await?,
    })
}

struct TypeTableEntry {
    resource_table_offset_in_resource_dir: u16,
    name_table_offset_in_resource_dir: u16,
}

async fn parse_type_table(reader: &mut Reader) -> Result<Vec<(TypeID, TypeTableEntry)>> {
    let type_entry_count = reader.read_u16().await?;
    try_stream! {
        for _ in 0..type_entry_count {
            yield (
                TypeID::from(reader.read_4_bytes().await?),
                TypeTableEntry {
                    resource_table_offset_in_resource_dir: reader.read_u16().await?,
                    name_table_offset_in_resource_dir: reader.read_u16().await?,
                },
            );
        }
    }
    .collect::<Result<_>>()
    .await
}

async fn parse_name_table(reader: &mut Reader) -> Result<Vec<(ResourceID, u16)>> {
    let names_count = reader.read_u16().await?;

    try_stream! {
        for _ in 0..names_count {
            let name_offset_in_name_list = reader.read_u16().await?;
            let resource_id = reader.read_u16().await?;

            yield (resource_id, name_offset_in_name_list)
        }
    }
    .collect::<Result<_>>()
    .await
}

/// parse the resource table, making a mapping from ResourceID to file table index
async fn parse_resource_table(reader: &mut Reader) -> Result<Vec<(ResourceID, u16)>> {
    let resource_entry_count = reader.read_u16().await?;

    try_stream! {
        for _ in 0..resource_entry_count {
            yield (reader.read_u16().await?, reader.read_u16().await? - 1)
        }
    }
    .collect::<Result<_>>()
    .await
}

async fn parse_file_table(
    reader: &mut Reader,
    expected_size: u16,
) -> Result<HashMap<FileID, File>> {
    let file_entry_count: u16 = reader
        .read_u32()
        .await?
        .try_into()
        .map_err(|_| Error::TooBigFileTable)?;

    const BYTES_PER_ENTRY: u16 = 4 + 4 + 2;
    if 4 + file_entry_count * BYTES_PER_ENTRY != expected_size {
        Err(Error::UncoherentFileTableSize)?
    }

    try_stream! {
        for file_id in 0..file_entry_count {
            let offset = reader.read_u32().await? as u64;

            let size_lower = reader.read_u16().await?;
            let size_upper = reader.read_u8().await?;
            let flag = reader.read_u8().await?;
            let size = ((size_upper as u32) << 16) | size_lower as u32;

            let unknown = reader.read_u16().await?;

            yield (
                file_id,
                File { offset, size, flag, unknown },
            )
        }
    }
    .collect::<reader::Result<Vec<_>>>()
    .await
    .map(|t| t.into_iter().collect())
    .map_err(Error::Reader)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::tests::get_known_files;

    async fn open(path: PathBuf) {
        Mohawk::open(&path).await.expect("to parse Mohawk file");
    }

    #[tokio::test]
    async fn open_known_files() {
        get_known_files().then(open).collect::<()>().await
    }
}
