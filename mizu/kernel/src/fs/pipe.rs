use alloc::{boxed::Box, sync::Arc};
use core::sync::atomic::{AtomicUsize, Ordering::*};

use arsc_rs::Arsc;
use async_trait::async_trait;
use kmem::Phys;
use ksc::{
    Error,
    Error::{EEXIST, ENOTDIR, EPERM, ESPIPE},
};
use ksync::event::Event;
use umifs::{
    path::Path,
    traits::{Entry, Io},
    types::{FileType, Metadata, OpenOptions, Permissions},
};
use umio::{IoPoll, IoSlice, IoSliceMut, SeekFrom};

struct Pipe {
    phys: Phys,
    readable: Event,
    end_pos: AtomicUsize,
}

struct Receiver {
    pipe: Arsc<Pipe>,
    pos: AtomicUsize,
}

#[async_trait]
impl Io for Receiver {
    async fn read(&self, buffer: &mut [IoSliceMut]) -> Result<usize, Error> {
        let mut pos = self.pos.load(Acquire);
        let mut listener = None;
        loop {
            let read_len = self.pipe.phys.read_at(pos, buffer).await?;
            log::trace!("Pipe::read: Attempt to read at {pos}, read len = {read_len}");
            if read_len == 0 {
                log::trace!("Pipe::read: Check pipe ref count");
                if Arsc::count(&self.pipe) == 1 {
                    return Ok(0);
                }
                log::trace!("Pipe::read: Listen for event");
                match listener.take() {
                    Some(listener) => listener.await,
                    None => listener = Some(self.pipe.readable.listen()),
                }
            } else {
                match self
                    .pos
                    .compare_exchange_weak(pos, pos + read_len, AcqRel, Acquire)
                {
                    Ok(_) => break Ok(read_len),
                    Err(p) => pos = p,
                }
            }
        }
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

#[async_trait]
impl IoPoll for Receiver {
    async fn event(&self, expected: umio::Event) -> umio::Event {
        if !expected.contains(umio::Event::READABLE) {
            return umio::Event::INVALID;
        }
        let mut listener = None;
        loop {
            let pos = self.pos.load(SeqCst);
            let end = self.pipe.end_pos.load(SeqCst);
            if pos < end {
                break umio::Event::READABLE;
            }
            if Arsc::count(&self.pipe) == 1 {
                break umio::Event::HANG_UP;
            }
            match listener.take() {
                Some(listener) => listener.await,
                None => listener = Some(self.pipe.readable.listen()),
            }
        }
    }
}

struct Sender {
    pipe: Arsc<Pipe>,
}

#[async_trait]
impl Io for Sender {
    async fn read(&self, _: &mut [IoSliceMut]) -> Result<usize, Error> {
        Err(EPERM)
    }

    async fn write(&self, buffer: &mut [IoSlice]) -> Result<usize, Error> {
        let pos = self.pipe.end_pos.load(SeqCst);
        let written_len = self.pipe.phys.write_at(pos, buffer).await?;

        log::trace!("Pipe::write: Attempt to write, written len = {written_len}");
        if written_len > 0 {
            self.pipe.end_pos.fetch_add(written_len, SeqCst);
            self.pipe.readable.notify(usize::MAX);
        }
        Ok(written_len)
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

impl Drop for Sender {
    fn drop(&mut self) {
        self.pipe.readable.notify(usize::MAX);
    }
}

#[async_trait]
impl IoPoll for Sender {
    async fn event(&self, expected: umio::Event) -> umio::Event {
        if expected != umio::Event::WRITABLE {
            return umio::Event::INVALID;
        }
        if Arsc::count(&self.pipe) == 1 {
            return umio::Event::ERROR;
        }
        umio::Event::WRITABLE
    }
}

pub fn pipe() -> (Arc<dyn Entry>, Arc<dyn Entry>) {
    let phys = Phys::new_anon(true);
    let pipe = Arsc::new(Pipe {
        phys,
        readable: Event::new(),
        end_pos: Default::default(),
    });
    let tx = Arc::new(Sender { pipe: pipe.clone() });
    let rx = Arc::new(Receiver {
        pipe,
        pos: Default::default(),
    });
    (tx, rx)
}
