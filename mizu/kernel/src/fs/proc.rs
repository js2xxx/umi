use alloc::{boxed::Box, string::String, sync::Arc};
use core::{
    fmt::Write,
    sync::atomic::{
        AtomicUsize,
        Ordering::{Relaxed, SeqCst},
    },
};

use arsc_rs::Arsc;
use async_trait::async_trait;
use ksc::Error::{self, ENOENT, ENOTDIR, EPERM, ESPIPE};
use ksync::Mutex;
use rv39_paging::PAGE_SIZE;
use umifs::{
    path::Path,
    traits::{Entry, FileSystem},
    types::{FileType, FsStat, Metadata, OpenOptions, Permissions},
};
use umio::*;

pub struct ProcFs;

#[async_trait]
impl FileSystem for ProcFs {
    async fn root_dir(self: Arsc<Self>) -> Result<Arc<dyn Entry>, Error> {
        Ok(Arc::new(ProcRoot::default()))
    }

    async fn flush(&self) -> Result<(), Error> {
        Ok(())
    }

    async fn stat(&self) -> FsStat {
        FsStat {
            ty: "devfs",
            block_size: PAGE_SIZE,
            block_count: 0,
            block_free: 0,
            file_count: 1,
        }
    }
}

#[derive(Default)]
pub struct ProcRoot {
    minfo: Arc<MemInfo>,
    mounts: Arc<Mounts>,
    intrs: Arc<Interrupts>,
}

impl ToIo for ProcRoot {}

#[async_trait]
impl Entry for ProcRoot {
    async fn open(
        self: Arc<Self>,
        path: &Path,
        options: OpenOptions,
        perm: Permissions,
    ) -> Result<(Arc<dyn Entry>, bool), Error> {
        match path.as_str() {
            "meminfo" => self.minfo.clone().open(Path::new(""), options, perm).await,
            "mounts" => self.mounts.clone().open(Path::new(""), options, perm).await,
            "interrupts" => self.intrs.clone().open(Path::new(""), options, perm).await,
            _ => {
                let (dir, _next) = {
                    let mut comp = path.components();
                    (comp.next().ok_or(ENOENT)?.as_str(), comp.as_path())
                };
                match dir {
                    "meminfo" | "mounts" => Err(ENOTDIR),
                    _ => Err(ENOENT),
                }
            }
        }
    }

    async fn metadata(&self) -> Metadata {
        todo!()
    }
}
impl IoPoll for ProcRoot {}

#[derive(Default)]
pub struct MemInfo(Mutex<String>, AtomicUsize);

#[async_trait]
impl Io for MemInfo {
    async fn seek(&self, whence: SeekFrom) -> Result<usize, Error> {
        let pos = match whence {
            SeekFrom::Start(pos) => pos,
            SeekFrom::End(_) => return Err(ESPIPE),
            SeekFrom::Current(pos) if pos >= 0 => self.1.load(SeqCst) + pos as usize,
            SeekFrom::Current(pos) => self.1.load(SeqCst) - (-pos as usize),
        };
        self.1.store(pos, SeqCst);
        Ok(pos)
    }

    async fn read_at(&self, offset: usize, buffer: &mut [IoSliceMut]) -> Result<usize, Error> {
        let alloc_stat = kalloc::stat();
        let total = alloc_stat.total + kmem::frames().total_count() * PAGE_SIZE;
        let used = alloc_stat.used + kmem::frames().used_count() * PAGE_SIZE;

        let mut buf = self.0.lock().await;
        buf.clear();

        writeln!(buf, "MemTotal:     {:>10} kB", total / 1024).unwrap();
        writeln!(buf, "MemAvailable: {:>10} kB", (total - used) / 1024).unwrap();

        let Some(buf) = buf.as_bytes().get(offset..) else {
            return Ok(0)
        };
        Ok(copy_to_ioslice(buf, buffer))
    }

    async fn write_at(&self, _: usize, _: &mut [IoSlice]) -> Result<usize, Error> {
        Err(EPERM)
    }

    async fn flush(&self) -> Result<(), Error> {
        Ok(())
    }
}

#[async_trait]
impl Entry for MemInfo {
    async fn open(
        self: Arc<Self>,
        path: &Path,
        options: OpenOptions,
        perm: Permissions,
    ) -> Result<(Arc<dyn Entry>, bool), Error> {
        umifs::misc::open_file(
            self,
            path,
            options,
            perm,
            Permissions::all_same(true, true, false),
        )
        .await
    }

    async fn metadata(&self) -> Metadata {
        Metadata {
            ty: FileType::FILE,
            len: 0,
            offset: rand_riscv::seed64(),
            perm: Permissions::all_same(true, true, false),
            block_size: 1024,
            block_count: 0,
            times: Default::default(),
        }
    }
}
impl IoPoll for MemInfo {}

#[derive(Default)]
pub struct Mounts(Mutex<String>, AtomicUsize);

#[async_trait]
impl Io for Mounts {
    async fn seek(&self, whence: SeekFrom) -> Result<usize, Error> {
        let pos = match whence {
            SeekFrom::Start(pos) => pos,
            SeekFrom::End(_) => return Err(ESPIPE),
            SeekFrom::Current(pos) if pos >= 0 => self.1.load(SeqCst) + pos as usize,
            SeekFrom::Current(pos) => self.1.load(SeqCst) - (-pos as usize),
        };
        self.1.store(pos, SeqCst);
        Ok(pos)
    }

    async fn read_at(&self, offset: usize, buffer: &mut [IoSliceMut]) -> Result<usize, Error> {
        let fs = ksync::critical(|| super::FS.read().clone());

        let mut buf = self.0.lock().await;
        buf.clear();

        for (dst, handle) in fs.iter() {
            let stat = handle.fs.stat().await;
            write!(buf, "{} /{dst} {} rw,relatime", handle.dev, stat.ty).unwrap();
            if stat.block_count != 0 {
                write!(buf, ",size={}k", stat.block_count * stat.block_size / 1024).unwrap();
            } else {
                write!(buf, ",nosuid,nodev,noexec").unwrap();
            }
            writeln!(buf, " 0 0").unwrap();
        }

        let Some(buf) = buf.as_bytes().get(offset..) else {
            return Ok(0)
        };
        Ok(copy_to_ioslice(buf, buffer))
    }

    async fn write_at(&self, _: usize, _: &mut [IoSlice]) -> Result<usize, Error> {
        Err(EPERM)
    }

    async fn flush(&self) -> Result<(), Error> {
        Ok(())
    }
}

#[async_trait]
impl Entry for Mounts {
    async fn open(
        self: Arc<Self>,
        path: &Path,
        options: OpenOptions,
        perm: Permissions,
    ) -> Result<(Arc<dyn Entry>, bool), Error> {
        umifs::misc::open_file(
            self,
            path,
            options,
            perm,
            Permissions::all_same(true, true, false),
        )
        .await
    }

    async fn metadata(&self) -> Metadata {
        Metadata {
            ty: FileType::FILE,
            len: 0,
            offset: rand_riscv::seed64(),
            perm: Permissions::all_same(true, true, false),
            block_size: 1024,
            block_count: 0,
            times: Default::default(),
        }
    }
}
impl IoPoll for Mounts {}

#[derive(Default)]
pub struct Interrupts(Mutex<String>, AtomicUsize);

#[async_trait]
impl Io for Interrupts {
    async fn seek(&self, whence: SeekFrom) -> Result<usize, Error> {
        let pos = match whence {
            SeekFrom::Start(pos) => pos,
            SeekFrom::End(_) => return Err(ESPIPE),
            SeekFrom::Current(pos) if pos >= 0 => self.1.load(SeqCst) + pos as usize,
            SeekFrom::Current(pos) => self.1.load(SeqCst) - (-pos as usize),
        };
        self.1.store(pos, SeqCst);
        Ok(pos)
    }

    async fn read_at(&self, offset: usize, buffer: &mut [IoSliceMut]) -> Result<usize, Error> {
        let counts = crate::dev::INTR.counts();

        let mut buf = self.0.lock().await;
        buf.clear();

        write!(buf, "0: {}\n", crate::trap::TIMER_COUNT.load(Relaxed)).unwrap();
        for (pin, count) in counts {
            write!(buf, "{pin}: {count}\n").unwrap();
        }

        let Some(buf) = buf.as_bytes().get(offset..) else {
            return Ok(0)
        };
        Ok(copy_to_ioslice(buf, buffer))
    }

    async fn write_at(&self, _: usize, _: &mut [IoSlice]) -> Result<usize, Error> {
        Err(EPERM)
    }

    async fn flush(&self) -> Result<(), Error> {
        Ok(())
    }
}

#[async_trait]
impl Entry for Interrupts {
    async fn open(
        self: Arc<Self>,
        path: &Path,
        options: OpenOptions,
        perm: Permissions,
    ) -> Result<(Arc<dyn Entry>, bool), Error> {
        umifs::misc::open_file(
            self,
            path,
            options,
            perm,
            Permissions::all_same(true, true, false),
        )
        .await
    }

    async fn metadata(&self) -> Metadata {
        Metadata {
            ty: FileType::FILE,
            len: 0,
            offset: rand_riscv::seed64(),
            perm: Permissions::all_same(true, false, false),
            block_size: 1024,
            block_count: 0,
            times: Default::default(),
        }
    }
}
impl IoPoll for Interrupts {}

pub fn copy_to_ioslice(mut buf: &[u8], mut out: &mut [IoSliceMut]) -> usize {
    let mut read_len = 0;
    loop {
        if buf.is_empty() {
            break read_len;
        }
        let Some(first) = out.first_mut() else { break read_len };

        let len = first.len().min(buf.len());
        first[..len].copy_from_slice(&buf[..len]);

        read_len += len;
        buf = &buf[len..];
        advance_slices(&mut out, len);
    }
}
