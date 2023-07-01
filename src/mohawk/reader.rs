use std::{
    fmt,
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

struct PositionedCommand {
    pos: u64,
    cmd: Commands,
}

enum Commands {
    FixedWitdh(FixedWitdhCommands),
    VariableWidth(VariableWidthCommands),
}

enum FixedWitdhCommands {
    ReadU8(oneshot::Sender<io::Result<u8>>),
    ReadU16(oneshot::Sender<io::Result<u16>>),
    ReadU32(oneshot::Sender<io::Result<u32>>),
    Read4Bytes(oneshot::Sender<io::Result<[u8; 4]>>),
}

enum VariableWidthCommands {
    // Widder Error as can also fail UTF-8 conv
    ReadString(oneshot::Sender<crate::Result<String>>),
    /// Low-level call to non-blockingly read some bytes
    ReadBuf {
        capacity: usize,
        resp: oneshot::Sender<io::Result<Vec<u8>>>,
    },
}

impl FixedWitdhCommands {
    const fn width(&self) -> usize {
        match self {
            Self::ReadU8(_) => 1,
            Self::ReadU16(_) => 2,
            Self::ReadU32(_) => 4,
            Self::Read4Bytes(_) => 4,
        }
    }
}

impl From<FixedWitdhCommands> for Commands {
    fn from(value: FixedWitdhCommands) -> Self {
        Self::FixedWitdh(value)
    }
}

impl From<VariableWidthCommands> for Commands {
    fn from(value: VariableWidthCommands) -> Self {
        Self::VariableWidth(value)
    }
}

impl fmt::Display for FixedWitdhCommands {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadU8(_) => f.write_str("ReadU8"),
            Self::ReadU16(_) => f.write_str("ReadU16"),
            Self::ReadU32(_) => f.write_str("ReadU32"),
            Self::Read4Bytes(_) => f.write_str("Read4Bytes"),
        }
    }
}

impl fmt::Display for VariableWidthCommands {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadString(_) => f.write_str("ReadString"),
            Self::ReadBuf { .. } => f.write_str("ReadBuf"),
        }
    }
}

impl fmt::Display for Commands {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FixedWitdh(cmd) => f.write_fmt(format_args!("{}", cmd)),
            Self::VariableWidth(cmd) => f.write_fmt(format_args!("{}", cmd)),
        }
    }
}

// small undef behavior when reading via AsyncRead and other reads concurrently
#[pin_project::pin_project]
pub struct MohawkReader {
    agent: mpsc::Sender<PositionedCommand>,
    pos: u64,
    #[pin]
    reader: Option<Pin<Box<dyn Future<Output = io::Result<Vec<u8>>>>>>,
}

impl MohawkReader {
    pub async fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        trace!("open {}", path.as_ref().display());

        Ok(Self {
            agent: Handler::open(path).await?.spawn(),
            pos: 0,
            reader: None,
            // state: State::Idle,
        })
    }

    async fn send<T, F>(&self, cmd_type: F) -> T
    where
        F: FnOnce(oneshot::Sender<T>) -> Commands,
    {
        let (tx, rx) = oneshot::channel();

        let cmd = cmd_type(tx);

        self.agent
            .send(PositionedCommand { pos: self.pos, cmd })
            .await
            .ok();

        rx.await.unwrap()
    }

    async fn send_fixed_width<T, F>(&mut self, cmd_type: F) -> io::Result<T>
    where
        F: Fn(oneshot::Sender<io::Result<T>>) -> FixedWitdhCommands,
    {
        let mut width = 0;

        let ret = self
            .send(|tx| {
                let cmd = cmd_type(tx);
                width = cmd.width();
                Commands::FixedWitdh(cmd)
            })
            .await?;
        self.pos += width as u64;

        Ok(ret)
    }

    pub(super) async fn read_u8(&mut self) -> io::Result<u8> {
        self.send_fixed_width(FixedWitdhCommands::ReadU8).await
    }

    pub(super) async fn read_u16(&mut self) -> io::Result<u16> {
        self.send_fixed_width(FixedWitdhCommands::ReadU16).await
    }

    pub(super) async fn read_u32(&mut self) -> io::Result<u32> {
        self.send_fixed_width(FixedWitdhCommands::ReadU32).await
    }

    pub(super) async fn read_4_bytes(&mut self) -> io::Result<[u8; 4]> {
        self.send_fixed_width(FixedWitdhCommands::Read4Bytes).await
    }

    pub(super) async fn read_string(&mut self) -> crate::Result<String> {
        let ret = self
            .send(|tx| Commands::VariableWidth(VariableWidthCommands::ReadString(tx)))
            .await?;
        self.pos += u64::try_from(ret.len()).expect("not to add so much");
        Ok(ret)
    }

    async fn read_buf(
        agent: mpsc::Sender<PositionedCommand>,
        capacity: usize,
        pos: u64,
    ) -> io::Result<Vec<u8>> {
        let (tx, rx) = oneshot::channel();
        let cmd = Commands::VariableWidth(VariableWidthCommands::ReadBuf { capacity, resp: tx });

        agent.send(PositionedCommand { pos, cmd }).await.ok();

        rx.await.unwrap()
    }
}

impl io::AsyncRead for MohawkReader {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let mut this = self.project();

        if this.reader.is_none() {
            *this.reader = Some(Box::pin(Self::read_buf(
                this.agent.clone(),
                buf.remaining(),
                *this.pos,
            )));
        };

        let recv = this.reader.iter_mut().next().unwrap();
        let got = ready!(Future::poll(recv.as_mut(), cx));

        Poll::Ready(match got {
            Ok(read) => {
                buf.put_slice(&read);
                *this.pos += u64::try_from(read.len()).expect("not to add so much");
                Ok(())
            }
            Err(e) => Err(e),
        })
    }
}

// we don't actually send anything to the handler as seeking is done on every request
impl io::AsyncSeek for MohawkReader {
    fn start_seek(self: Pin<&mut Self>, position: SeekFrom) -> std::io::Result<()> {
        match position {
            SeekFrom::Start(cur) => self.get_mut().pos = cur,
            _ => todo!("impl other seeks"),
        }

        Ok(())
    }

    fn poll_complete(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<u64>> {
        Poll::Ready(Ok(self.pos))
    }
}

impl Clone for MohawkReader {
    fn clone(&self) -> Self {
        Self {
            agent: self.agent.clone(),
            pos: self.pos,
            reader: None, // will create own reader on first poll
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
            Ok(())
        } else {
            trace!("seeking to {}", seek_to);
            let ret = self.reader.seek(SeekFrom::Start(seek_to)).await.map(|_| ());
            self.pos = seek_to;
            ret
        }
    }

    fn spawn(mut self) -> mpsc::Sender<PositionedCommand> {
        let (tx, mut rx) = mpsc::channel(10);

        tokio::spawn(async move {
            while let Some(PositionedCommand { pos: seek_to, cmd }) = rx.recv().await {
                use Commands::*;
                use FixedWitdhCommands::*;
                use VariableWidthCommands::*;

                match cmd {
                    FixedWitdh(cmd) => {
                        let width = cmd.width() as u64;
                        match cmd {
                            ReadU8(resp) => resp
                                .send(
                                    async {
                                        self.seek(seek_to).await?;
                                        self.reader.read_u8().await
                                    }
                                    .await,
                                )
                                .unwrap(),
                            ReadU16(resp) => resp
                                .send(
                                    async {
                                        self.seek(seek_to).await?;
                                        self.reader.read_u16().await
                                    }
                                    .await,
                                )
                                .unwrap(),
                            ReadU32(resp) => resp
                                .send(
                                    async {
                                        self.seek(seek_to).await?;
                                        self.reader.read_u32().await
                                    }
                                    .await,
                                )
                                .unwrap(),
                            Read4Bytes(resp) => resp
                                .send(
                                    async {
                                        self.seek(seek_to).await?;
                                        let mut buffer = [0u8; 4];
                                        self.reader.read_exact(&mut buffer).await?;
                                        Ok(buffer)
                                    }
                                    .await,
                                )
                                .unwrap(),
                        };
                        self.pos += width;
                    }
                    VariableWidth(ReadString(resp)) => resp
                        .send(
                            async {
                                self.seek(seek_to).await?;
                                let mut c_string = vec![];
                                self.reader.read_until(0u8, &mut c_string).await?;
                                c_string.remove(c_string.len() - 1);

                                let ret = String::from_utf8(c_string)?;

                                self.pos += (ret.len() as u64) + 1;
                                Ok(ret)
                            }
                            .await,
                        )
                        .unwrap(),
                    VariableWidth(ReadBuf { capacity, resp }) => resp
                        .send(
                            async {
                                self.seek(seek_to).await?;
                                let mut buf = vec![0; capacity];
                                self.reader.read_buf(&mut buf).await?;
                                self.pos += buf.len() as u64;
                                Ok(buf)
                            }
                            .await,
                        )
                        .unwrap(),
                };
            }
        });

        tx
    }
}
