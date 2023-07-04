use alloc::{boxed::Box, sync::Arc};
use core::num::NonZeroUsize;

use arsc_rs::Arsc;
use async_trait::async_trait;
use kmem::{LruCache, Phys};
use ksc::{
    Boxed,
    Error::{self, *},
};
use rand_riscv::RandomState;
use spin::Mutex;
use umifs::{path::*, traits::*, types::*};
use umio::{Event, IoPoll, SeekFrom};

pub struct CachedFs {
    inner: Arsc<dyn FileSystem>,
    root_dir: Arc<CachedDir>,
}

const CACHE_SIZE: NonZeroUsize = unsafe { NonZeroUsize::new_unchecked(32) };

impl CachedFs {
    pub async fn new(fs: Arsc<dyn FileSystem>) -> Result<Arsc<Self>, Error> {
        let root_dir = Arc::new(CachedDir {
            entry: fs.clone().root_dir().await?,
            cache: Mutex::new(LruCache::with_hasher(CACHE_SIZE, RandomState::new())),
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

    fn stat<'a: 'r, 'r>(&'a self) -> Boxed<'r, FsStat> {
        self.inner.stat()
    }
}

#[derive(Clone)]
enum EntryCache {
    Dir(Arc<CachedDir>),
    File(CachedFile),
}

pub struct CachedDir {
    entry: Arc<dyn Entry>,
    cache: Mutex<LruCache<PathBuf, EntryCache, RandomState>>,
}

pub struct CachedFile {
    entry: Arc<dyn Entry>,
    phys: Arc<Phys>,
}

impl Clone for CachedFile {
    fn clone(&self) -> Self {
        Self {
            entry: self.entry.clone(),
            phys: Arc::new((*self.phys).clone()),
        }
    }
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
        let create = options.contains(OpenOptions::CREAT);

        if let Some(ec) = ksync::critical(|| self.cache.lock().get(path).cloned()) {
            let entry: Arc<dyn Entry> = match ec {
                EntryCache::Dir(_) if !expect_dir && create => return Err(EISDIR),
                EntryCache::File(_) if expect_dir => return Err(ENOTDIR),
                EntryCache::Dir(dir) => dir,
                EntryCache::File(file) => {
                    if options.contains(OpenOptions::APPEND) {
                        file.phys.seek(SeekFrom::End(0)).await?;
                    }
                    Arc::new(file)
                }
            };
            return Ok((entry, false));
        }
        let (entry, created) = self.entry.clone().open(path, options, perm).await?;
        let (ec, entry): (_, Arc<dyn Entry>) = match entry.clone().to_dir() {
            None if expect_dir => return Err(ENOTDIR),
            Some(_) => {
                let dir = Arc::new(CachedDir {
                    entry,
                    cache: Mutex::new(LruCache::with_hasher(CACHE_SIZE, RandomState::new())),
                });
                (EntryCache::Dir(dir.clone()), dir)
            }
            None => {
                let io = entry.clone().to_io().ok_or(EISDIR)?;
                let file = CachedFile {
                    entry,
                    phys: Arc::new(crate::mem::new_phys(io, false)),
                };
                let ec = EntryCache::File(file.clone());
                if options.contains(OpenOptions::APPEND) {
                    file.phys.seek(SeekFrom::End(0)).await?;
                }
                (ec, Arc::new(file))
            }
        };
        ksync::critical(|| self.cache.lock().put(path.to_path_buf(), ec));
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
impl IoPoll for CachedDir {
    fn event<'s: 'r, 'r>(&'s self, expected: Event) -> Boxed<'r, Option<Event>> {
        self.entry.event(expected)
    }
}

#[async_trait]
impl Directory for CachedDir {
    async fn next_dirent(&self, last: Option<&DirEntry>) -> Result<Option<DirEntry>, Error> {
        let dir = self.entry.clone().to_dir().ok_or(ENOTDIR)?;
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
        let dst_cached = dst_parent.downcast::<Self>().ok_or(ENOSYS)?;
        let dst_parent = dst_cached.entry.clone().to_dir_mut().ok_or(EPERM)?;
        dir.rename(src_path, dst_parent, dst_path).await
    }

    async fn link(
        self: Arc<Self>,
        src_path: &Path,
        dst_parent: Arc<dyn DirectoryMut>,
        dst_path: &Path,
    ) -> Result<(), Error> {
        let dir = self.entry.clone().to_dir_mut().ok_or(EPERM)?;
        let dst_cached = dst_parent.downcast::<Self>().ok_or(ENOSYS)?;
        let dst_parent = dst_cached.entry.clone().to_dir_mut().ok_or(EPERM)?;
        dir.link(src_path, dst_parent, dst_path).await
    }

    async fn unlink(&self, path: &Path, expect_dir: Option<bool>) -> Result<(), Error> {
        let dir = self.entry.clone().to_dir_mut().ok_or(EPERM)?;
        dir.unlink(path, expect_dir).await
    }
}

impl ToIo for CachedFile {
    fn to_io(self: Arc<Self>) -> Option<Arc<dyn Io>> {
        Some(self.phys.clone())
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

    fn set_metadata<'a: 'b, 'b>(&'a self, metadata: SetMetadata) -> Boxed<'b, Result<(), Error>> {
        if let Some(new_len) = metadata.len {
            self.phys.resize(new_len);
        }
        self.entry.set_metadata(metadata)
    }
}

impl IoPoll for CachedFile {
    fn event<'s: 'r, 'r>(&'s self, expected: Event) -> Boxed<'r, Option<Event>> {
        self.entry.event(expected)
    }
}
