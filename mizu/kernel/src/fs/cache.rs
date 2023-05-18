use alloc::{boxed::Box, sync::Arc};

use arsc_rs::Arsc;
use async_trait::async_trait;
use hashbrown::HashMap;
use kmem::Phys;
use ksc::{
    Boxed,
    Error::{self, *},
};
use rand_riscv::RandomState;
use spin::RwLock;
use umifs::{path::*, traits::*, types::*};

pub struct CachedFs {
    inner: Arsc<dyn FileSystem>,
    root_dir: Arc<CachedDir>,
}

impl CachedFs {
    pub async fn new(fs: Arsc<dyn FileSystem>) -> Result<Arsc<Self>, Error> {
        let root_dir = Arc::new(CachedDir {
            entry: fs.clone().root_dir().await?,
            cache: RwLock::new(Default::default()),
        });
        Ok(Arsc::new(CachedFs {
            inner: fs,
            root_dir,
        }))
    }
}

#[async_trait]
impl FileSystem for CachedFs {
    async fn root_dir(self: Arsc<Self>) -> Result<Arc<dyn Entry>, Error> {
        Ok(self.root_dir.clone())
    }

    fn flush<'a: 'r, 'r>(&'a self) -> Boxed<'r, Result<(), Error>> {
        self.inner.flush()
    }
}

#[derive(Clone)]
enum EntryCache {
    Dir(Arc<CachedDir>),
    File(Arc<CachedFile>),
}

pub struct CachedDir {
    entry: Arc<dyn Entry>,
    cache: RwLock<HashMap<PathBuf, EntryCache, RandomState>>,
}

pub struct CachedFile {
    entry: Arc<dyn Entry>,
    phys: Arc<Phys>,
}

impl ToIo for CachedDir {}

#[async_trait]
impl Entry for CachedDir {
    async fn open(
        self: Arc<Self>,
        path: &Path,
        options: OpenOptions,
        perm: Permissions,
    ) -> Result<(Arc<dyn Entry>, bool), Error> {
        let expect_dir = options.contains(OpenOptions::DIRECTORY);

        if let Some(ec) = ksync::critical(|| self.cache.read().get(path).cloned()) {
            let entry: Arc<dyn Entry> = match ec {
                EntryCache::Dir(_) if !expect_dir => return Err(EISDIR),
                EntryCache::File(_) if expect_dir => return Err(ENOTDIR),
                EntryCache::Dir(dir) => dir,
                EntryCache::File(file) => file,
            };
            return Ok((entry, false));
        }
        let (entry, created) = self.entry.clone().open(path, options, perm).await?;
        let (ec, entry): (_, Arc<dyn Entry>) = if expect_dir {
            let dir = Arc::new(CachedDir {
                entry,
                cache: RwLock::new(Default::default()),
            });
            (EntryCache::Dir(dir.clone()), dir)
        } else {
            let io = entry.clone().to_io().ok_or(EISDIR)?;
            let file = Arc::new(CachedFile {
                entry,
                phys: Arc::new(Phys::new(io, 0, false)),
            });
            (EntryCache::File(file.clone()), file)
        };
        ksync::critical(|| self.cache.write().insert(path.to_path_buf(), ec));
        Ok((entry, created))
    }

    fn metadata<'a: 'b, 'b>(&'a self) -> Boxed<'b, Metadata> {
        self.entry.metadata()
    }

    fn to_dir(self: Arc<Self>) -> Option<Arc<dyn Directory>> {
        Some(self)
    }

    fn to_dir_mut(self: Arc<Self>) -> Option<Arc<dyn DirectoryMut>> {
        Some(self)
    }
}

#[async_trait]
impl Directory for CachedDir {
    async fn next_dirent(&self, last: Option<&DirEntry>) -> Result<Option<DirEntry>, Error> {
        let dir = self.entry.clone().to_dir().ok_or(EPERM)?;
        dir.next_dirent(last).await
    }
}

#[async_trait]
impl DirectoryMut for CachedDir {
    async fn rename(
        self: Arc<Self>,
        src_path: &Path,
        dst_parent: Arc<dyn DirectoryMut>,
        dst_path: &Path,
    ) -> Result<(), Error> {
        let dir = self.entry.clone().to_dir_mut().ok_or(EPERM)?;
        dir.rename(src_path, dst_parent, dst_path).await
    }

    async fn link(
        self: Arc<Self>,
        src_path: &Path,
        dst_parent: Arc<dyn DirectoryMut>,
        dst_path: &Path,
    ) -> Result<(), Error> {
        let dir = self.entry.clone().to_dir_mut().ok_or(EPERM)?;
        dir.link(src_path, dst_parent, dst_path).await
    }

    async fn unlink(&self, path: &Path, expect_dir: Option<bool>) -> Result<(), Error> {
        let dir = self.entry.clone().to_dir_mut().ok_or(EPERM)?;
        dir.unlink(path, expect_dir).await
    }
}

#[async_trait]
impl Io for CachedFile {
    fn read<'a: 'r, 'b: 'r, 'r>(
        &'a self,
        buffer: &'b mut [IoSliceMut],
    ) -> Boxed<'r, Result<usize, Error>> {
        self.phys.read(buffer)
    }

    fn write<'a: 'r, 'b: 'r, 'r>(
        &'a self,
        buffer: &'b mut [IoSlice],
    ) -> Boxed<'r, Result<usize, Error>> {
        self.phys.write(buffer)
    }

    fn seek<'a: 'r, 'r>(&'a self, whence: SeekFrom) -> Boxed<'r, Result<usize, Error>> {
        self.phys.seek(whence)
    }

    fn stream_len<'a: 'r, 'r>(&'a self) -> Boxed<'r, Result<usize, Error>> {
        self.phys.stream_len()
    }

    fn read_at<'a: 'r, 'b: 'r, 'r>(
        &'a self,
        offset: usize,
        buffer: &'b mut [IoSliceMut],
    ) -> Boxed<'r, Result<usize, Error>> {
        self.phys.read_at(offset, buffer)
    }

    fn write_at<'a: 'r, 'b: 'r, 'r>(
        &'a self,
        offset: usize,
        buffer: &'b mut [IoSlice],
    ) -> Boxed<'r, Result<usize, Error>> {
        self.phys.write_at(offset, buffer)
    }

    async fn flush(&self) -> Result<(), Error> {
        self.phys.flush_all().await
    }
}

#[async_trait]
impl Entry for CachedFile {
    async fn open(
        self: Arc<Self>,
        path: &Path,
        options: OpenOptions,
        perm: Permissions,
    ) -> Result<(Arc<dyn Entry>, bool), Error> {
        let _ = self.entry.clone().open(path, options, perm).await?;
        Ok((self, false))
    }

    fn metadata<'a: 'b, 'b>(&'a self) -> Boxed<'b, Metadata> {
        self.entry.metadata()
    }
}
