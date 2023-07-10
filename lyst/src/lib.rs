pub mod mohawk;
pub use mohawk::Mohawk;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("parsing mohawk: {0}")]
    Mohawk(#[from] mohawk::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests;
