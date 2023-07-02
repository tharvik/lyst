use std::{
    cmp, fmt,
    future::Future,
    io::SeekFrom,
    path::Path,
    pin::{pin, Pin},
    task::{ready, Context, Poll},
    vec,
};
use tracing::trace;

use tokio::{
    fs,
    io::{self, AsyncBufReadExt, AsyncReadExt, AsyncSeekExt},
    sync::{mpsc, oneshot},
};

struct Command {
    pos: u64,
    cmd: Commands,
}

// TODO flatten
enum Commands {
    ReadBuf {
        capacity: usize,
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
            Commands::ReadBuf { capacity, .. } => {
                f.write_fmt(format_args!("ReadBuf(capacity={})", capacity))
            }
        }
    }
}

type BoxedFuture<T> = Pin<Box<dyn Future<Output = T>>>;

const BUFFER_SIZE: usize = 1024;

#[pin_project::pin_project]
pub struct MohawkReader {
    agent: mpsc::Sender<Command>,
    pos: u64,

    #[pin]
    read_buf: Option<BoxedFuture<io::Result<Vec<u8>>>>,
    buffer: Vec<u8>,
}

impl MohawkReader {
    pub async fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        trace!("open {}", path.as_ref().display());

        Ok(Self {
            agent: Handler::open(path).await?.spawn(),
            pos: 0,

            read_buf: None,
            buffer: Vec::new(),
        })
    }

    pub async fn read_string(&mut self) -> crate::Result<String> {
        let mut c_string = vec![];
        self.read_until(0u8, &mut c_string).await?;
        c_string.remove(c_string.len() - 1);

        Ok(String::from_utf8(c_string)?)
    }

    pub async fn read_2_bytes(&mut self) -> io::Result<[u8; 2]> {
        let mut buffer = [0u8; 2];
        self.read_exact(&mut buffer).await?;

        Ok(buffer)
    }

    pub async fn read_4_bytes(&mut self) -> io::Result<[u8; 4]> {
        let mut buffer = [0u8; 4];
        self.read_exact(&mut buffer).await?;

        Ok(buffer)
    }

    async fn read_buf(
        agent: mpsc::Sender<Command>,
        pos: u64,
        capacity: usize,
    ) -> io::Result<Vec<u8>> {
        let (tx, rx) = oneshot::channel();

        agent
            .send(Command {
                pos,
                cmd: Commands::ReadBuf { capacity, resp: tx },
            })
            .await
            .ok();

        rx.await.unwrap()
    }

    fn poll_read_inner(
        self: &mut Pin<&mut Self>,
        cx: &mut Context<'_>,
        size: usize,
    ) -> Poll<io::Result<()>> {
        trace!("async read inner: size={}", size);

        let mut this = self.as_mut().project();
        if !this.buffer.is_empty() {
            trace!("async read: buffer not empty, using it direclty");
            return Poll::Ready(Ok(()));
        };

        if this.read_buf.is_none() {
            trace!("async read inner: #nofuture, building it");
            *this.read_buf = Some(Box::pin(Self::read_buf(
                this.agent.clone(),
                *this.pos,
                size,
            )));
        };

        let resp = this.read_buf.iter_mut().next().unwrap();
        let got = ready!(resp.as_mut().poll(cx));
        *this.read_buf = None;

        Poll::Ready(got.map(|read| {
            *this.buffer = read;
        }))
    }
}

impl io::AsyncRead for MohawkReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut io::ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        trace!("async read: size={}", buf.remaining());

        let got = ready!(self.poll_read_inner(cx, buf.remaining()));

        Poll::Ready(got.map(|_| {
            let size = cmp::min(buf.remaining(), self.buffer.len());
            buf.put_slice(self.buffer.get(0..size).unwrap());
            io::AsyncBufRead::consume(self, size);
        }))
    }
}

impl io::AsyncBufRead for MohawkReader {
    fn poll_fill_buf(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<&[u8]>> {
        trace!("buf read: fill buf");

        let got = ready!(self.poll_read_inner(cx, BUFFER_SIZE));

        Poll::Ready(got.map(|_| self.get_mut().buffer.as_slice()))
    }

    fn consume(self: Pin<&mut Self>, amt: usize) {
        trace!("buf read: consume {}", amt);

        let this = self.project();

        *this.buffer = this.buffer.split_off(amt);
        *this.pos += u64::try_from(amt).expect("not to add so much");
    }
}

// we don't actually send anything to the handler as seeking is done on every request
impl io::AsyncSeek for MohawkReader {
    fn start_seek(self: Pin<&mut Self>, position: SeekFrom) -> io::Result<()> {
        match position {
            SeekFrom::Start(cur) => self.get_mut().pos = cur,
            _ => todo!("impl other seeks"),
        }

        Ok(())
    }

    fn poll_complete(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<u64>> {
        Poll::Ready(Ok(self.pos))
    }
}

impl Clone for MohawkReader {
    fn clone(&self) -> Self {
        Self {
            agent: self.agent.clone(),
            pos: self.pos,

            read_buf: None,
            buffer: Vec::new(),
        }
    }
}

struct Handler {
    reader: io::BufReader<fs::File>,
    pos: u64,
}

impl Handler {
    async fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        Ok(Self {
            reader: fs::File::open(&path).await.map(io::BufReader::new)?,
            pos: 0,
        })
    }

    async fn seek(&mut self, seek_to: u64) -> io::Result<()> {
        if self.pos == seek_to {
            return Ok(());
        }

        trace!("seeking to {}", seek_to);
        let ret = self.reader.seek(SeekFrom::Start(seek_to)).await.map(|_| ());
        self.pos = seek_to;
        ret
    }

    fn spawn(mut self) -> mpsc::Sender<Command> {
        let (tx, mut rx) = mpsc::channel(10);

        tokio::spawn(async move {
            while let Some(Command { pos, cmd }) = rx.recv().await {
                match cmd {
                    Commands::ReadBuf { capacity, resp } => resp
                        .send(
                            async {
                                trace!("exec ReadBuf for {}", capacity);
                                self.seek(pos).await?;
                                let mut buf = Vec::with_capacity(capacity);
                                self.reader.read_buf(&mut buf).await?;
                                trace!("exec ReadBuf returns {}", buf.len());
                                self.pos += buf.len() as u64;
                                Ok(buf)
                            }
                            .await,
                        )
                        .unwrap(),
                }
            }
        });

        tx
    }
}
