use alloc::{boxed::Box, vec::Vec, sync::Arc};
use core::sync::atomic::{
    AtomicUsize,
    Ordering::{Relaxed, SeqCst},
};

use arsc_rs::Arsc;
use async_trait::async_trait;
use futures_util::TryStreamExt;
use ksc_core::Error::{self, EINVAL, EISDIR, ENOSYS, ENOTDIR};
use ksync::{Mutex, RwLock, RwLockUpgradableReadGuard, RwLockWriteGuard};
use umifs::{
    traits::{Entry, File},
    types::{advance_slices, ioslice_len, IoSlice, IoSliceMut, Metadata, SeekFrom, OpenOptions, Permissions, FileType}, path::Path,
};

use crate::{dirent::DirEntryEditor, fs::FatFileSystem, TimeProvider};

#[derive(Debug)]
pub struct FatFile<T: TimeProvider> {
    pub(crate) fs: Arsc<FatFileSystem<T>>,
    clusters: RwLock<Vec<u32>>,
    cluster_shift: u32,

    entry: Option<Mutex<DirEntryEditor>>,
    len: AtomicUsize,
    cur_offset: AtomicUsize,
}

impl<T: TimeProvider> FatFile<T> {
    pub(crate) async fn new(
        fs: Arsc<FatFileSystem<T>>,
        first_cluster: Option<u32>,
        entry: Option<DirEntryEditor>,
    ) -> Result<Self, Error> {
        let cluster_shift = fs.bpb.cluster_size().ilog2();

        let clusters = match first_cluster {
            Some(first_cluster) => {
                fs.fat
                    .cluster_chain(first_cluster)
                    .try_collect::<Vec<_>>()
                    .await?
            }
            None => Vec::new(),
        };

        let len = entry
            .as_ref()
            .and_then(|e| e.inner().size().map(|s| s as usize))
            .unwrap_or(clusters.len() << cluster_shift);

        log::trace!("FatFile::new: clusters = {clusters:?}");

        Ok(FatFile {
            fs,
            clusters: RwLock::new(clusters),
            cluster_shift,
            entry: entry.map(Mutex::new),
            len: AtomicUsize::new(len),
            cur_offset: AtomicUsize::new(0),
        })
    }

    pub(crate) async fn abs_start_pos(&self) -> Option<u64> {
        let cluster = self.clusters.read().await.first().cloned();
        cluster.map(|cluster| u64::from(cluster) << self.cluster_shift)
    }

    pub(crate) async fn first_cluster(&self) -> Option<u32> {
        self.clusters.read().await.first().cloned()
    }

    pub async fn truncate(&self, new_len: u32) -> Result<(), Error> {
        log::trace!("FatFile::truncate to {new_len}");

        let Some(ref entry) = self.entry else {
            return Err(ENOSYS)
        };

        let mut clusters = self.clusters.write().await;
        let mut entry = entry.lock().await;

        let len = match entry.inner().size() {
            Some(size) => size as usize,
            None => return Err(EISDIR),
        };
        let new_len = new_len as usize;

        if new_len >= len {
            return Ok(());
        }
        let (cluster_index, _) = match self.decomp_end(len) {
            Some(data) => data,
            None => return Ok(()),
        };
        let new_decomp = self.decomp_end(new_len);

        match new_decomp {
            Some((new_cluster_index, _)) => {
                if cluster_index == new_cluster_index {
                    self.len.store(new_len, Relaxed);
                    entry.set_size(new_len as u32);
                    return Ok(());
                }
                let start = clusters[new_cluster_index];
                clusters.truncate(new_cluster_index + 1);
                self.fs.truncate_cluster_chain(start).await?;
            }
            None => {
                entry.set_first_cluster(None);
                let start = clusters[0];
                clusters.clear();
                self.fs.free_cluster_chain(start).await?;
            }
        }
        self.len.store(new_len, Relaxed);
        entry.set_size(new_len as u32);

        Ok(())
    }

    fn decomp(&self, offset: usize) -> (usize, usize) {
        let cluster_index = offset >> self.cluster_shift;
        let offset_in_cluster = offset & ((1 << self.cluster_shift) - 1);
        (cluster_index, offset_in_cluster)
    }

    fn decomp_end(&self, offset: usize) -> Option<(usize, usize)> {
        let cluster_index = offset >> self.cluster_shift;
        let offset_in_cluster = offset & ((1 << self.cluster_shift) - 1);
        if offset_in_cluster == 0 {
            cluster_index
                .checked_sub(1)
                .map(|cluster_index| (cluster_index, 1 << self.cluster_shift))
        } else {
            Some((cluster_index, offset_in_cluster))
        }
    }

    async fn update_read(&self) {
        if let Some(ref entry) = self.entry {
            let now = self.fs.time_provider.get_current_date();
            let mut e = entry.lock().await;
            e.set_accessed(now);
        }
    }

    async fn update_write(&self, offset: u32) {
        self.len.fetch_max(offset as _, Relaxed);
        if let Some(ref entry) = self.entry {
            let now = self.fs.time_provider.get_current_date_time();
            let mut e = entry.lock().await;
            e.set_modified(now);

            if e.inner().size().map_or(false, |s| offset > s) {
                e.set_size(offset);
            }
        }
    }
}

#[async_trait]
impl<T: TimeProvider> File for FatFile<T> {
    async fn seek(&self, whence: SeekFrom) -> Result<usize, Error> {
        log::trace!("FatFile::seek {whence:?}");

        let offset = match whence {
            SeekFrom::Start(offset) => offset,
            SeekFrom::End(offset) => {
                let len = self.len.load(SeqCst);
                if offset > 0 {
                    len.checked_add(offset as usize)
                } else {
                    len.checked_sub((-offset) as usize)
                }
                .ok_or(EINVAL)?
            }
            SeekFrom::Current(offset) => {
                let cur = self.cur_offset.load(SeqCst);
                if offset > 0 {
                    cur.checked_add(offset as usize)
                } else {
                    cur.checked_sub((-offset) as usize)
                }
                .ok_or(EINVAL)?
            }
        };
        self.cur_offset.store(offset, SeqCst);
        Ok(offset)
    }

    async fn read_at(
        &self,
        mut offset: usize,
        mut buffer: &mut [IoSliceMut],
    ) -> Result<usize, Error> {
        log::trace!(
            "FatFile::read_at {offset:#x}, buffer len = {}",
            ioslice_len(&buffer)
        );

        let cluster_shift = self.cluster_shift;
        let (cluster_index, offset_in_cluster) = self.decomp(offset);

        let clusters = self.clusters.read().await;

        let Some(&cluster) = clusters.get(cluster_index) else {
            return Ok(0)
        };

        let mut cluster_offset = self.fs.offset_from_cluster(cluster) as usize + offset_in_cluster;
        let mut rest = (1 << cluster_shift) - offset_in_cluster;
        let mut read_len = 0;
        let device = self.fs.fat.device();
        loop {
            if rest == 0 || buffer.is_empty() {
                self.update_read().await;
                break Ok(read_len);
            }
            let len = rest.min(buffer[0].len());

            let len = device
                .read_at(cluster_offset, &mut [&mut buffer[0][..len]])
                .await?;

            cluster_offset += len;
            offset += len;
            read_len += len;
            rest -= len;
            advance_slices(&mut buffer, len)
        }
    }

    async fn write_at(
        &self,
        mut offset: usize,
        mut buffer: &mut [IoSlice],
    ) -> Result<usize, Error> {
        log::trace!(
            "FatFile::write_at {offset:#x}, buffer len = {}",
            ioslice_len(&buffer)
        );

        let cluster_shift = self.cluster_shift;
        let (cluster_index, offset_in_cluster) = self.decomp(offset);

        let clusters = self.clusters.upgradable_read().await;

        let (cluster, _clusters) = {
            let cluster = clusters.get(cluster_index).cloned();
            match cluster {
                Some(cluster) => (cluster, clusters),
                None => {
                    let mut clusters = RwLockUpgradableReadGuard::upgrade(clusters).await;
                    let mut times = clusters.len() + 1 - cluster_index;
                    let mut prev = clusters.last().cloned();
                    loop {
                        let new = self.fs.fat.allocate(prev, None).await?;
                        if clusters.is_empty() {
                            if let Some(ref entry) = self.entry {
                                entry.lock().await.set_first_cluster(Some(new));
                            }
                        }
                        clusters.push(new);
                        times -= 1;
                        if times == 0 {
                            break (new, RwLockWriteGuard::downgrade_to_upgradable(clusters));
                        }
                        prev = Some(new)
                    }
                }
            }
        };

        let mut cluster_offset = self.fs.offset_from_cluster(cluster) as usize + offset_in_cluster;
        let mut rest = (1 << cluster_shift) - offset_in_cluster;
        let mut written_len = 0;
        let device = self.fs.fat.device();
        loop {
            if rest == 0 || buffer.is_empty() {
                self.update_write(offset as u32).await;
                break Ok(written_len);
            }
            let len = rest.min(buffer[0].len());
            let len = device
                .write_at(cluster_offset, &mut [&buffer[0][..len]])
                .await?;

            cluster_offset += len;
            offset += len;
            written_len += len;
            rest -= len;
            advance_slices(&mut buffer, len)
        }
    }

    async fn flush(&self) -> Result<(), Error> {
        if let Some(ref entry) = self.entry {
            entry.lock().await.flush(&**self.fs.fat.device()).await?;
        }
        Ok(())
    }
}

#[async_trait]
impl<T: TimeProvider> Entry for FatFile<T> {
    async fn open(
        self: Arc<Self>,
        path: &Path,
        expect_ty: Option<FileType>,
        _options: OpenOptions,
        _perm: Permissions,
    ) -> Result<(Arc<dyn Entry>, bool), Error> {
        if !path.as_str().is_empty() {
            return Err(ENOTDIR)
        }
        if !matches!(expect_ty, None | Some(FileType::FILE)) {
            return Err(ENOTDIR);
        }
        // TODO: Check options & permissions
        Ok((self, false))
    }

    fn metadata(&self) -> Metadata {
        todo!()
    }
}
