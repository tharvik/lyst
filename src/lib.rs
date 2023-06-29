pub mod mohawk;
pub use mohawk::{Mohawk, MohawkReader};

pub mod errors {
    #[derive(thiserror::Error, Debug)]
    pub enum Error {
        #[error("reading file: {0}")]
        IO(#[from] std::io::Error),
        #[error("parse string: {0}")]
        UTF8(#[from] std::string::FromUtf8Error),
        #[error("invalid header: {0}")]
        InvalidHeader(#[from] InvalidHeaderError),
    }

    #[derive(thiserror::Error, Debug)]
    pub enum InvalidHeaderError {
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
        #[error("file id reference but not found in table")]
        UnknownFileID,
        #[error("too big file table")]
        TooBigFileTable,
        #[error("uncoherent file table size")]
        UncoherentFileTableSize,
    }
}
pub use errors::Error;
pub type Result<T> = std::result::Result<T, Error>;
