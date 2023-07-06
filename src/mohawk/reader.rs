use std::{
    cmp, fmt,
    future::Future,
    io::SeekFrom,
    path::Path,
    pin::{pin, Pin},
    task::{ready, Context, Poll},
};
use tracing::{trace, trace_span, warn, Instrument};

use tokio::{
    fs,
    io::{self, AsyncBufReadExt, AsyncReadExt, AsyncSeekExt},
    sync::{mpsc, oneshot},
};

pub type Error = io::Error;
pub type Result<T> = std::result::Result<T, Error>;

struct Command {
    pos: u64,
    cmd: Commands,
}

enum Commands {
    ReadBuf {
        resp: oneshot::Sender<io::Result<Vec<u8>>>,
    },
}

impl fmt::Display for Command {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_fmt(format_args!("{}", self.cmd))
    }
}

impl fmt::Display for Commands {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Commands::ReadBuf { .. } => f.write_str("FillBuf"),
        }
    }
}

type BoxedFuture<T> = Pin<Box<dyn Future<Output = T>>>;

#[pin_project::pin_project]
pub struct Reader {
    agent: mpsc::Sender<Command>,
    pos: u64,
    remaining: Option<usize>,

    #[pin]
    fill_buf: Option<BoxedFuture<io::Result<Vec<u8>>>>,
    buffer: Vec<u8>,
}

impl Reader {
    // Open the given file
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        trace!("open {}", path.as_ref().display());

        Ok(Self {
            agent: Handler::open(path).await?.spawn(),
            pos: 0,
            remaining: None,

            fill_buf: None,
            buffer: Vec::new(),
        })
    }

    // differ from AsyncReadExt by cloning as needed
    pub fn take(&self, size: usize) -> Self {
        trace!("take {}", size);

        Self {
            remaining: Some(
                self.remaining
                    .map(|old| cmp::min(old, size))
                    .unwrap_or(size),
            ),
            ..self.clone()
        }
    }

    pub async fn read_4_bytes(&mut self) -> Result<[u8; 4]> {
        let mut buffer = [0u8; 4];
        self.read_exact(&mut buffer).await?;

        Ok(buffer)
    }

    // helpers

    async fn fill_buf(agent: mpsc::Sender<Command>, pos: u64) -> io::Result<Vec<u8>> {
        let (tx, rx) = oneshot::channel();

        agent
            .send(Command {
                pos,
                cmd: Commands::ReadBuf { resp: tx },
            })
            .await
            .ok();

        rx.await.unwrap()
    }

    fn poll_fill_buf_inner<'a>(
        self: &mut Pin<&'a mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        if !self.as_mut().project().buffer.is_empty() {
            return Poll::Ready(Ok(()));
        }

        if self.remaining == Some(0) {
            // no update to buffer == EOF
            return Poll::Ready(Ok(()));
        }

        let mut this = self.as_mut().project();

        if this.fill_buf.is_none() {
            trace!(
                "buffer empty, new request :: remaining={:?}",
                this.remaining
            );
            *this.fill_buf = Some(Box::pin(Self::fill_buf(this.agent.clone(), *this.pos)));
        }

        let fut = this.fill_buf.iter_mut().next().unwrap();
        let got = ready!(fut.as_mut().poll(cx));
        *this.fill_buf = None;

        Poll::Ready(got.map(|mut v| {
            trace!("got buffer, keeping it");
            if let Some(rem) = this.remaining {
                v.truncate(*rem);
            }
            *this.buffer = v;
        }))
    }
}

impl io::AsyncRead for Reader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut io::ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let _span_ = trace_span!("read").entered();

        self.poll_fill_buf_inner(cx).map_ok(|_| {
            let size = cmp::min(buf.remaining(), self.buffer.len());
            buf.put_slice(self.buffer.get(0..size).unwrap());
            io::AsyncBufRead::consume(self, size);
        })
    }
}

impl io::AsyncBufRead for Reader {
    fn poll_fill_buf(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<&[u8]>> {
        let _span_ = trace_span!("fill buf").entered();

        self.poll_fill_buf_inner(cx)
            .map_ok(|_| self.get_mut().buffer.as_slice())
    }

    fn consume(self: Pin<&mut Self>, amt: usize) {
        let _span_ = trace_span!("consume", amount = amt).entered();

        let this = self.project();

        *this.buffer = this.buffer.split_off(amt);
        *this.pos += u64::try_from(amt).expect("not to add so much");
        *this.remaining = this.remaining.map(|rem| rem - amt);
    }
}

// we don't actually send anything to the handler as seeking is done on every request
impl io::AsyncSeek for Reader {
    fn start_seek(mut self: Pin<&mut Self>, seek_to: SeekFrom) -> io::Result<()> {
        let is_within_buffer = |cur| cur >= self.pos && cur - self.pos < self.buffer.len() as u64;

        match seek_to {
            SeekFrom::Current(0) => {}
            SeekFrom::Start(cur) if is_within_buffer(cur) => {
                let skip = (cur - self.pos) as usize;
                io::AsyncBufRead::consume(self.as_mut(), skip);
            }
            SeekFrom::Start(cur) => {
                self.as_mut().fill_buf = None;
                self.as_mut().buffer = Vec::new();

                self.pos = cur;
            }

            SeekFrom::Current(off) if off >= 0 && is_within_buffer(self.pos + off as u64) => {
                io::AsyncBufRead::consume(self.as_mut(), off as usize);
            }
            _ => todo!("impl other seeks: {:?}", seek_to),
        }

        Ok(())
    }

    fn poll_complete(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<u64>> {
        Poll::Ready(Ok(self.pos))
    }
}

impl Clone for Reader {
    fn clone(&self) -> Self {
        Self {
            agent: self.agent.clone(),
            pos: self.pos,
            remaining: self.remaining,

            fill_buf: None,
            buffer: Vec::new(),
        }
    }
}

struct Handler {
    reader: io::BufReader<fs::File>,
    pos: u64,
}

impl Handler {
    async fn open(path: impl AsRef<Path>) -> Result<Self> {
        Ok(Self {
            reader: fs::File::open(&path).await.map(io::BufReader::new)?,
            pos: 0,
        })
    }

    async fn seek(&mut self, seek_to: u64) -> io::Result<()> {
        let is_within_buffer =
            seek_to > self.pos && seek_to - self.pos < self.reader.buffer().len() as u64;
        let ret = if is_within_buffer {
            trace!("seek fast forward!");

            let skip = (seek_to - self.pos) as usize;
            self.reader.consume(skip);

            Ok(())
        } else {
            trace!("seeking to 0x{:08x}", seek_to);
            self.reader.seek(SeekFrom::Start(seek_to)).await.map(|_| ())
        };

        self.pos = seek_to;
        ret
    }

    async fn fill_buf(&mut self, at_pos: u64) -> io::Result<Vec<u8>> {
        self.seek(at_pos).await?;
        let buf = Vec::from(self.reader.fill_buf().await?);
        trace!("got {} bytes", buf.len());
        Ok(Vec::from(buf))
    }

    fn spawn(mut self) -> mpsc::Sender<Command> {
        let (tx, mut rx) = mpsc::channel(10);

        tokio::spawn(
            async move {
                while let Some(Command { pos, cmd }) = rx.recv().await {
                    trace!("exec {}", cmd);
                    match cmd {
                        Commands::ReadBuf { resp } => {
                            if resp.send(self.fill_buf(pos).await).is_err() {
                                warn!("receiver gone");
                            }
                        }
                    };
                }
            }
            .instrument(trace_span!("handler")),
        );

        tx
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use tokio::io::AsyncReadExt;
    use tokio_stream::StreamExt;

    use super::Reader;
    use crate::tests::get_known_files;

    async fn take_more(path: impl AsRef<Path>) {
        let reader = Reader::open(path).await.expect("to open path").take(0);

        let mut buf = [0u8; 1];
        assert_eq!(
            reader
                .take(1)
                .read(&mut buf)
                .await
                .expect("to notice EOF gracefully"),
            0,
            "not end of file but reader should be finished"
        )
    }

    #[tokio::test]
    async fn take_more_than_already_taken_returns_smallest() {
        get_known_files().then(take_more).collect::<()>().await
    }
}
