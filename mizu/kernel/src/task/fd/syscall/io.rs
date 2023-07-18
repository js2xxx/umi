use alloc::{boxed::Box, vec, vec::Vec};
use core::{mem, time::Duration};

use co_trap::UserCx;
use futures_util::{
    stream::{self, FuturesUnordered},
    FutureExt, StreamExt, TryStreamExt,
};
use ksc::{
    async_handler,
    Error::{self, *},
};
use ktime::TimeOutExt;
use rv39_paging::{Attr, PAGE_SIZE};
use sygnal::SigSet;
use umio::SeekFrom;

use crate::{
    mem::{In, InOut, UserBuffer, UserPtr},
    syscall::{ffi::Ts, ScRet},
    task::{fd::Files, yield_now, TaskState},
};

#[async_handler]
pub async fn read(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(i32, UserBuffer, usize) -> Result<usize, Error>>,
) -> ScRet {
    let (fd, buffer, len) = cx.args();
    let fut = async move {
        if len == 0 {
            return Ok(0);
        }
        // log::trace!("user read fd = {fd}, buffer len = {len}");
        let mut guard = ts.virt.start_commit(Attr::WRITABLE).await;
        buffer.commit(&mut guard, len).await?;

        let entry = ts.files.get(fd).await?;
        let io = entry.to_io().ok_or(EBADF)?;

        io.read(guard.as_mut_slice()).await
    };
    cx.ret(fut.await);
    ScRet::Continue(None)
}

#[async_handler]
pub async fn write(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(i32, UserBuffer, usize) -> Result<usize, Error>>,
) -> ScRet {
    let (fd, buffer, len) = cx.args();
    let fut = async move {
        if len == 0 {
            return Ok(0);
        }
        // log::trace!("user write fd = {fd}, buffer len = {len}");
        let mut guard = ts.virt.start_commit(Attr::READABLE).await;
        buffer.commit(&mut guard, len).await?;

        let entry = ts.files.get(fd).await?;
        let io = entry.to_io().ok_or(EBADF)?;

        io.write(guard.as_slice()).await
    };
    cx.ret(fut.await);
    ScRet::Continue(None)
}

#[async_handler]
pub async fn pread(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(i32, UserBuffer, usize, usize) -> Result<usize, Error>>,
) -> ScRet {
    let (fd, buffer, len, offset) = cx.args();
    let fut = async move {
        if len == 0 {
            return Ok(0);
        }
        let mut guard = ts.virt.start_commit(Attr::WRITABLE).await;
        buffer.commit(&mut guard, len).await?;

        let entry = ts.files.get(fd).await?;
        let io = entry.to_io().ok_or(EBADF)?;

        io.read_at(offset, guard.as_mut_slice()).await
    };
    cx.ret(fut.await);
    ScRet::Continue(None)
}

#[async_handler]
pub async fn pwrite(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(i32, UserBuffer, usize, usize) -> Result<usize, Error>>,
) -> ScRet {
    let (fd, buffer, len, offset) = cx.args();
    let fut = async move {
        if len == 0 {
            return Ok(0);
        }
        let mut guard = ts.virt.start_commit(Attr::READABLE).await;
        buffer.commit(&mut guard, len).await?;

        let entry = ts.files.get(fd).await?;
        let io = entry.to_io().ok_or(EBADF)?;

        io.write_at(offset, guard.as_slice()).await
    };
    cx.ret(fut.await);
    ScRet::Continue(None)
}

#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct IoVec {
    buffer: UserBuffer,
    len: usize,
}
const MAX_IOV_LEN: usize = 8;

#[async_handler]
pub async fn readv(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(i32, UserPtr<IoVec, In>, usize) -> Result<usize, Error>>,
) -> ScRet {
    let (fd, iov, vlen) = cx.args();
    let fut = async move {
        if vlen == 0 {
            return Ok(0);
        }
        let vlen = vlen.min(MAX_IOV_LEN);
        let entry = ts.files.get(fd).await?;
        let io = entry.to_io().ok_or(EBADF)?;

        let mut iov_buf = [Default::default(); MAX_IOV_LEN];
        iov.read_slice(ts.virt.as_ref(), &mut iov_buf[..vlen])
            .await?;

        let mut guard = ts.virt.start_commit(Attr::WRITABLE).await;
        for iov in iov_buf[..vlen].iter() {
            iov.buffer.commit(&mut guard, iov.len).await?;
        }

        io.read(guard.as_mut_slice()).await
    };
    cx.ret(fut.await);
    ScRet::Continue(None)
}

#[async_handler]
pub async fn writev(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(i32, UserPtr<IoVec, In>, usize) -> Result<usize, Error>>,
) -> ScRet {
    let (fd, iov, vlen) = cx.args();
    let fut = async move {
        if vlen == 0 {
            return Ok(0);
        }
        let vlen = vlen.min(MAX_IOV_LEN);
        let entry = ts.files.get(fd).await?;
        let io = entry.to_io().ok_or(EBADF)?;

        let mut iov_buf = [Default::default(); MAX_IOV_LEN];
        iov.read_slice(ts.virt.as_ref(), &mut iov_buf[..vlen])
            .await?;

        let mut guard = ts.virt.start_commit(Attr::READABLE).await;
        for iov in iov_buf[..vlen].iter() {
            iov.buffer.commit(&mut guard, iov.len).await?;
        }

        io.write(guard.as_slice()).await
    };
    cx.ret(fut.await);
    ScRet::Continue(None)
}

#[async_handler]
pub async fn preadv(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(i32, UserPtr<IoVec, In>, usize, usize) -> Result<usize, Error>>,
) -> ScRet {
    let (fd, iov, vlen, offset) = cx.args();
    let fut = async move {
        if vlen == 0 {
            return Ok(0);
        }
        let vlen = vlen.min(MAX_IOV_LEN);
        let entry = ts.files.get(fd).await?;
        let io = entry.to_io().ok_or(EBADF)?;

        let mut iov_buf = [Default::default(); MAX_IOV_LEN];
        iov.read_slice(ts.virt.as_ref(), &mut iov_buf[..vlen])
            .await?;

        let mut guard = ts.virt.start_commit(Attr::WRITABLE).await;
        for iov in iov_buf[..vlen].iter() {
            iov.buffer.commit(&mut guard, iov.len).await?;
        }

        io.read_at(offset, guard.as_mut_slice()).await
    };
    cx.ret(fut.await);
    ScRet::Continue(None)
}

#[async_handler]
pub async fn pwritev(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(i32, UserPtr<IoVec, In>, usize, usize) -> Result<usize, Error>>,
) -> ScRet {
    let (fd, iov, vlen, offset) = cx.args();
    let fut = async move {
        if vlen == 0 {
            return Ok(0);
        }
        let vlen = vlen.min(MAX_IOV_LEN);
        let entry = ts.files.get(fd).await?;
        let io = entry.to_io().ok_or(EBADF)?;

        let mut iov_buf = [Default::default(); MAX_IOV_LEN];
        iov.read_slice(ts.virt.as_ref(), &mut iov_buf[..vlen])
            .await?;

        let mut guard = ts.virt.start_commit(Attr::READABLE).await;
        for iov in iov_buf[..vlen].iter() {
            iov.buffer.commit(&mut guard, iov.len).await?;
        }

        io.write_at(offset, guard.as_slice()).await
    };
    cx.ret(fut.await);
    ScRet::Continue(None)
}

#[async_handler]
pub async fn lseek(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(i32, isize, isize) -> Result<usize, Error>>,
) -> ScRet {
    const SEEK_SET: isize = 0;
    const SEEK_CUR: isize = 1;
    const SEEK_END: isize = 2;

    let (fd, offset, whence) = cx.args();
    let fut = async move {
        let whence = match whence {
            SEEK_SET => SeekFrom::Start(offset as usize),
            SEEK_CUR => SeekFrom::Current(offset),
            SEEK_END => SeekFrom::End(offset),
            _ => return Err(EINVAL),
        };
        let entry = ts.files.get(fd).await?;
        let io = entry.to_io().ok_or(EISDIR)?;
        io.seek(whence).await
    };
    cx.ret(fut.await);

    ScRet::Continue(None)
}

#[async_handler]
pub async fn sendfile(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(i32, i32, UserPtr<usize, InOut>, usize) -> Result<usize, Error>>,
) -> ScRet {
    let (output, input, mut offset_ptr, mut count) = cx.args();
    let fut = async move {
        let output = ts.files.get(output).await?.to_io().ok_or(EISDIR)?;
        let input = ts.files.get(input).await?.to_io().ok_or(EISDIR)?;

        let mut buf = kmem::Frame::new()?;
        if offset_ptr.is_null() {
            let mut ret = 0;
            while count > 0 {
                let len = count.min(PAGE_SIZE);
                let read_len = input.read(&mut [&mut buf[..len]]).await?;
                let written_len = output.write(&mut [&buf[..read_len]]).await?;
                ret += written_len;
                count -= written_len;

                if written_len == 0 || written_len < read_len {
                    break;
                }
            }
            Ok(ret)
        } else {
            let mut offset = offset_ptr.read(ts.virt.as_ref()).await?;
            let mut ret = 0;
            while count > 0 {
                let len = count.min(PAGE_SIZE);
                let read_len = input.read_at(offset, &mut [&mut buf[..len]]).await?;
                let written_len = output.write(&mut [&buf[..read_len]]).await?;

                offset += written_len;
                ret += written_len;
                count -= written_len;
                if written_len == 0 {
                    break;
                }
            }
            offset_ptr.write(ts.virt.as_ref(), offset).await?;
            Ok(ret)
        }
    };
    cx.ret(fut.await);
    ScRet::Continue(None)
}

#[async_handler]
pub async fn fsync(ts: &mut TaskState, cx: UserCx<'_, fn(i32) -> Result<(), Error>>) -> ScRet {
    let fd = cx.args();
    let fut = async {
        let file = ts.files.get(fd).await?;
        file.to_io().ok_or(EISDIR)?.flush().await
    };
    cx.ret(fut.await);
    ScRet::Continue(None)
}

#[async_handler]
pub async fn sync(_: &mut TaskState, cx: UserCx<'_, fn()>) -> ScRet {
    crate::fs::sync();
    cx.ret(());
    ScRet::Continue(None)
}

#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct PollFd {
    fd: i32,
    events: umio::Event,
    revents: umio::Event,
}

async fn poll_fds(
    pfd: &mut [PollFd],
    files: &Files,
    timeout: Option<Duration>,
) -> Result<usize, Error> {
    pfd.iter()
        .for_each(|pfd| log::trace!("Polling fd: {pfd:?}"));

    let files = stream::iter(&*pfd)
        .then(|pfd| files.get(pfd.fd))
        .try_collect::<Vec<_>>()
        .await?;

    pfd.iter_mut()
        .for_each(|pfd| pfd.revents = umio::Event::empty());

    let iter = files.iter().zip(&*pfd).enumerate();
    let events = iter.map(|(index, (e, p))| e.event(p.events).map(move |e| (index, e)));
    let mut events = events.collect::<FuturesUnordered<_>>();

    let mut count = 0;
    let first = match timeout {
        Some(Duration::ZERO) => ksync::poll_once(events.next()).flatten(),
        Some(timeout) => events.next().on_timeout(timeout, || None).await,
        None => events.next().await,
    };
    let Some((index, event)) = first else { return Ok(0) };
    if let Some(event) = event {
        log::trace!("PFD fd = {}, event = {event:?}", pfd[index].fd);
        pfd[index].revents |= event;
        count += 1;
    }
    loop {
        let next = ksync::poll_once(events.next()).flatten();
        let Some((index, event)) = next else { break Ok(count) };
        if let Some(event) = event {
            log::trace!("PFD fd = {}, event = {event:?}", pfd[index].fd);
            pfd[index].revents |= event;
            count += 1;
        }
    }
}

#[async_handler]
pub async fn ppoll(
    ts: &mut TaskState,
    cx: UserCx<
        '_,
        fn(
            UserPtr<PollFd, InOut>,
            usize,
            UserPtr<Ts, In>,
            UserPtr<SigSet, In>,
            usize,
        ) -> Result<usize, Error>,
    >,
) -> ScRet {
    let (mut poll_fd, len, timeout, _sigmask, sigmask_size) = cx.args();
    let fut = async {
        if sigmask_size != mem::size_of::<SigSet>() {
            return Err(EINVAL);
        }
        if len > ts.files.get_limit() {
            return Err(EINVAL);
        }
        let timeout = if timeout.is_null() {
            None
        } else {
            Some(timeout.read(ts.virt.as_ref()).await?.into())
        };

        let mut pfd = vec![PollFd::default(); len];
        poll_fd.read_slice(ts.virt.as_ref(), &mut pfd).await?;

        let count = poll_fds(&mut pfd, &ts.files, timeout).await?;

        poll_fd.write_slice(ts.virt.as_ref(), &pfd, false).await?;
        Ok(count)
    };
    cx.ret(fut.await);
    ScRet::Continue(None)
}

const FD_SET_BITS: usize = usize::BITS as usize;

fn push_pfd(pfd: &mut Vec<PollFd>, fd_set: &[usize], events: umio::Event) {
    for (index, mut fds) in fd_set.iter().copied().enumerate() {
        let base = (index * FD_SET_BITS) as i32;
        while fds > 0 {
            let mask = fds & (!fds + 1);
            let fd = mask.trailing_zeros() as i32 + base;
            pfd.push(PollFd {
                fd,
                events,
                revents: Default::default(),
            });
            fds -= mask;
        }
    }
}

fn write_fd_set(pfd: &[PollFd], fd_set: &mut [usize], events: umio::Event) {
    fd_set.fill(0);
    for pfd in pfd {
        if pfd.revents.contains(events) {
            let index = pfd.fd as usize / FD_SET_BITS;
            let mask = 1 << (pfd.fd as usize % FD_SET_BITS);
            fd_set[index] |= mask;
        }
    }
}

#[async_handler]
pub async fn pselect(
    ts: &mut TaskState,
    cx: UserCx<
        '_,
        fn(
            usize,
            UserPtr<usize, InOut>,
            UserPtr<usize, InOut>,
            UserPtr<usize, InOut>,
            UserPtr<Ts, In>,
            UserPtr<SigSet, In>,
        ) -> Result<usize, Error>,
    >,
) -> ScRet {
    let (count, mut rd, mut wr, mut ex, timeout, _sigmask) = cx.args();
    let fut = async {
        let timeout = if timeout.is_null() {
            None
        } else {
            Some(timeout.read(ts.virt.as_ref()).await?.into())
        };
        if count == 0 {
            match timeout {
                Some(Duration::ZERO) => yield_now().await,
                Some(timeout) => ktime::sleep(timeout).await,
                _ => {}
            }
            return Ok(0);
        }

        let len = (count + FD_SET_BITS - 1) / FD_SET_BITS;
        let mut buf = vec![0; len];

        let mut pfd = Vec::new();
        if !rd.is_null() {
            rd.read_slice(ts.virt.as_ref(), &mut buf).await?;
            push_pfd(&mut pfd, &buf, umio::Event::READABLE);
        }
        if !wr.is_null() {
            wr.read_slice(ts.virt.as_ref(), &mut buf).await?;
            push_pfd(&mut pfd, &buf, umio::Event::WRITABLE);
        }
        if !ex.is_null() {
            ex.read_slice(ts.virt.as_ref(), &mut buf).await?;
            push_pfd(&mut pfd, &buf, umio::Event::EXCEPTION);
        }

        let count = poll_fds(&mut pfd, &ts.files, timeout).await?;

        if !rd.is_null() {
            write_fd_set(&pfd, &mut buf, umio::Event::READABLE);
            rd.write_slice(ts.virt.as_ref(), &buf, false).await?;
        }
        if !wr.is_null() {
            write_fd_set(&pfd, &mut buf, umio::Event::WRITABLE);
            wr.write_slice(ts.virt.as_ref(), &buf, false).await?;
        }
        if !ex.is_null() {
            write_fd_set(&pfd, &mut buf, umio::Event::EXCEPTION);
            ex.write_slice(ts.virt.as_ref(), &buf, false).await?;
        }

        Ok(count)
    };
    cx.ret(fut.await);
    ScRet::Continue(None)
}
