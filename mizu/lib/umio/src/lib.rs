#![cfg_attr(not(test), no_std)]

use alloc::{boxed::Box, string::String, sync::Arc};
use core::{any::Any, fmt, mem, slice, str};

use arsc_rs::Arsc;
use async_trait::async_trait;
use futures_util::{stream, Stream};
use ksc_core::{Error, EINTR, EIO};

extern crate alloc;

#[derive(Copy, PartialEq, Eq, Clone, Debug)]
pub enum SeekFrom {
    /// Sets the offset to the provided number of bytes.
    Start(usize),

    /// Sets the offset to the size of this object plus the specified number of
    /// bytes.
    ///
    /// It is possible to seek beyond the end of an object, but it's an error to
    /// seek before byte 0.
    End(isize),

    /// Sets the offset to the current position plus the specified number of
    /// bytes.
    ///
    /// It is possible to seek beyond the end of an object, but it's an error to
    /// seek before byte 0.
    Current(isize),
}

pub type IoSlice<'a> = &'a [u8];

pub type IoSliceMut<'a> = &'a mut [u8];

#[allow(clippy::len_without_is_empty)]
pub trait IoSliceExt {
    fn len(&self) -> usize;

    fn advance(&mut self, n: usize);
}

impl IoSliceExt for IoSlice<'_> {
    fn len(&self) -> usize {
        (**self).len()
    }

    fn advance(&mut self, n: usize) {
        if self.len() < n {
            panic!("advancing IoSlice beyond its length");
        }

        *self = &self[n..];
    }
}

impl IoSliceExt for IoSliceMut<'_> {
    fn len(&self) -> usize {
        (**self).len()
    }

    fn advance(&mut self, n: usize) {
        if self.len() < n {
            panic!("advancing IoSlice beyond its length");
        }

        *self = unsafe { slice::from_raw_parts_mut(self.as_mut_ptr().add(n), self.len() - n) };
    }
}

pub fn ioslice_len(bufs: &&mut [impl IoSliceExt]) -> usize {
    bufs.iter().fold(0, |sum, buf| sum + buf.len())
}

pub fn ioslice_is_empty(bufs: &&mut [impl IoSliceExt]) -> bool {
    bufs.iter().all(|b| b.len() == 0)
}

#[track_caller]
pub fn advance_slices(bufs: &mut &mut [impl IoSliceExt], n: usize) {
    // Number of buffers to remove.
    let mut remove = 0;
    // Total length of all the to be removed buffers.
    let mut accumulated_len = 0;
    for buf in bufs.iter() {
        if accumulated_len + buf.len() > n {
            break;
        } else {
            accumulated_len += buf.len();
            remove += 1;
        }
    }

    *bufs = &mut mem::take(bufs)[remove..];
    if bufs.is_empty() {
        assert!(
            n == accumulated_len,
            "advancing io slices beyond their length, {n} == {accumulated_len}"
        );
    } else {
        bufs[0].advance(n - accumulated_len)
    }
}

pub struct FormatWriter<'a, 's, 'u>(pub &'a mut &'s mut [IoSliceMut<'u>], pub usize);

impl fmt::Write for FormatWriter<'_, '_, '_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let mut bytes = s.as_bytes();
        loop {
            if bytes.is_empty() {
                break;
            }
            let Some(first) = self.0.first_mut() else { break };

            let len = bytes.len().min(first.len());
            first[..len].copy_from_slice(&bytes[..len]);

            bytes = &bytes[len..];
            self.1 += len;
            advance_slices(&mut *self.0, len);
        }
        Ok(())
    }
}

pub trait IntoAny: Any + Send + Sync {
    fn into_any(self: Arc<Self>) -> Arc<dyn Any + Send + Sync>;

    fn into_any_arsc(self: Arsc<Self>) -> Arsc<dyn Any + Send + Sync>;
}

impl<T: Any + Send + Sync> IntoAny for T {
    fn into_any(self: Arc<Self>) -> Arc<dyn Any + Send + Sync> {
        self as _
    }

    fn into_any_arsc(self: Arsc<Self>) -> Arsc<dyn Any + Send + Sync> {
        self as _
    }
}

pub trait IntoAnyExt: IntoAny {
    fn downcast<T: Any + Send + Sync>(self: Arc<Self>) -> Option<Arc<T>> {
        self.into_any().downcast().ok()
    }

    fn downcast_arsc<T: Any + Send + Sync>(self: Arsc<Self>) -> Option<Arsc<T>> {
        self.into_any_arsc().downcast().ok()
    }
}

impl<T: IntoAny + ?Sized> IntoAnyExt for T {}

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

#[async_trait]
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
