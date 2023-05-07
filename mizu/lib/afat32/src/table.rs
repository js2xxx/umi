use alloc::sync::Arc;
use core::{
    fmt, mem,
    ops::{Bound, Range, RangeBounds},
};

use futures_util::{future::try_join_all, stream, FutureExt, Stream, StreamExt, TryStreamExt};
use ksc_core::Error::{self, EINVAL, ENOSPC};
use umifs::traits::{Io, IoExt};

use crate::raw::BiosParameterBlock;

pub const RESERVED_FAT_ENTRIES: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FatEntry {
    Free,
    Next(u32),
    Bad,
    End,
}

impl FatEntry {
    pub fn from_raw(raw: u32, cluster: u32) -> Self {
        match raw & 0x0fff_ffff {
            0 if (0x0FFF_FFF7..=0x0FFF_FFFF).contains(&cluster) => {
                let tmp = if cluster == 0x0FFF_FFF7 {
                    "BAD_CLUSTER"
                } else {
                    "end-of-chain"
                };
                log::warn!(
                    "cluster number {} is a special value in FAT to indicate {}; it should never be seen as free",
                    cluster, tmp
                );
                FatEntry::Bad // avoid accidental use or allocation into a FAT
                              // chain
            }
            0 => FatEntry::Free,
            0x0FFF_FFF7 => FatEntry::Bad,
            0x0FFF_FFF8..=0x0FFF_FFFF => FatEntry::End,
            n if (0x0FFF_FFF7..=0x0FFF_FFFF).contains(&cluster) => {
                let tmp = if cluster == 0x0FFF_FFF7 {
                    "BAD_CLUSTER"
                } else {
                    "end-of-chain"
                };
                log::warn!("cluster number {} is a special value in FAT to indicate {}; hiding potential FAT chain value {} and instead reporting as a bad sector", cluster, tmp, n);
                FatEntry::Bad // avoid accidental use or allocation into a FAT
                              // chain
            }
            n => FatEntry::Next(n),
        }
    }

    pub fn into_raw(self, cluster: u32, old_raw: u32) -> u32 {
        if self == FatEntry::Free && (0x0FFF_FFF7..=0x0FFF_FFFF).contains(&cluster) {
            // NOTE: it is technically allowed for them to store FAT chain loops,
            //       or even have them all store value '4' as their next cluster.
            //       Some believe only FatEntry::Bad should be allowed for this edge case.
            let tmp = if cluster == 0x0FFF_FFF7 {
                "BAD_CLUSTER"
            } else {
                "end-of-chain"
            };
            panic!(
                "cluster number {} is a special value in FAT to indicate {}; it should never be set as free",
                cluster, tmp
            );
        };
        let raw = match self {
            FatEntry::Free => 0,
            FatEntry::Bad => 0x0FFF_FFF7,
            FatEntry::End => 0x0FFF_FFFF,
            FatEntry::Next(n) => n,
        };
        old_raw | raw
    }
}

pub struct Fat {
    device: Arc<dyn Io>,
    start_offset: usize,
    cluster_count: u32,
    mirrors: u8,
}

impl fmt::Debug for Fat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Fat")
            .field("start_offset", &self.start_offset)
            .field("cluster_count", &self.cluster_count)
            .field("mirrors", &self.mirrors)
            .finish()
    }
}

impl Fat {
    const ENTRY_SIZE: usize = mem::size_of::<u32>();

    pub fn new(device: Arc<dyn Io>, bpb: &BiosParameterBlock) -> Self {
        let sectors_per_fat = bpb.sectors_per_fat();
        let mirroring_enabled = bpb.mirroring_enabled();
        let (fat_first_sector, mirrors) = if mirroring_enabled {
            (u32::from(bpb.reserved_sectors), bpb.fats)
        } else {
            let active_fat = u32::from(bpb.active_fat());
            let fat_first_sector = u32::from(bpb.reserved_sectors) + active_fat * sectors_per_fat;
            (fat_first_sector, 1)
        };
        Fat {
            device,
            start_offset: bpb.bytes_from_sectors(fat_first_sector) as usize,
            cluster_count: bpb.total_clusters(),
            mirrors,
        }
    }

    pub fn device(&self) -> &Arc<dyn Io> {
        &self.device
    }

    pub const fn size(&self) -> usize {
        self.cluster_count as usize * Self::ENTRY_SIZE
    }

    pub const fn cluster_count(&self) -> u32 {
        self.cluster_count
    }

    pub const fn allocable_range(&self) -> Range<u32> {
        RESERVED_FAT_ENTRIES..(self.cluster_count + RESERVED_FAT_ENTRIES)
    }

    fn offset(&self, mirror: u8, cluster: u32) -> usize {
        self.start_offset + self.size() * mirror as usize + cluster as usize * Self::ENTRY_SIZE
    }

    async fn get_raw(&self, cluster: u32) -> Result<u32, Error> {
        let mut buf = [0; 4];
        if cluster >= self.allocable_range().end {
            return Err(EINVAL);
        }
        self.device
            .read_exact_at(self.offset(0, cluster), &mut buf)
            .await?;

        Ok(u32::from_le_bytes(buf))
    }

    pub async fn get(&self, cluster: u32) -> Result<FatEntry, Error> {
        self.get_raw(cluster)
            .await
            .map(|raw| FatEntry::from_raw(raw, cluster))
    }

    pub async fn set(&self, cluster: u32, entry: FatEntry) -> Result<(), Error> {
        let old = self.get_raw(cluster).await? & 0xf000_0000;
        let raw = entry.into_raw(cluster, old);

        let buffer = &raw.to_le_bytes();
        try_join_all((0..self.mirrors).map(|mirror| async move {
            let offset = self.offset(mirror, cluster);
            self.device.write_all_at(offset, buffer).await
        }))
        .await?;

        Ok(())
    }

    async fn find_free<R>(&self, cluster_range: R) -> Result<u32, Error>
    where
        R: RangeBounds<u32>,
    {
        let allocable_range = self.allocable_range();

        let start = match cluster_range.start_bound() {
            Bound::Included(&bound) => bound.max(allocable_range.start),
            Bound::Excluded(&bound) => bound.wrapping_add(1).max(allocable_range.start),
            Bound::Unbounded => allocable_range.end,
        };
        let end = match cluster_range.end_bound() {
            Bound::Included(&bound) => bound.wrapping_add(1).min(allocable_range.end),
            Bound::Excluded(&bound) => bound.min(allocable_range.end),
            Bound::Unbounded => allocable_range.end,
        };

        let iter = (start..end).map(|cluster| async move {
            match self.get(cluster).await {
                Ok(entry) if entry == FatEntry::Free => Err(Ok(cluster)),
                Ok(_) => Ok(()),
                Err(err) => Err(Err(err)),
            }
        });
        match try_join_all(iter).await {
            Ok(_) => Err(ENOSPC),
            Err(res) => res,
        }
    }

    pub async fn count_free(&self) -> usize {
        let stream = stream::iter(self.allocable_range())
            .filter(|&cluster| self.get(cluster).map(|res| res.unwrap() == FatEntry::Free));
        stream.count().await
    }

    pub async fn allocate(&self, prev: Option<u32>, hint: Option<u32>) -> Result<u32, Error> {
        let hint = hint.unwrap_or(self.allocable_range().start);

        let ret = match self.find_free(hint..).await {
            Ok(cluster) => cluster,
            Err(ENOSPC) => self.find_free(..hint).await?,
            Err(err) => return Err(err),
        };

        self.set(ret, FatEntry::End).await?;
        if let Some(prev) = prev {
            self.set(prev, FatEntry::Next(ret)).await?;
        }
        Ok(ret)
    }

    async fn iter_next(&self, cluster: u32) -> Result<Option<u32>, Error> {
        Ok(match self.get(cluster).await? {
            FatEntry::Next(next) => Some(next),
            _ => None,
        })
    }

    pub fn cluster_chain(&self, start: u32) -> impl Stream<Item = Result<u32, Error>> + Send + '_ {
        stream::unfold((self, Some(Ok(start))), move |(this, cluster)| async move {
            Some(match cluster? {
                Ok(cluster) => {
                    let next = self.iter_next(cluster).await;
                    (Ok(cluster), (this, next.transpose()))
                }
                Err(err) => (Err(err), (this, None)),
            })
        })
    }

    pub async fn free(&self, chain_start: u32) -> Result<u32, Error> {
        self.cluster_chain(chain_start)
            .try_fold(0, |acc, cluster| async move {
                self.set(cluster, FatEntry::Free).await?;
                Ok(acc + 1)
            })
            .await
    }

    pub async fn truncate(&self, chain_start: u32) -> Result<u32, Error> {
        self.set(chain_start, FatEntry::End).await?;
        match self.iter_next(chain_start).await? {
            Some(next) => self.free(next).await,
            None => Ok(0),
        }
    }
}
