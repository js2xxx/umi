use alloc::string::String;

use bitflags::bitflags;
use ktime_core::Instant;
pub use umio::{
    advance_slices, ioslice_is_empty, ioslice_len, IoSlice, IoSliceExt, IoSliceMut, SeekFrom,
};

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
    pub struct OpenOptions: i32 {
        const ACCMODE =  0x0007;
        const EXEC    =  1;
        const RDONLY  =  2;
        const RDWR    =  3;
        const SEARCH  =  4;
        const WRONLY  =  5;

        const APPEND    = 0x000008;
        const CREAT     = 0x40;
        const EXCL      = 0x000040;
        const NOCTTY    = 0x000080;
        const NOFOLLOW  = 0x000100;
        const TRUNC     = 0x000200;
        const NONBLOCK  = 0x000400;
        const DSYNC     = 0x000800;
        const RSYNC     = 0x001000;
        const SYNC      = 0x002000;
        const CLOEXEC   = 0x004000;
        const PATH      = 0x008000;
        const LARGEFILE = 0x010000;
        const NOATIME   = 0x020000;
        const ASYNC     = 0x040000;
        const TMPFILE   = 0x080000;
        const DIRECT    = 0x100000;
        const DIRECTORY = 0x200000;
    }


    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
    pub struct Permissions: u32 {
        const SELF_R = 1;
        const SELF_W = 1 << 1;
        const SELF_X = 1 << 2;
        const GROUP_R = 1 << 3;
        const GROUP_W = 1 << 4;
        const GROUP_X = 1 << 5;
        const OTHERS_R = 1 << 6;
        const OTHERS_W = 1 << 7;
        const OTHERS_X = 1 << 8;
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct FileType: u8 {
        const UNKNOWN =	0;
        const FIFO = 1;
        const CHR = 2;
        const DIR = 4;
        const BLK = 6;
        const REG = 8;
        const FILE = Self::REG.bits();
        const LNK = 10;
        const SOCK = 12;
        const WHT = 14;
    }
}

impl FileType {
    pub const fn is(&self, ty: Self) -> bool {
        self.contains(ty)
    }
}

impl Permissions {
    pub fn all_same(readable: bool, writable: bool, executable: bool) -> Self {
        let mut ret = Permissions::empty();
        if readable {
            ret |= Permissions::SELF_R | Permissions::GROUP_R | Permissions::OTHERS_R;
        }
        if writable {
            ret |= Permissions::SELF_W | Permissions::GROUP_W | Permissions::OTHERS_W;
        }
        if executable {
            ret |= Permissions::SELF_X | Permissions::GROUP_X | Permissions::OTHERS_X;
        }
        ret
    }

    pub fn me(readable: bool, writable: bool, executable: bool) -> Self {
        let mut ret = Permissions::empty();
        if readable {
            ret |= Permissions::SELF_R;
        }
        if writable {
            ret |= Permissions::SELF_W;
        }
        if executable {
            ret |= Permissions::SELF_X;
        }
        ret
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Metadata {
    pub ty: FileType,
    pub len: usize,
    pub offset: u64,
    pub perm: Permissions,
    pub block_size: usize,
    pub block_count: usize,
    pub last_access: Option<Instant>,
    pub last_modified: Option<Instant>,
    pub last_created: Option<Instant>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct DirEntry {
    pub name: String,
    pub metadata: Metadata,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct FsStat {
    pub ty: &'static str,
    pub block_size: usize,
    pub block_count: usize,
    pub block_free: usize,
    pub file_count: usize,
}
