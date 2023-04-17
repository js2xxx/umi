use alloc::string::String;
use core::{mem, slice};

use bitflags::bitflags;
use ktime_core::Instant;

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
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

    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Metadata {
    pub ty: FileType,
    pub len: usize,
    pub perm: Permissions,
    pub last_access: Instant,
    pub last_modified: Instant,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct DirEntry {
    pub name: String,
    pub metadata: Metadata,
}

#[derive(Copy, PartialEq, Eq, Clone, Debug)]
pub enum SeekFrom {
    /// Sets the offset to the provided number of bytes.
    Start(usize),

    /// Sets the offset to the size of this object plus the specified number of
    /// bytes.
    ///
    /// It is possible to seek beyond the end of an object, but it's an error to
    /// seek before byte 0.
    End(isize),

    /// Sets the offset to the current position plus the specified number of
    /// bytes.
    ///
    /// It is possible to seek beyond the end of an object, but it's an error to
    /// seek before byte 0.
    Current(isize),
}

pub type IoSlice<'a> = &'a [u8];

pub type IoSliceMut<'a> = &'a mut [u8];

#[allow(clippy::len_without_is_empty)]
pub trait IoSliceExt {
    fn len(&self) -> usize;

    fn advance(&mut self, n: usize);
}

impl IoSliceExt for IoSlice<'_> {
    fn len(&self) -> usize {
        (**self).len()
    }

    fn advance(&mut self, n: usize) {
        if self.len() < n {
            panic!("advancing IoSlice beyond its length");
        }

        *self = &self[n..];
    }
}

impl IoSliceExt for IoSliceMut<'_> {
    fn len(&self) -> usize {
        (**self).len()
    }

    fn advance(&mut self, n: usize) {
        if self.len() < n {
            panic!("advancing IoSlice beyond its length");
        }

        *self = unsafe { slice::from_raw_parts_mut(self.as_mut_ptr().add(n), self.len() - n) };
    }
}

pub fn advance_slices(bufs: &mut &mut [impl IoSliceExt], n: usize) {
    // Number of buffers to remove.
    let mut remove = 0;
    // Total length of all the to be removed buffers.
    let mut accumulated_len = 0;
    for buf in bufs.iter() {
        if accumulated_len + buf.len() > n {
            break;
        } else {
            accumulated_len += buf.len();
            remove += 1;
        }
    }

    *bufs = &mut mem::take(bufs)[remove..];
    if bufs.is_empty() {
        assert!(
            n == accumulated_len,
            "advancing io slices beyond their length"
        );
    } else {
        bufs[0].advance(n - accumulated_len)
    }
}
