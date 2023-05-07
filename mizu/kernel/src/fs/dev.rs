use alloc::{boxed::Box, sync::Arc};

use arsc_rs::Arsc;
use async_trait::async_trait;
use kmem::Phys;
use ksc::Error::{self, ENOENT};
use umifs::{
    misc::{Null, Zero},
    path::Path,
    traits::{Entry, FileSystem},
    types::*,
};

pub struct DevFs;

#[async_trait]
impl FileSystem for DevFs {
    async fn root_dir(self: Arsc<Self>) -> Result<Arc<dyn Entry>, Error> {
        Ok(Arc::new(DevRoot))
    }

    async fn flush(&self) -> Result<(), Error> {
        Ok(())
    }
}

pub struct DevRoot;

#[async_trait]
impl Entry for DevRoot {
    async fn open(
        self: Arc<Self>,
        path: &Path,
        expect_ty: Option<FileType>,
        options: OpenOptions,
        perm: Permissions,
    ) -> Result<(Arc<dyn Entry>, bool), Error> {
        match path.as_str() {
            "null" => {
                let null = Arc::new(Null);
                null.open(Path::new(""), expect_ty, options, perm).await
            }
            "zero" => {
                let zero = Arc::new(Zero);
                zero.open(Path::new(""), expect_ty, options, perm).await
            }
            _ => {
                let (dir, next) = {
                    let mut comp = path.components();
                    (comp.next().ok_or(ENOENT)?.as_str(), comp.as_path())
                };
                match dir {
                    "block" => {
                        let dev_blocks = Arc::new(DevBlocks);
                        dev_blocks.open(next, expect_ty, options, perm).await
                    }
                    _ => Err(ENOENT),
                }
            }
        }
    }

    fn metadata(&self) -> Metadata {
        todo!()
    }
}

pub struct DevBlocks;

#[async_trait]
impl Entry for DevBlocks {
    async fn open(
        self: Arc<Self>,
        path: &Path,
        _expect_ty: Option<FileType>,
        _options: OpenOptions,
        _perm: Permissions,
    ) -> Result<(Arc<dyn Entry>, bool), Error> {
        if let Ok(n) = path.as_str().parse() {
            let block = crate::dev::block(n).ok_or(ENOENT)?;
            let _phys = Arc::new(Phys::new(block.to_backend(), 0));
            todo!("implement Entry for Phys using phys' backend")
        }
        Err(ENOENT)
    }

    fn metadata(&self) -> Metadata {
        todo!()
    }
}
