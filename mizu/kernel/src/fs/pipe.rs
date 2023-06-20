use alloc::{boxed::Box, sync::Arc};
use core::sync::atomic::{
    AtomicUsize,
    Ordering::{Relaxed, SeqCst},
};

use arsc_rs::Arsc;
use async_trait::async_trait;
use crossbeam_queue::SegQueue;
use kmem::Frame;
use ksc::{
    Boxed,
    Error::{self, EEXIST, ENOTDIR, EPERM, EPIPE, ESPIPE},
};
use ksync::event::Event;
use rv39_paging::PAGE_SIZE;
use umifs::{
    path::Path,
    traits::Entry,
    types::{FileType, Metadata, OpenOptions, Permissions},
};
use umio::{Io, IoPoll, IoSlice, IoSliceMut, SeekFrom};

struct End {
    frame: Frame,
    start: usize,
    end: usize,
}

#[derive(Default)]
struct Pipe {
    buffer: SegQueue<End>,
    read_end: spin::Mutex<Option<End>>,
    write_end: spin::Mutex<Option<End>>,

    read_event: Event,
    write_event: Event,
    max_buffers: AtomicUsize,
}

struct Sender(Arsc<Pipe>);

struct Receiver(Arsc<Pipe>);

impl End {
    fn is_buffer_full(&self) -> bool {
        self.end == PAGE_SIZE
    }

    fn should_discard(&self) -> bool {
        self.start == PAGE_SIZE
    }

    fn is_empty(&self) -> bool {
        self.start == self.end
    }

    fn read(&mut self, mut buf: &mut [IoSliceMut]) -> usize {
        let mut read_len = 0;
        loop {
            let Some((first, next)) = buf.split_first_mut() else {
                return read_len;
            };
            let len = first.len().min(self.end - self.start);
            first[..len].copy_from_slice(&self.frame[self.start..][..len]);

            read_len += len;
            self.start += len;
            if len < first.len() {
                return read_len;
            }
            buf = next;
        }
    }

    fn write(&mut self, mut buf: &mut [IoSlice]) -> usize {
        let mut written_len = 0;
        loop {
            let Some((first, next)) = buf.split_first_mut() else {
                return written_len;
            };
            let len = first.len().min(PAGE_SIZE - self.end);
            self.frame[self.end..][..len].copy_from_slice(&first[..len]);

            written_len += len;
            self.end += len;
            if len < first.len() {
                return written_len;
            }
            buf = next;
        }
    }
}

impl Receiver {
    async fn read(&self, buffer: &mut [IoSliceMut<'_>]) -> Result<usize, Error> {
        let mut listener = None;
        loop {
            let trial = ksync::critical(|| {
                // First attempt, only lock read end optimistically.

                let mut me = self.0.read_end.lock();
                if let Some(mut end) = me.take().or_else(|| {
                    let new = self.0.buffer.pop();
                    new.inspect(|_| self.0.write_event.notify(usize::MAX))
                }) {
                    assert!(end.is_buffer_full());
                    let len = end.read(buffer);
                    if !end.should_discard() {
                        *me = Some(end);
                    }
                    return Some(len);
                }

                // Second attempt, lock write end and try popping from the buffer queue again.
                //
                // Must try that again with write end lock because the writer might push some
                // buffer into the queue between the first pop attempt and the lock operation.

                let mut writer = self.0.write_end.lock();

                let new = self.0.buffer.pop();
                if let Some(mut end) = new.inspect(|_| self.0.write_event.notify(usize::MAX)) {
                    assert!(end.is_buffer_full());
                    let len = end.read(buffer);
                    if !end.should_discard() {
                        *me = Some(end);
                    }
                    return Some(len);
                }

                // Third attempt, try to access the writer directly.

                if let Some(mut end) = writer.take() {
                    if !end.is_empty() {
                        let len = end.read(buffer);
                        if !end.should_discard() {
                            *writer = Some(end);
                        }
                        return Some(len);
                    }
                }

                // Cannot find the data, waiting for the event.

                None
            });

            if let Some(len) = trial {
                break Ok(len);
            }

            if Arsc::count(&self.0) == 1 {
                break Ok(0);
            }

            match listener.take() {
                Some(l) => l.await,
                None => listener = Some(self.0.read_event.listen()),
            }
        }
    }

    async fn event(&self, expected: umio::Event) -> Option<umio::Event> {
        if !expected.contains(umio::Event::READABLE) {
            return None;
        }
        Some(match self.read(&mut []).await {
            Ok(_) => umio::Event::READABLE,
            Err(_) => umio::Event::HANG_UP,
        })
    }
}

impl Drop for Receiver {
    fn drop(&mut self) {
        self.0.write_event.notify(usize::MAX);
    }
}

#[async_trait]
impl Io for Receiver {
    fn read<'a: 'r, 'b: 'r, 'r>(
        &'a self,
        buffer: &'b mut [IoSliceMut],
    ) -> Boxed<'r, Result<usize, Error>> {
        Box::pin(self.read(buffer))
    }

    async fn write(&self, _: &mut [IoSlice]) -> Result<usize, Error> {
        Err(EPERM)
    }

    async fn seek(&self, _: SeekFrom) -> Result<usize, Error> {
        Err(ESPIPE)
    }

    async fn read_at(&self, _: usize, _: &mut [IoSliceMut]) -> Result<usize, Error> {
        Err(ESPIPE)
    }

    async fn write_at(&self, _: usize, _: &mut [IoSlice]) -> Result<usize, Error> {
        Err(ESPIPE)
    }

    async fn flush(&self) -> Result<(), Error> {
        Ok(())
    }
}

#[async_trait]
impl Entry for Receiver {
    async fn open(
        self: Arc<Self>,
        path: &Path,
        options: OpenOptions,
        perm: Permissions,
    ) -> Result<(Arc<dyn Entry>, bool), Error> {
        if !path.as_str().is_empty() || options.contains(OpenOptions::DIRECTORY) {
            return Err(ENOTDIR);
        }
        if options.contains(OpenOptions::CREAT) {
            return Err(EEXIST);
        }
        if !Permissions::all_same(true, false, false).contains(perm) {
            return Err(EPERM);
        }
        Ok((self, false))
    }

    async fn metadata(&self) -> Metadata {
        Metadata {
            ty: FileType::FIFO,
            len: 0,
            offset: 0,
            perm: Permissions::all_same(true, false, false),
            block_size: 0,
            block_count: 0,
            last_access: None,
            last_modified: None,
            last_created: None,
        }
    }
}

impl IoPoll for Receiver {
    fn event<'a: 'r, 'r>(&'a self, expected: umio::Event) -> Boxed<'r, Option<umio::Event>> {
        Box::pin(self.event(expected))
    }
}

impl Sender {
    async fn write(&self, buffer: &mut [IoSlice<'_>]) -> Result<usize, Error> {
        let alloc_new = || {
            if self.0.buffer.len() < self.0.max_buffers.load(SeqCst) {
                Frame::new().ok().map(|frame| End {
                    frame,
                    start: 0,
                    end: 0,
                })
            } else {
                None
            }
        };

        let mut listener = None;

        loop {
            let trial = ksync::critical(|| {
                let mut me = self.0.write_end.lock();
                if let Some(mut end) = me.take().or_else(alloc_new) {
                    let len = end.write(buffer);
                    if end.is_buffer_full() {
                        assert!(!end.is_empty());
                        self.0.buffer.push(end);
                    } else {
                        *me = Some(end);
                    }
                    return Some(len);
                }
                None
            });

            if let Some(len) = trial {
                self.0.read_event.notify(usize::MAX);
                break Ok(len);
            }

            if Arsc::count(&self.0) == 1 {
                break Err(EPIPE);
            }

            match listener.take() {
                Some(l) => l.await,
                None => listener = Some(self.0.write_event.listen()),
            }
        }
    }

    async fn event(&self, expected: umio::Event) -> Option<umio::Event> {
        if !expected.contains(umio::Event::WRITABLE) {
            return None;
        }
        Some(match self.write(&mut []).await {
            Ok(_) => umio::Event::WRITABLE,
            Err(_) => umio::Event::ERROR,
        })
    }
}

impl Drop for Sender {
    fn drop(&mut self) {
        self.0.read_event.notify(usize::MAX);
    }
}

#[async_trait]
impl Io for Sender {
    async fn read(&self, _: &mut [IoSliceMut]) -> Result<usize, Error> {
        Err(EPERM)
    }

    fn write<'a: 'r, 'b: 'r, 'r>(
        &'a self,
        buffer: &'b mut [IoSlice],
    ) -> Boxed<'r, Result<usize, Error>> {
        Box::pin(self.write(buffer))
    }

    async fn seek(&self, _: SeekFrom) -> Result<usize, Error> {
        Err(ESPIPE)
    }

    async fn read_at(&self, _: usize, _: &mut [IoSliceMut]) -> Result<usize, Error> {
        Err(ESPIPE)
    }

    async fn write_at(&self, _: usize, _: &mut [IoSlice]) -> Result<usize, Error> {
        Err(ESPIPE)
    }

    async fn flush(&self) -> Result<(), Error> {
        Ok(())
    }
}

#[async_trait]
impl Entry for Sender {
    async fn open(
        self: Arc<Self>,
        path: &Path,
        options: OpenOptions,
        perm: Permissions,
    ) -> Result<(Arc<dyn Entry>, bool), Error> {
        if !path.as_str().is_empty() || options.contains(OpenOptions::DIRECTORY) {
            return Err(ENOTDIR);
        }
        if options.contains(OpenOptions::CREAT) {
            return Err(EEXIST);
        }
        if !Permissions::all_same(false, true, false).contains(perm) {
            return Err(EPERM);
        }
        Ok((self, false))
    }

    async fn metadata(&self) -> Metadata {
        Metadata {
            ty: FileType::FIFO,
            len: 0,
            offset: 0,
            perm: Permissions::all_same(false, true, false),
            block_size: 0,
            block_count: 0,
            last_access: None,
            last_modified: None,
            last_created: None,
        }
    }
}

impl IoPoll for Sender {
    fn event<'a: 'r, 'r>(&'a self, expected: umio::Event) -> Boxed<'r, Option<umio::Event>> {
        Box::pin(self.event(expected))
    }
}

pub fn pipe() -> (Arc<dyn Entry>, Arc<dyn Entry>) {
    const DEFAULT_MAX_BUFFERS: usize = 16;

    let pipe = Arsc::new(Pipe::default());
    pipe.max_buffers.store(DEFAULT_MAX_BUFFERS, Relaxed);
    let tx = Arc::new(Sender(pipe.clone()));
    let rx = Arc::new(Receiver(pipe));
    (tx, rx)
}
