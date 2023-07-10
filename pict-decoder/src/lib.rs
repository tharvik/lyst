use std::{io, string};

mod operation;
mod pict;
mod pixmap;
mod point;
mod quicktime;
mod rectangle;
mod utils;

pub use pict::PICT;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error(transparent)]
    Reader(#[from] io::Error),

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
    #[error("picture parsed but nothing found of value")]
    UnableToFindImage,
}

pub type Result<T> = std::result::Result<T, Error>;
