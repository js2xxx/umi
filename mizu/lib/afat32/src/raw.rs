use concat_arrays::concat_arrays;
use nom::{bytes, number, IResult};

use crate::{dirent::DIR_ENTRY_SIZE, fs::FsStatusFlags, table::RESERVED_FAT_ENTRIES};

fn take_byte_array<const N: usize>(mut input: &[u8]) -> IResult<&[u8], [u8; N]> {
    let data;
    (input, data) = bytes::streaming::take(N)(input)?;
    Ok((input, data.try_into().unwrap()))
}

#[derive(Default, Debug, Clone, Copy)]
pub struct BiosParameterBlock {
    pub bytes_per_sector: u16,
    pub sectors_per_cluster: u8,
    pub reserved_sectors: u16,
    pub fats: u8,
    pub root_entries: u16,
    pub total_sectors_16: u16,
    pub media: u8,
    pub sectors_per_fat_16: u16,
    pub sectors_per_track: u16,
    pub heads: u16,
    pub hidden_sectors: u32,
    pub total_sectors_32: u32,

    // Extended BIOS Parameter Block
    pub sectors_per_fat_32: u32,
    pub extended_flags: u16,
    pub fs_version: u16,
    pub root_dir_first_cluster: u32,
    pub fs_info_sector: u16,
    pub backup_boot_sector: u16,
    pub reserved_0: [u8; 12],
    pub drive_num: u8,
    pub reserved_1: u8,
    pub ext_sig: u8,
    pub volume_id: u32,
    pub volume_label: [u8; 11],
    pub fs_type_label: [u8; 8],
}

impl BiosParameterBlock {
    const RESERVED_0: usize = 12;
    const RESERVED_1: usize = 1;

    pub fn parse(mut input: &[u8]) -> IResult<&[u8], Self> {
        let mut bpb = Self::default();
        (input, bpb.bytes_per_sector) = number::streaming::le_u16(input)?;
        (input, bpb.sectors_per_cluster) = number::streaming::le_u8(input)?;
        (input, bpb.reserved_sectors) = number::streaming::le_u16(input)?;
        (input, bpb.fats) = number::streaming::le_u8(input)?;
        (input, bpb.root_entries) = number::streaming::le_u16(input)?;
        (input, bpb.total_sectors_16) = number::streaming::le_u16(input)?;
        (input, bpb.media) = number::streaming::le_u8(input)?;
        (input, bpb.sectors_per_fat_16) = number::streaming::le_u16(input)?;
        (input, bpb.sectors_per_track) = number::streaming::le_u16(input)?;
        (input, bpb.heads) = number::streaming::le_u16(input)?;
        (input, bpb.hidden_sectors) = number::streaming::le_u32(input)?;
        (input, bpb.total_sectors_32) = number::streaming::le_u32(input)?;

        if bpb.is_fat32() {
            (input, bpb.sectors_per_fat_32) = number::streaming::le_u32(input)?;
            (input, bpb.extended_flags) = number::streaming::le_u16(input)?;
            (input, bpb.fs_version) = number::streaming::le_u16(input)?;
            (input, bpb.root_dir_first_cluster) = number::streaming::le_u32(input)?;
            (input, bpb.fs_info_sector) = number::streaming::le_u16(input)?;
            (input, bpb.backup_boot_sector) = number::streaming::le_u16(input)?;
            (input, _) = bytes::streaming::take(Self::RESERVED_0)(input)?;
        }

        (input, bpb.drive_num) = number::streaming::le_u8(input)?;
        (input, _) = bytes::streaming::take(Self::RESERVED_1)(input)?;
        (input, bpb.ext_sig) = number::streaming::le_u8(input)?;
        (input, bpb.volume_id) = number::streaming::le_u32(input)?;

        (input, bpb.volume_label) = take_byte_array(input)?;
        (input, bpb.fs_type_label) = take_byte_array(input)?;

        if bpb.ext_sig == 0x29 {
            bpb.volume_id = 0;
            bpb.volume_label.fill(0);
            bpb.fs_type_label.fill(0);
        }

        Ok((input, bpb))
    }
    pub(crate) fn status_flags(&self) -> FsStatusFlags {
        FsStatusFlags::decode(self.reserved_1)
    }

    pub(crate) fn mirroring_enabled(&self) -> bool {
        self.extended_flags & 0x80 == 0
    }

    pub(crate) fn active_fat(&self) -> u16 {
        // The zero-based number of the active FAT is only valid if mirroring is
        // disabled.
        if self.mirroring_enabled() {
            0
        } else {
            self.extended_flags & 0x0F
        }
    }

    pub fn is_fat32(&self) -> bool {
        // because this field must be zero on FAT32, and
        // because it must be non-zero on FAT12/FAT16,
        // this provides a simple way to detect FAT32
        self.sectors_per_fat_16 == 0
    }

    pub fn sectors_per_fat(&self) -> u32 {
        if self.is_fat32() {
            self.sectors_per_fat_32
        } else {
            self.sectors_per_fat_16 as u32
        }
    }

    pub fn total_sectors(&self) -> u32 {
        if self.total_sectors_16 == 0 {
            self.total_sectors_32
        } else {
            self.total_sectors_16 as u32
        }
    }

    pub fn root_dir_sectors(&self) -> u32 {
        let root_dir_bytes = u32::from(self.root_entries) * DIR_ENTRY_SIZE;
        (root_dir_bytes + u32::from(self.bytes_per_sector) - 1) / u32::from(self.bytes_per_sector)
    }

    pub fn sectors_per_all_fats(&self) -> u32 {
        u32::from(self.fats) * self.sectors_per_fat()
    }

    pub fn first_data_sector(&self) -> u32 {
        let root_dir_sectors = self.root_dir_sectors();
        let fat_sectors = self.sectors_per_all_fats();
        self.reserved_sectors as u32 + fat_sectors + root_dir_sectors
    }

    pub fn total_clusters(&self) -> u32 {
        let total_sectors = self.total_sectors();
        let first_data_sector = self.first_data_sector();
        let data_sectors = total_sectors - first_data_sector;
        data_sectors / u32::from(self.sectors_per_cluster)
    }

    pub fn bytes_from_sectors(&self, sectors: u32) -> u64 {
        // Note: total number of sectors is a 32 bit number so offsets have to be 64 bit
        u64::from(sectors) * u64::from(self.bytes_per_sector)
    }

    pub fn sectors_from_clusters(&self, clusters: u32) -> u32 {
        // Note: total number of sectors is a 32 bit number so it should not overflow
        clusters * u32::from(self.sectors_per_cluster)
    }

    pub fn cluster_size(&self) -> u32 {
        u32::from(self.sectors_per_cluster) * u32::from(self.bytes_per_sector)
    }

    pub fn fs_info_sector(&self) -> u32 {
        u32::from(self.fs_info_sector)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct BootSector {
    bootjmp: [u8; 3],
    oem_name: [u8; 8],
    pub bpb: BiosParameterBlock,
    boot_code: [u8; 448],
}

impl BootSector {
    pub const BOOT_SIG: [u8; 2] = [0x55, 0xaa];

    pub fn parse(mut input: &[u8]) -> IResult<&[u8], Self> {
        let mut bs = BootSector::default();

        (input, bs.bootjmp) = take_byte_array(input)?;
        (input, bs.oem_name) = take_byte_array(input)?;
        (input, bs.bpb) = BiosParameterBlock::parse(input)?;
        if bs.bpb.is_fat32() {
            let (bc, _) = bs.boot_code.split_array_mut::<420>();
            (input, *bc) = take_byte_array(input)?;
        } else {
            (input, bs.boot_code) = take_byte_array(input)?;
        }
        (input, _) = bytes::streaming::tag(Self::BOOT_SIG)(input)?;

        Ok((input, bs))
    }
}

impl Default for BootSector {
    fn default() -> Self {
        Self {
            bootjmp: Default::default(),
            oem_name: Default::default(),
            bpb: Default::default(),
            boot_code: [0; 448],
        }
    }
}

#[derive(Default, Debug)]
pub struct FsInfoSector {
    pub(crate) free_cluster_count: Option<u32>,
    pub(crate) next_free_cluster: Option<u32>,
    pub(crate) dirty: bool,
}

impl FsInfoSector {
    const LEAD_SIG: u32 = 0x4161_5252;
    const STRUC_SIG: u32 = 0x6141_7272;
    const TRAIL_SIG: u32 = 0xAA55_0000;

    pub fn parse(mut input: &[u8]) -> IResult<&[u8], Self> {
        let mut fis = FsInfoSector::default();

        (input, _) = bytes::streaming::tag(Self::LEAD_SIG.to_le_bytes())(input)?;
        (input, _) = bytes::streaming::take(480usize)(input)?;
        (input, _) = bytes::streaming::tag(Self::STRUC_SIG.to_le_bytes())(input)?;

        let free_cluster_count;
        (input, free_cluster_count) = number::streaming::le_u32(input)?;
        fis.free_cluster_count = (free_cluster_count != u32::MAX).then_some(free_cluster_count);

        let next_free_cluster;
        (input, next_free_cluster) = number::streaming::le_u32(input)?;
        fis.next_free_cluster =
            (!matches!(next_free_cluster, u32::MAX | 0 | 1)).then_some(next_free_cluster);

        (input, _) = bytes::streaming::take(12usize)(input)?;
        (input, _) = bytes::streaming::tag(Self::TRAIL_SIG.to_le_bytes())(input)?;

        Ok((input, fis))
    }

    #[allow(clippy::drop_non_drop)]
    pub fn to_bytes(&self) -> ([u8; 4], [u8; 28]) {
        let prefix = Self::LEAD_SIG.to_le_bytes();
        let suffix = concat_arrays!(
            Self::STRUC_SIG.to_le_bytes(),
            self.free_cluster_count.unwrap_or(0xFFFF_FFFF).to_le_bytes(),
            self.next_free_cluster.unwrap_or(0xFFFF_FFFF).to_le_bytes(),
            [0; 12],
            Self::TRAIL_SIG.to_le_bytes()
        );
        (prefix, suffix)
    }

    pub fn fix(&mut self, total_clusters: u32) {
        let max_valid_cluster_number = total_clusters + RESERVED_FAT_ENTRIES;
        if let Some(n) = self.free_cluster_count {
            if n > total_clusters {
                log::warn!(
                    "invalid free_cluster_count ({}) in fs_info exceeds total cluster count ({})",
                    n,
                    total_clusters
                );
                self.free_cluster_count = None;
            }
        }
        if let Some(n) = self.next_free_cluster {
            if n > max_valid_cluster_number {
                log::warn!(
                    "invalid free_cluster_count ({}) in fs_info exceeds maximum cluster number ({})",
                    n, max_valid_cluster_number
                );
                self.next_free_cluster = None;
            }
        }
    }

    pub(crate) fn map_free_clusters(&mut self, map_fn: impl Fn(u32) -> u32) {
        if let Some(n) = self.free_cluster_count {
            self.free_cluster_count = Some(map_fn(n));
            self.dirty = true;
        }
    }

    pub(crate) fn set_next_free_cluster(&mut self, cluster: u32) {
        self.next_free_cluster = Some(cluster);
        self.dirty = true;
    }

    pub(crate) fn set_free_cluster_count(&mut self, free_cluster_count: u32) {
        self.free_cluster_count = Some(free_cluster_count);
        self.dirty = true;
    }
}
