use alloc::{boxed::Box, string::String, sync::Arc};
use core::str;

use async_trait::async_trait;
use futures_util::{stream, Stream};
use ksc_core::{Error, EINTR, EIO};

use crate::{IntoAny, IoSlice, IoSliceMut, SeekFrom};

#[async_trait]
pub trait Io: ToIo + IntoAny {
    async fn read(&self, buffer: &mut [IoSliceMut]) -> Result<usize, Error> {
        let offset = self.seek(SeekFrom::Current(0)).await?;
        let read_len = self.read_at(offset, buffer).await?;
        self.seek(SeekFrom::Current(read_len as isize)).await?;
        Ok(read_len)
    }

    async fn write(&self, buffer: &mut [IoSlice]) -> Result<usize, Error> {
        let offset = self.seek(SeekFrom::Current(0)).await?;
        let written_len = self.write_at(offset, buffer).await?;
        self.seek(SeekFrom::Current(written_len as isize)).await?;
        Ok(written_len)
    }

    async fn seek(&self, whence: SeekFrom) -> Result<usize, Error>;

    async fn stream_len(&self) -> Result<usize, Error> {
        let old = self.seek(SeekFrom::Current(0)).await?;
        let ret = self.seek(SeekFrom::End(0)).await?;
        self.seek(SeekFrom::Start(old)).await?;
        Ok(ret)
    }

    async fn read_at(&self, offset: usize, buffer: &mut [IoSliceMut]) -> Result<usize, Error>;

    async fn write_at(&self, offset: usize, buffer: &mut [IoSlice]) -> Result<usize, Error>;

    async fn flush(&self) -> Result<(), Error>;
}

pub trait IoExt: Io {
    async fn current_pos(&self) -> Result<usize, Error> {
        self.seek(SeekFrom::Current(0)).await
    }

    async fn read_exact_at(&self, mut offset: usize, mut buffer: &mut [u8]) -> Result<(), Error> {
        while !buffer.is_empty() {
            match self.read_at(offset, &mut [buffer]).await {
                Ok(0) => break,
                Ok(n) => {
                    offset += n;
                    buffer = &mut buffer[n..];
                }
                Err(EINTR) => {}
                Err(e) => return Err(e),
            }
        }
        if buffer.is_empty() {
            Ok(())
        } else {
            log::trace!("unexpected EOF");
            Err(EIO)
        }
    }

    async fn read_exact(&self, mut buffer: &mut [u8]) -> Result<(), Error> {
        while !buffer.is_empty() {
            match self.read(&mut [buffer]).await {
                Ok(0) => break,
                Ok(n) => buffer = &mut buffer[n..],
                Err(EINTR) => {}
                Err(e) => return Err(e),
            }
        }
        if buffer.is_empty() {
            Ok(())
        } else {
            log::trace!("unexpected EOF");
            Err(EIO)
        }
    }

    async fn write_all_at(&self, mut offset: usize, mut buffer: &[u8]) -> Result<(), Error> {
        while !buffer.is_empty() {
            match self.write_at(offset, &mut [buffer]).await {
                Ok(0) => break,
                Ok(n) => {
                    offset += n;
                    buffer = &buffer[n..]
                }
                Err(EINTR) => {}
                Err(e) => return Err(e),
            }
        }
        if buffer.is_empty() {
            Ok(())
        } else {
            log::trace!("write zero");
            Err(EIO)
        }
    }

    async fn write_all(&self, mut buffer: &[u8]) -> Result<(), Error> {
        while !buffer.is_empty() {
            match self.write(&mut [buffer]).await {
                Ok(0) => break,
                Ok(n) => buffer = &buffer[n..],
                Err(EINTR) => {}
                Err(e) => return Err(e),
            }
        }
        if buffer.is_empty() {
            Ok(())
        } else {
            log::trace!("write zero");
            Err(EIO)
        }
    }

    async fn read_line(&self, out: &mut String) -> Result<Option<String>, Error> {
        let mut buf = [0; 64];
        loop {
            if let Some(pos) = out.find('\n') {
                let next = out.split_off(pos + 1);
                out.pop();
                return Ok(Some(next));
            }

            let len = self.read(&mut [&mut buf]).await?;
            if len == 0 {
                return Ok(None);
            }

            let s = str::from_utf8(&buf[..len])?;
            out.push_str(s);
        }
    }
}
impl<T: Io + ?Sized> IoExt for T {}

pub fn lines(io: Arc<dyn Io>) -> impl Stream<Item = Result<String, Error>> + Send {
    stream::unfold((io, Some(String::new())), |(io, buf)| async move {
        let mut buf = buf?;
        let next_buf = io.read_line(&mut buf).await;
        Some(match next_buf {
            Ok(next) => (Ok(buf), (io, next)),
            Err(err) => (Err(err), (io, None)),
        })
    })
}

pub trait ToIo {
    fn to_io(self: Arc<Self>) -> Option<Arc<dyn Io>> {
        None
    }
}

impl<T: Io> ToIo for T {
    fn to_io(self: Arc<Self>) -> Option<Arc<dyn Io>> {
        Some(self as _)
    }
}

/// Used in implementations of `read_at` by files where random access is not
/// supported.
pub async fn read_at_by_seek<T: Io>(
    io: &T,
    offset: usize,
    buffer: &mut [IoSliceMut<'_>],
) -> Result<usize, Error> {
    let old = io.seek(SeekFrom::Current(0)).await?;
    io.seek(SeekFrom::Start(offset)).await?;
    let res = io.read(buffer).await;
    let _ = io.seek(SeekFrom::Start(old)).await;
    res
}

/// Used in implementations of `write_at` by files where random access is not
/// supported.
pub async fn write_at_by_seek<T: Io>(
    io: &T,
    offset: usize,
    buffer: &mut [IoSlice<'_>],
) -> Result<usize, Error> {
    let old = io.seek(SeekFrom::Current(0)).await?;
    io.seek(SeekFrom::Start(offset)).await?;
    let res = io.write(buffer).await;
    let _ = io.seek(SeekFrom::Start(old)).await;
    res
}
