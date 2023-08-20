use alloc::{boxed::Box, sync::Arc, vec::Vec};
use core::{
    future::ready,
    iter,
    sync::atomic::{
        AtomicU32, AtomicUsize,
        Ordering::{Relaxed, SeqCst},
    },
};

use arsc_rs::Arsc;
use async_trait::async_trait;
use ksc_core::{
    handler::Boxed,
    Error::{self, EINVAL, EISDIR, ENOSYS, ENOTDIR},
};
use ksync::{Mutex, RwLock};
use umifs::{
    path::Path,
    traits::{Entry, Io},
    types::{FileType, Metadata, OpenOptions, Permissions, SetMetadata},
};
use umio::{advance_slices, IoPoll, IoSlice, IoSliceMut, SeekFrom};

use crate::{dirent::DirEntryEditor, fs::FatFileSystem, TimeProvider};

#[derive(Debug)]
pub struct FatFile<T: TimeProvider> {
    pub(crate) fs: Arsc<FatFileSystem<T>>,
    clusters: RwLock<Vec<(u32, u32)>>,
    cluster_shift: u32,

    perm: AtomicU32,
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
            Some(first_cluster) => fs.fat.all_clusters(first_cluster).await?,
            None => Vec::new(),
        };

        let len = entry
            .as_ref()
            .and_then(|e| e.inner().size().map(|s| s as usize))
            .unwrap_or(clusters.len() << cluster_shift);

        // log::trace!("FatFile::new: length = {}", len);

        Ok(FatFile {
            fs,
            clusters: RwLock::new(clusters),
            cluster_shift,
            perm: AtomicU32::new(Permissions::me(true, true, true).bits()),
            entry: entry.map(Mutex::new),
            len: AtomicUsize::new(len),
            cur_offset: AtomicUsize::new(0),
        })
    }

    pub(crate) async fn abs_start_pos(&self) -> Option<u64> {
        let cluster = self.first_cluster().await;
        cluster.map(|cluster| u64::from(cluster) << self.cluster_shift)
    }

    pub(crate) async fn first_cluster(&self) -> Option<u32> {
        self.clusters.read().await.first().map(|&(c, _)| c)
    }

    pub async fn truncate(&self, new_len: u32) -> Result<(), Error> {
        // log::trace!("FatFile::truncate to {new_len}");

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
            Some((new_cluster_index, _)) if cluster_index != new_cluster_index => {
                let (start, _) = clusters[new_cluster_index];
                clusters.truncate(new_cluster_index + 1);
                if let Some(&(last, old_end)) = clusters.last() {
                    for (_, end) in clusters.iter_mut().rev() {
                        if *end != old_end {
                            break;
                        }
                        *end = last;
                    }
                }
                self.fs.truncate_cluster_chain(start).await?;
            }
            None => {
                entry.set_first_cluster(None);
                let (start, _) = clusters[0];
                clusters.clear();
                self.fs.free_cluster_chain(start).await?;
            }
            _ => {}
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
        // log::trace!("FAT32 file update write: offset = {offset:?}");
        self.len.fetch_max(offset as _, SeqCst);
        if let Some(ref entry) = self.entry {
            let now = self.fs.time_provider.get_current_date_time();
            let mut e = entry.lock().await;
            e.set_modified(now);

            if e.inner().size().map_or(false, |s| offset > s) {
                e.set_size(offset);
            }
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
impl<T: TimeProvider> Io for FatFile<T> {
    async fn seek(&self, whence: SeekFrom) -> Result<usize, Error> {
        // log::trace!("FatFile::seek {whence:?}");

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

    fn stream_len<'a: 'b, 'b>(&'a self) -> Boxed<'b, Result<usize, Error>> {
        Box::pin(ready(Ok(self.len.load(SeqCst))))
    }

    async fn read_at(&self, offset: usize, mut buffer: &mut [IoSliceMut]) -> Result<usize, Error> {
        // let ioslice_len = umio::ioslice_len(&buffer);
        // log::trace!("FatFile::read_at {offset:#x}, buffer len = {ioslice_len}");

        let cluster_shift = self.cluster_shift;
        let (cluster_index, offset_in_cluster) = self.decomp(offset);

        let Some((end_len_ci, end_len_oc)) = self
            .decomp_end(self.len.load(SeqCst)) else {
            return Ok(0);
        };

        let clusters = self.clusters.read().await;

        let Some(&(cluster, cluster_end)) = clusters.get(cluster_index) else {
            return Ok(0);
        };
        let count = (cluster_end + 1 - cluster) as usize;

        let mut rest = match clusters.get(end_len_ci) {
            Some(&(end_len_cluster, end_len_cluster_end)) => {
                if end_len_cluster_end != cluster_end {
                    (count << cluster_shift) - offset_in_cluster
                } else {
                    let count = (end_len_cluster - cluster) as usize;
                    (count << cluster_shift) + end_len_oc - offset_in_cluster
                }
            }
            None => (count << cluster_shift) - offset_in_cluster,
        };
        // log::trace!("FatFile::read_at: rest {rest:#x} bytes can be read");

        let mut cluster_offset = self.fs.offset_from_cluster(cluster) as usize + offset_in_cluster;
        let mut read_len = 0;
        let device = self.fs.fat.device();
        loop {
            if rest == 0 || buffer.is_empty() {
                self.update_read().await;
                break Ok(read_len);
            }
            let len = rest.min(buffer[0].len());
            // log::trace!("FatFile::read_at: attempt to read {len:#x} bytes");

            let len = device
                .read_at(cluster_offset, &mut [&mut buffer[0][..len]])
                .await?;
            // log::trace!("FatFile::read_at: actual read {len:#x} bytes");

            cluster_offset += len;
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
        let ioslice_len = umio::ioslice_len(&buffer);
        // log::trace!("FatFile::write_at {offset:#x}, buffer len = {ioslice_len}");

        let cluster_shift = self.cluster_shift;
        let (cluster_index, offset_in_cluster) = self.decomp(offset);

        let mut clusters = self.clusters.write().await;

        let (cluster, count) = {
            let cluster = clusters.get(cluster_index).cloned();
            match cluster {
                Some((cluster, cluster_end)) => {
                    let count = (cluster_end + 1 - cluster) as usize;
                    (cluster, count)
                }
                None => {
                    let gap_count: u32 = (cluster_index - clusters.len()).try_into()?;
                    let times: u32 = (cluster_index
                        + ((ioslice_len + ((1 << cluster_shift) - 1)) >> cluster_shift)
                        - clusters.len())
                    .try_into()?;
                    let mut prev = clusters.last().map(|&(c, _)| c);

                    let mut allocated = 0;
                    loop {
                        let mut count = times - allocated;
                        // log::trace!(
                        //     "FatFile::write_at {self:p} remaining {count} clusters to allocate"
                        // );
                        let new = self.fs.alloc_cluster(prev, &mut count, false).await?;
                        let new_end = new + count - 1;

                        if let Some(&(_, old_end)) = clusters.last() {
                            if old_end + 1 == new {
                                for (_, end) in clusters.iter_mut().rev() {
                                    if *end != old_end {
                                        break;
                                    }
                                    *end = new_end;
                                }
                            }
                        } else if let Some(ref entry) = self.entry {
                            // No last entry means emptiness.
                            entry.lock().await.set_first_cluster(Some(new));
                        }

                        clusters.extend((new..(new + count)).zip(iter::repeat(new_end)));

                        allocated += count;
                        if allocated > gap_count || allocated >= times {
                            break (new, (allocated - gap_count) as usize);
                        }
                        prev = Some(new);
                    }
                }
            }
        };

        let mut cluster_offset = self.fs.offset_from_cluster(cluster) as usize + offset_in_cluster;
        let mut rest = (count << cluster_shift) - offset_in_cluster;
        let mut written_len = 0;
        let device = self.fs.fat.device();
        loop {
            if rest == 0 || buffer.is_empty() {
                self.update_write(offset as u32).await;
                break Ok(written_len);
            }
            let len = rest.min(buffer[0].len());
            // log::trace!("FatFile::write_at: attempt to write {len:#x} bytes");
            let len = device
                .write_at(cluster_offset, &mut [&buffer[0][..len]])
                .await?;
            // log::trace!("FatFile::write_at: actual wrote {len:#x} bytes");

            cluster_offset += len;
            offset += len;
            written_len += len;
            rest -= len;
            advance_slices(&mut buffer, len)
        }
    }

    async fn flush(&self) -> Result<(), Error> {
        self.flush().await
    }
}

#[async_trait]
impl<T: TimeProvider> Entry for FatFile<T> {
    async fn open(
        self: Arc<Self>,
        path: &Path,
        options: OpenOptions,
        _perm: Permissions,
    ) -> Result<(Arc<dyn Entry>, bool), Error> {
        if !path.as_str().is_empty() {
            return Err(ENOTDIR);
        }
        if options.contains(OpenOptions::DIRECTORY) {
            return Err(ENOTDIR);
        }
        // TODO: Check options & permissions
        Ok((self, false))
    }

    async fn metadata(&self) -> Metadata {
        Metadata {
            ty: FileType::FILE,
            len: self.len.load(SeqCst),
            offset: self.abs_start_pos().await.unwrap_or(u64::MAX),
            perm: Permissions::from_bits_truncate(self.perm.load(SeqCst)),
            block_size: 1 << self.cluster_shift,
            block_count: self.clusters.read().await.len(),
            times: Default::default(),
        }
    }

    async fn set_metadata(&self, metadata: SetMetadata) -> Result<(), Error> {
        if let Some(new_len) = metadata.len {
            self.truncate(new_len.try_into()?).await?;
        }
        if let Some(perm) = metadata.perm {
            self.perm.store(perm.bits(), SeqCst);
        }
        Ok(())
    }
}
impl<T: TimeProvider> IoPoll for FatFile<T> {}
