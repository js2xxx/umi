use alloc::{boxed::Box, sync::Arc};
use core::fmt::Write;

use arsc_rs::Arsc;
use async_trait::async_trait;
use ksc::Error::{self, ENOENT, ENOTDIR, EPERM, ESPIPE};
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
        Ok(Arc::new(ProcRoot))
    }

    async fn flush(&self) -> Result<(), Error> {
        Ok(())
    }

    async fn stat(&self) -> FsStat {
        FsStat {
            ty: "devfs",
            block_size: PAGE_SIZE,
            block_count: 0xdeadbeef,
            block_free: 0,
            file_count: 1,
        }
    }
}

pub struct ProcRoot;

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
            "meminfo" => {
                let meminfo = Arc::new(MemInfo);
                meminfo.open(Path::new(""), options, perm).await
            }
            _ => {
                let (dir, _next) = {
                    let mut comp = path.components();
                    (comp.next().ok_or(ENOENT)?.as_str(), comp.as_path())
                };
                match dir {
                    "meminfo" => Err(ENOTDIR),
                    _ => Err(ENOENT),
                }
            }
        }
    }

    async fn metadata(&self) -> Metadata {
        todo!()
    }
}

pub struct MemInfo;

#[async_trait]
impl Io for MemInfo {
    async fn seek(&self, _: SeekFrom) -> Result<usize, Error> {
        Err(ESPIPE)
    }

    async fn read_at(&self, offset: usize, mut buffer: &mut [IoSliceMut]) -> Result<usize, Error> {
        if offset != 0 {
            return Ok(0);
        }

        let alloc_stat = kalloc::stat();
        let total = alloc_stat.total + kmem::frames().total_count() * PAGE_SIZE;
        let used = alloc_stat.used + kmem::frames().used_count() * PAGE_SIZE;

        let mut writer = FormatWriter(&mut buffer, 0);
        writeln!(writer, "MemTotal: {:>10} kB", total / 1024).unwrap();
        writeln!(writer, "MemAvailable: {:>10} kB", (total - used) / 1024).unwrap();
        Ok(writer.1)
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
            last_access: None,
            last_modified: None,
            last_created: None,
        }
    }
}
