use alloc::{boxed::Box, sync::Arc};
use core::{
    mem,
    sync::atomic::{AtomicBool, Ordering::SeqCst},
};

use arsc_rs::Arsc;
use async_trait::async_trait;
use kmem::Phys;
use ksc::Error::{self, ENOENT, ENOTDIR};
use rv39_paging::PAGE_SIZE;
use scoped_tls::scoped_thread_local;
use umifs::{
    path::Path,
    traits::*,
    types::{FileType, FsStat, Metadata, OpenOptions, Permissions},
};
use umio::{Io, IoPoll};

pub struct Coverage {
    data: Arc<Phys>,
    enabled: AtomicBool,
}

impl Coverage {
    pub fn new() -> Self {
        Coverage {
            data: Arc::new(Phys::new(false)),
            enabled: AtomicBool::new(false),
        }
    }

    pub fn enable(&self, enable: bool) {
        self.enabled.store(enable, SeqCst)
    }
}

impl Default for Coverage {
    fn default() -> Self {
        Self::new()
    }
}

scoped_thread_local!(pub static COVERAGE: Coverage);

pub async fn coverage() {
    extern "C" {
        #[link_name = "llvm.returnaddress"]
        fn return_address(x: i32) -> *const u8;
    }

    let Some(data) = COVERAGE
        .try_with(|cov| cov.enabled.load(SeqCst).then(|| cov.data.clone()))
        .flatten() else
    {
        return;
    };

    const SZ: usize = mem::size_of::<usize>();

    let mut buffer = [0u8; SZ];
    if data.read_at(0, &mut [&mut buffer]).await != Ok(SZ) {
        return;
    }

    let count = usize::from_le_bytes(buffer);
    let return_address = unsafe { return_address(0) } as usize;
    let _ = data
        .write_at((count + 1) * SZ, &mut [&return_address.to_le_bytes()])
        .await;
    let _ = data.write_at(0, &mut [&(count + 1).to_le_bytes()]).await;
}

pub struct DebugFs;

#[async_trait]
impl FileSystem for DebugFs {
    async fn root_dir(self: Arsc<Self>) -> Result<Arc<dyn Entry>, Error> {
        Ok(Arc::new(DebugRoot))
    }

    async fn flush(&self) -> Result<(), Error> {
        Ok(())
    }

    async fn stat(&self) -> FsStat {
        FsStat {
            ty: "debugfs",
            block_size: PAGE_SIZE,
            block_count: 0,
            block_free: 0,
            file_count: 1,
        }
    }
}

pub struct DebugRoot;

impl ToIo for DebugRoot {}

#[async_trait]
impl Entry for DebugRoot {
    async fn open(
        self: Arc<Self>,
        path: &Path,
        options: OpenOptions,
        perm: Permissions,
    ) -> Result<(Arc<dyn Entry>, bool), Error> {
        match path.as_str() {
            "kcov" => {
                let null = Arc::new(CoverageFile);
                null.open(Path::new(""), options, perm).await
            }
            _ => {
                let dir = {
                    let mut comp = path.components();
                    comp.next().ok_or(ENOENT)?.as_str()
                };
                match dir {
                    "kcov" => Err(ENOTDIR),
                    _ => Err(ENOENT),
                }
            }
        }
    }

    async fn metadata(&self) -> Metadata {
        todo!()
    }
}

impl IoPoll for DebugRoot {}

pub struct CoverageFile;

impl CoverageFile {
    pub fn enable(&self, enable: bool) {
        COVERAGE.try_with(|cov| cov.enable(enable));
    }
}

impl ToIo for CoverageFile {
    fn to_io(self: Arc<Self>) -> Option<Arc<dyn Io>> {
        COVERAGE.try_with(|cov| cov.data.clone() as _)
    }
}

#[async_trait]
impl Entry for CoverageFile {
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
            offset: 0,
            perm: Permissions::all_same(true, false, false),
            block_size: 0,
            block_count: 0,
            times: Default::default(),
        }
    }
}

impl IoPoll for CoverageFile {}
