use alloc::{boxed::Box, sync::Arc, vec};
use core::{
    mem,
    sync::atomic::{AtomicU8, Ordering::SeqCst},
};

use arsc_rs::Arsc;
use async_trait::async_trait;
use ksc_core::Error::{self, ENOSYS};
use spin::RwLock;
use umifs::traits::{Entry, FileSystem, Io, IoExt};

use crate::{
    raw::{BiosParameterBlock, BootSector, FsInfoSector},
    table::{Fat, RESERVED_FAT_ENTRIES},
    FatDir, FatFile, TimeProvider,
};

/// A FAT volume statistics.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub struct FatStats {
    cluster_size: u32,
    total_clusters: u32,
    free_clusters: u32,
}

impl FatStats {
    /// Cluster size in bytes
    #[must_use]
    pub fn cluster_size(&self) -> u32 {
        self.cluster_size
    }

    /// Number of total clusters in filesystem usable for file allocation
    #[must_use]
    pub fn total_clusters(&self) -> u32 {
        self.total_clusters
    }

    /// Number of free clusters
    #[must_use]
    pub fn free_clusters(&self) -> u32 {
        self.free_clusters
    }
}

#[derive(Debug)]
pub struct FatFileSystem<T: TimeProvider> {
    pub(crate) fat: Fat,
    pub(crate) bpb: BiosParameterBlock,
    fs_info: RwLock<FsInfoSector>,
    current_status_flags: AtomicU8,

    pub(crate) time_provider: T,
}

impl<T: TimeProvider> FatFileSystem<T> {
    pub async fn new(
        device: Arc<dyn Io>,
        block_shift: u32,
        time_provider: T,
    ) -> Result<Arsc<Self>, Error> {
        let mut b0 = vec![0; 1 << block_shift];
        device.read_exact_at(0, &mut b0).await?;

        let (_, bs) = BootSector::parse(&b0)?;
        let bpb = bs.bpb;

        log::trace!("BPB: {bpb:#?}");

        if !bpb.is_fat32() {
            log::error!("Unsupported FAT file system; only FAT32 is supported");
            return Err(ENOSYS);
        }

        let fis = bpb.bytes_from_sectors(bpb.fs_info_sector());
        device.read_exact_at(fis as usize, &mut b0).await.unwrap();
        let (_, mut fis) = FsInfoSector::parse(&b0)?;

        log::trace!("FIS: {fis:#?}");

        fis.fix(bpb.total_clusters());

        Ok(Arsc::new(FatFileSystem {
            fat: Fat::new(device, &bpb),
            bpb,
            fs_info: RwLock::new(fis),
            current_status_flags: AtomicU8::new(bpb.status_flags().encode()),
            time_provider,
        }))
    }
}

impl<T: TimeProvider> FatFileSystem<T> {
    pub(crate) async fn alloc_cluster(
        &self,
        prev_cluster: Option<u32>,
        zero: bool,
    ) -> Result<u32, Error> {
        let hint = ksync::critical(|| self.fs_info.read().next_free_cluster);
        let cluster = self.fat.allocate(prev_cluster, hint).await?;
        if zero {
            write_zeros(
                &**self.fat.device(),
                self.offset_from_cluster(cluster) as usize,
                self.bpb.cluster_size() as usize,
            )
            .await?;
        }
        ksync::critical(|| {
            let mut fs_info = self.fs_info.write();
            fs_info.set_next_free_cluster(cluster + 1);
            fs_info.map_free_clusters(|n| n - 1);
        });
        Ok(cluster)
    }

    fn sector_from_cluster(&self, cluster: u32) -> u32 {
        self.bpb.first_data_sector()
            + self
                .bpb
                .sectors_from_clusters(cluster - RESERVED_FAT_ENTRIES)
    }

    pub(crate) fn offset_from_cluster(&self, cluster: u32) -> u64 {
        self.bpb
            .bytes_from_sectors(self.sector_from_cluster(cluster))
    }

    pub(crate) async fn truncate_cluster_chain(&self, cluster: u32) -> Result<(), Error> {
        let num_free = self.fat.truncate(cluster).await?;
        ksync::critical(|| {
            let mut fs_info = self.fs_info.write();
            fs_info.map_free_clusters(|n| n + num_free);
        });
        Ok(())
    }

    pub(crate) async fn free_cluster_chain(&self, cluster: u32) -> Result<(), Error> {
        let num_free = self.fat.free(cluster).await?;
        ksync::critical(|| {
            let mut fs_info = self.fs_info.write();
            fs_info.map_free_clusters(|n| n + num_free);
        });
        Ok(())
    }

    async fn flush_fs_info(&self) -> Result<(), Error> {
        let bytes = ksync::critical(|| {
            let mut fs_info = self.fs_info.write();
            let dirty = mem::replace(&mut fs_info.dirty, false);
            dirty.then(|| fs_info.to_bytes())
        });

        if let Some((prefix, suffix)) = bytes {
            let offset = self
                .bpb
                .bytes_from_sectors(u32::from(self.bpb.fs_info_sector));
            for b in [&prefix, &[0; 480], &suffix] as [&[u8]; 3] {
                self.fat.device().write_all_at(offset as usize, b).await?;
            }
        }
        Ok(())
    }

    pub(crate) async fn set_dirty_flag(&self, dirty: bool) -> Result<(), Error> {
        // Do not overwrite flags read from BPB on mount
        let mut flags = self.bpb.status_flags();
        flags.dirty |= dirty;
        // Check if flags has changed
        let current_flags = FsStatusFlags::load(&self.current_status_flags);
        if flags == current_flags {
            // Nothing to do
            return Ok(());
        }
        let encoded = flags.encode();
        // Note: only one field is written to avoid rewriting entire boot-sector which
        // could be dangerous Compute reserver_1 field offset and write new
        // flags
        let offset = 0x041;
        self.fat.device().write_all_at(offset, &[encoded]).await?;
        FsStatusFlags::store(&self.current_status_flags, flags);
        Ok(())
    }

    async fn recalc_free_clusters(&self) -> Result<u32, Error> {
        let free_cluster_count = u32::try_from(self.fat.count_free().await)?;
        ksync::critical(|| {
            let mut fs_info_sector = self.fs_info.write();
            fs_info_sector.set_free_cluster_count(free_cluster_count);
        });
        Ok(free_cluster_count)
    }

    pub async fn flush(&self) -> Result<(), Error> {
        self.flush_fs_info().await?;
        self.set_dirty_flag(false).await?;
        Ok(())
    }

    pub async fn root_dir(self: Arsc<Self>) -> Result<FatDir<T>, Error> {
        FatFile::new(self.clone(), Some(self.bpb.root_dir_first_cluster), None)
            .await
            .map(FatDir::new)
    }

    pub async fn stats(&self) -> Result<FatStats, Error> {
        let free_clusters_option = ksync::critical(|| self.fs_info.read().free_cluster_count);
        let free_clusters = if let Some(n) = free_clusters_option {
            n
        } else {
            self.recalc_free_clusters().await?
        };
        Ok(FatStats {
            cluster_size: self.bpb.cluster_size(),
            total_clusters: self.fat.cluster_count(),
            free_clusters,
        })
    }

    pub fn status(&self) -> FsStatusFlags {
        FsStatusFlags::load(&self.current_status_flags)
    }
}

#[async_trait]
impl<T: TimeProvider> FileSystem for FatFileSystem<T> {
    async fn root_dir(self: Arsc<Self>) -> Result<Arc<dyn Entry>, Error> {
        self.root_dir().await.map(|dir| Arc::new(dir) as _)
    }

    async fn flush(&self) -> Result<(), Error> {
        (*self).flush().await
    }
}

pub(crate) async fn write_zeros(
    disk: &dyn Io,
    mut start: usize,
    mut len: usize,
) -> Result<(), Error> {
    const ZEROS: [u8; 512] = [0_u8; 512];
    while len > 0 {
        let write_size = len.min(ZEROS.len());
        disk.write_all_at(start, &ZEROS[..write_size]).await?;
        start += write_size;
        len -= write_size;
    }
    Ok(())
}

/// A FAT volume status flags retrived from the Boot Sector and the allocation
/// table second entry.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub struct FsStatusFlags {
    pub(crate) dirty: bool,
    pub(crate) io_error: bool,
}

impl FsStatusFlags {
    /// Checks if the volume is marked as dirty.
    ///
    /// Dirty flag means volume has been suddenly ejected from filesystem
    /// without unmounting.
    #[must_use]
    pub fn dirty(&self) -> bool {
        self.dirty
    }

    /// Checks if the volume has the IO Error flag active.
    #[must_use]
    pub fn io_error(&self) -> bool {
        self.io_error
    }

    fn encode(self) -> u8 {
        let mut res = 0_u8;
        if self.dirty {
            res |= 1;
        }
        if self.io_error {
            res |= 2;
        }
        res
    }

    pub(crate) fn decode(flags: u8) -> Self {
        Self {
            dirty: flags & 1 != 0,
            io_error: flags & 2 != 0,
        }
    }

    pub fn load(slot: &AtomicU8) -> Self {
        Self::decode(slot.load(SeqCst))
    }

    pub fn store(slot: &AtomicU8, flags: FsStatusFlags) {
        slot.store(flags.encode(), SeqCst)
    }
}
