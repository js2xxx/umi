use alloc::{boxed::Box, sync::Arc};

use arsc_rs::Arsc;
use async_trait::async_trait;
use kmem::Phys;
use ksc::Error::{self, EEXIST, ENOENT, ENOTDIR, EPERM};
use rv39_paging::PAGE_SIZE;
use umifs::{
    misc::{Null, Zero},
    path::Path,
    traits::{Entry, FileSystem, Io, ToIo},
    types::*,
};
use umio::IoPoll;

use super::serial::Serial;

pub struct DevFs;

#[async_trait]
impl FileSystem for DevFs {
    async fn root_dir(self: Arsc<Self>) -> Result<Arc<dyn Entry>, Error> {
        Ok(Arc::new(DevRoot))
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
            file_count: 3 + crate::dev::blocks().len(),
        }
    }
}

pub struct DevRoot;

impl ToIo for DevRoot {}

#[async_trait]
impl Entry for DevRoot {
    async fn open(
        self: Arc<Self>,
        path: &Path,
        options: OpenOptions,
        perm: Permissions,
    ) -> Result<(Arc<dyn Entry>, bool), Error> {
        match path.as_str() {
            "null" => {
                let null = Arc::new(Null);
                null.open(Path::new(""), options, perm).await
            }
            "zero" => {
                let zero = Arc::new(Zero);
                zero.open(Path::new(""), options, perm).await
            }
            "serial" => {
                let serial = Arc::new(Serial::default());
                serial.open(Path::new(""), options, perm).await
            }
            _ => {
                let (dir, next) = {
                    let mut comp = path.components();
                    (comp.next().ok_or(ENOENT)?.as_str(), comp.as_path())
                };
                match dir {
                    "block" => {
                        let dev_blocks = Arc::new(DevBlocks);
                        dev_blocks.open(next, options, perm).await
                    }
                    "null" | "zero" | "serial" => Err(ENOTDIR),
                    _ => Err(ENOENT),
                }
            }
        }
    }

    async fn metadata(&self) -> Metadata {
        todo!()
    }
}

impl IoPoll for DevRoot {}

pub struct DevBlocks;

impl ToIo for DevBlocks {}

#[async_trait]
impl Entry for DevBlocks {
    async fn open(
        self: Arc<Self>,
        path: &Path,
        _options: OpenOptions,
        _perm: Permissions,
    ) -> Result<(Arc<dyn Entry>, bool), Error> {
        let Ok(n) = path.as_str().parse() else {
            return Err(ENOENT)
        };
        let block = crate::dev::block(n).ok_or(ENOENT)?;
        let block_shift = block.block_shift();
        let block_count = block.capacity_blocks();
        let phys = crate::mem::new_phys(block.to_io().unwrap(), false);
        Ok((
            Arc::new(BlockEntry {
                io: Arc::new(phys),
                block_shift,
                block_count,
            }),
            false,
        ))
    }

    async fn metadata(&self) -> Metadata {
        todo!()
    }
}
impl IoPoll for DevBlocks {}

pub struct BlockEntry {
    io: Arc<Phys>,
    block_shift: u32,
    block_count: usize,
}

#[async_trait]
impl Entry for BlockEntry {
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
        if !Permissions::all_same(true, true, true).contains(perm) {
            return Err(EPERM);
        }
        Ok((self, false))
    }

    async fn metadata(&self) -> Metadata {
        Metadata {
            ty: FileType::BLK,
            len: 0,
            offset: 0xdeadbeef,
            perm: Permissions::all_same(true, true, true),
            block_size: 1 << self.block_shift,
            block_count: self.block_count,
            times: Default::default(),
        }
    }
}

impl IoPoll for BlockEntry {}

impl ToIo for BlockEntry {
    fn to_io(self: Arc<Self>) -> Option<Arc<dyn Io>> {
        Some(self.io.clone())
    }
}
