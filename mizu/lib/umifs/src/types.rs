use alloc::string::String;

use bitflags::bitflags;
use ktime_core::Instant;
pub use umio::{
    advance_slices, ioslice_is_empty, ioslice_len, IoSlice, IoSliceExt, IoSliceMut, SeekFrom,
};

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
    pub struct OpenOptions: u32 {
        const ACCMODE   = 0o0000003;
        const RDONLY    = 0o0000000;
        const WRONLY    = 0o0000001;
        const RDWR      = 0o0000002;

        const CREAT     = 0o0000100;
        const EXCL      = 0o0000200;
        const NOCTTY    = 0o0000400;
        const TRUNC     = 0o0001000;
        const APPEND    = 0o0002000;
        const NONBLOCK  = 0o0004000;
        const DSYNC     = 0o0010000;
        const FASYNC    = 0o0020000;
        const DIRECT    = 0o0040000;
        const LARGEFILE = 0o0100000;
        const DIRECTORY = 0o0200000;
        const NOFOLLOW  = 0o0400000;
        const NOATIME   = 0o1000000;
        const CLOEXEC   = 0o2000000;
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
    pub struct FileType: u32 {
        const DIR  = 0o0040000;
        const CHR  = 0o0020000;
        const BLK  = 0o0060000;
        const REG  = 0o0100000;
        const FILE = Self::REG.bits();
        const IFO  = 0o0010000;
        const LNK  = 0o0120000;
        const SOCK = 0o0140000;
        const MT   = 0o0170000;
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
    pub last_access: Option<Instant>,
    pub last_modified: Option<Instant>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct DirEntry {
    pub name: String,
    pub metadata: Metadata,
}
