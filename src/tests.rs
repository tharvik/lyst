use std::path::{Path, PathBuf};

use stream::StreamExt;
use tokio_stream::{self as stream, Stream};

static MYST_INSTALL_DIR: &str = "myst";

pub fn get_known_files() -> impl Stream<Item = PathBuf> {
    stream::iter([
        "CHANNEL.DAT",
        "CREDITS.DAT",
        "DUNNY.DAT",
        "INTRO.DAT",
        "MECHAN.DAT",
        "MYST.DAT",
        "SELEN.DAT",
        "STONE.DAT",
        "SYSTEM.DAT",
    ])
    .map(Path::new)
    .map(|p| Path::new(MYST_INSTALL_DIR).join(p))
}
