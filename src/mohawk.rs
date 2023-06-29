// TODO mod msnd;

use std::{
    collections::HashMap,
    future::Future,
    io::SeekFrom,
    ops::DerefMut,
    path::{Path, PathBuf},
    pin::{pin, Pin},
    sync::{Arc, Mutex},
    task::{ready, Context, Poll},
};
use tracing::{trace, trace_span, warn};

use async_stream::try_stream;
use tokio::{
    fs,
    io::{self, AsyncBufReadExt, AsyncReadExt, AsyncSeekExt},
};
use tokio_stream::{Stream, StreamExt};

use crate::{errors::*, Result};

pub struct MohawkReader {
    reader: Arc<Mutex<io::BufReader<fs::File>>>,
    path: PathBuf,
}

impl MohawkReader {
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        let mut cloned = PathBuf::new();
        cloned.push(path);

        trace!("open {}", cloned.display());

        Ok(Self {
            reader: fs::File::open(&cloned)
                .await
                .map(io::BufReader::new)
                .map(Mutex::new)
                .map(Arc::new)?,
            path: cloned,
        })
    }

    /// Reopen file to allow thread-independant actions.
    ///
    /// Do not seek to current pos
    async fn reopen(&self) -> Result<Self> {
        Self::open(&self.path).await
    }

    async fn read_4_bytes(&mut self) -> Result<[u8; 4]> {
        let mut buffer = [0u8; 4];
        self.read_exact(&mut buffer).await?;
        Ok(buffer)
    }

    async fn read_string(&mut self) -> Result<String> {
        let mut reader = self.reader.lock().unwrap();

        let mut resource_name_bytes = vec![];
        reader.read_until(0u8, &mut resource_name_bytes).await?;
        resource_name_bytes.remove(resource_name_bytes.len() - 1);

        String::from_utf8(resource_name_bytes).map_err(Error::UTF8)
    }
}

impl io::AsyncRead for MohawkReader {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let mut locked = self.reader.lock().unwrap();
        let pinned_reader: Pin<&mut io::BufReader<fs::File>> = Pin::new(locked.deref_mut());
        pinned_reader.poll_read(cx, buf)
    }
}

impl io::AsyncSeek for MohawkReader {
    fn start_seek(self: Pin<&mut Self>, position: io::SeekFrom) -> std::io::Result<()> {
        Pin::new(self.reader.lock().unwrap().deref_mut()).start_seek(position)
    }

    fn poll_complete(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<u64>> {
        Pin::new(self.reader.lock().unwrap().deref_mut()).poll_complete(cx)
    }
}

impl Stream for MohawkReader {
    type Item = Result<u8>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let got = ready!(pin!(pin!(self.reader.lock().unwrap().deref_mut()).read_u8()).poll(cx));
        Poll::Ready(match got {
            Ok(b) => Some(Ok(b)),
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => None,
            Err(e) => Some(Err(Error::IO(e))),
        })
    }
}

pub struct Resource {
    pub name: Option<String>,
    pub file: File,
}

pub struct File {
    offset: u64,
    pub size: usize,
    pub flag: u8,
    pub unknown: u16,
}

impl File {
    pub async fn with_reader(&self, reader: &mut MohawkReader) -> Result<Vec<u8>> {
        trace!("read {} bytes at offset {}", self.size, self.offset);

        reader.seek(SeekFrom::Start(self.offset)).await?;

        let mut raw = vec![0; self.size];
        reader.read_exact(&mut raw).await?;

        Ok(raw)
    }
}

impl Resource {
    pub async fn with_reader(&self, reader: &mut MohawkReader) -> Result<Vec<u8>> {
        self.file.with_reader(reader).await
    }
}

pub type TypeID = [u8; 4];
pub type ResourceID = u16;

type FileID = u16;

// http://insidethelink.ortiche.net/wiki/index.php/Mohawk_archive_format
pub struct Mohawk {
    //pub msnd: Option<HashMap<ResourceID, Resource>>,
    pub types: HashMap<TypeID, HashMap<ResourceID, Resource>>,
}

impl Mohawk {
    pub async fn with_reader(mut reader: &mut MohawkReader) -> Result<Self> {
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
        let type_tables = parse_type_table(&mut reader).await?;
        trace!(
            name_list_offset_in_resource_dir,
            "types table parsed: {} found",
            type_tables.len()
        );

        reader
            .seek(SeekFrom::Start(
                resource_dir_offset as u64 + file_table_offset_in_resource_dir as u64,
            ))
            .await?;
        let mut files = parse_file_table(&mut reader, file_table_size).await?;

        let types_tables = try_stream! {
            for (resource_type, entry) in type_tables {
                let _span_ = trace_span!(
                    "parse type",
                    "{}{}{}{}",
                    resource_type[0],
                    resource_type[1],
                    resource_type[2],
                    resource_type[3]
                )
                .entered();

                reader
                    .seek(SeekFrom::Start(
                        resource_dir_offset as u64 + entry.resource_table_offset_in_resource_dir as u64,
                    ))
                    .await?;
                let resource_table = parse_resource_table(&mut reader).await?;

                reader
                    .seek(SeekFrom::Start(
                        resource_dir_offset as u64 + entry.name_table_offset_in_resource_dir as u64,
                    ))
                    .await?;
                let name_table = parse_name_table(&mut reader).await?;

                let mut reader = reader.reopen().await?;
                let resource_id_to_name: HashMap<ResourceID, String> = try_stream! {
                    for (resource_id, name_offset_in_name_list) in name_table {
                        reader
                            .seek(SeekFrom::Start(
                                resource_dir_offset as u64
                                    + name_list_offset_in_resource_dir as u64
                                    + name_offset_in_name_list as u64,
                            ))
                            .await?;
                        let name = reader.read_string().await?;
                        yield (resource_id, name)
                    }
                }
                .collect::<Result<Vec<_>>>()
                .await?
                .into_iter()
                .collect();

                yield (resource_type, resource_table, resource_id_to_name)
            }
        }
        .collect::<Result<Vec<_>>>()
        .await?;

        let types = types_tables
            .into_iter()
            .map(|(resource_type, resource_table, mut resource_id_to_name)| {
                Ok((
                    resource_type,
                    resource_table
                        .into_iter()
                        .map(|(resource_id, file_id)| {
                            Ok((
                                resource_id,
                                Resource {
                                    name: resource_id_to_name.remove(&file_id),
                                    file: files
                                        .remove(&file_id)
                                        .ok_or(InvalidHeaderError::UnknownFileID)?,
                                },
                            ))
                        })
                        .collect::<Result<_>>()?,
                    resource_id_to_name,
                ))
            })
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            // TODO unefficient check
            .map(|(resource_type, resources, resource_id_to_name)| {
                if !resource_id_to_name.is_empty() {
                    warn!("{} names unmatched to resources", resource_id_to_name.len())
                }
                (resource_type, resources)
            })
            .collect::<HashMap<TypeID, HashMap<_, _>>>();

        if !files.is_empty() {
            warn!("{} files unmatched to resources", files.len())
        }

        Ok(Self {
            //msnd: types.get(b"MSND").map(|m| m.clone()),
            types,
        })
    }
}

/// parse IFF header and return total file size
async fn parse_iff_header(reader: &mut MohawkReader) -> Result<u32> {
    if reader.read_4_bytes().await? != *b"MHWK" {
        Err(InvalidHeaderError::IFFSignature)?;
    }

    reader
        .read_u32()
        .await
        .map(|size| size + 8)
        .map_err(Error::IO)
}

struct RSRCHeader {
    pub resource_dir_offset: u32,
    pub file_table_offset_in_resource_dir: u16,
    pub file_table_size: u16,
}

/// parse RSRC header and return its content
async fn parse_rsrc_header(reader: &mut MohawkReader, total_file_size: u32) -> Result<RSRCHeader> {
    if reader.read_4_bytes().await? != *b"RSRC" {
        Err(InvalidHeaderError::RSRCSignature)?;
    }
    let version = reader.read_u16().await?;
    if version != 0x100 {
        Err(InvalidHeaderError::UnsupportedVersion(version))?;
    }
    let compaction = reader.read_u16().await?;
    if compaction != 0x1 {
        Err(InvalidHeaderError::UnsupportedCompaction(compaction))?;
    }
    if reader.read_u32().await? != total_file_size {
        Err(InvalidHeaderError::UncoherentFileSize)?;
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

async fn parse_type_table(reader: &mut MohawkReader) -> Result<Vec<(TypeID, TypeTableEntry)>> {
    let type_entry_count = reader.read_u16().await?;
    try_stream! {
        for _ in 0..type_entry_count {
            yield (
                reader.read_4_bytes().await?,
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

async fn parse_name_table(reader: &mut MohawkReader) -> Result<Vec<(ResourceID, u16)>> {
    let names_count = reader.read_u16().await?;
    trace!("got {} names", names_count);

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
async fn parse_resource_table(reader: &mut MohawkReader) -> Result<Vec<(ResourceID, u16)>> {
    let resource_entry_count = reader.read_u16().await?;
    trace!("got {} resources", resource_entry_count);

    try_stream! {
        for _ in 0..resource_entry_count {
            yield (reader.read_u16().await?, reader.read_u16().await? - 1)
        }
    }
    .collect::<Result<_>>()
    .await
}

async fn parse_file_table(
    reader: &mut MohawkReader,
    expected_size: u16,
) -> Result<HashMap<FileID, File>> {
    let file_entry_count: u16 = reader
        .read_u32()
        .await?
        .try_into()
        .map_err(|_| InvalidHeaderError::TooBigFileTable)?;

    const BYTES_PER_ENTRY: u16 = 4 + 4 + 2;
    if 4 + file_entry_count * BYTES_PER_ENTRY != expected_size {
        Err(InvalidHeaderError::UncoherentFileTableSize)?
    }

    try_stream! {
        for file_id in 0..file_entry_count {
            let offset = reader.read_u32().await? as u64;

            let size_and_flag = reader.read_u32().await?;
            let size = (size_and_flag >> 8) as usize;
            let flag = size_and_flag.to_be_bytes()[3];

            let unknown = reader.read_u16().await?;

            yield (
                file_id,
                File { offset, size, flag, unknown },
            )
        }
    }
    .collect::<Result<Vec<_>>>()
    .await
    .map(|t| t.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    static MYST_INSTALL_DIR: &str = "Myst Masterpiece Edition";

    async fn test_known_file(filename: &str) {
        let path = Path::new(MYST_INSTALL_DIR).join(filename);

        let mut reader = MohawkReader::open(path).await.expect("to open Mohawk file");
        Mohawk::with_reader(&mut reader)
            .await
            .expect("to parse Mohawk file")
            .types
            .iter()
            .for_each(|(type_id, resources)| {
                println!(
                    "type {}{}{}{}",
                    type_id[0], type_id[1], type_id[2], type_id[3]
                );
                resources.iter().for_each(|(resource_id, resource)| {
                    if let Some(name) = &resource.name {
                        println!("  resource {:?}", name);
                    }
                })
            });
    }

    #[tokio::test]
    async fn test_know_file_myst() {
        test_known_file("MYST.DAT").await
    }
}
