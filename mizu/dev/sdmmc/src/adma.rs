use core::{
    mem,
    sync::atomic::{self, Ordering::SeqCst},
};

use bitflags::bitflags;
use kmem::{Frame, LAddr, PAddr, ID_OFFSET, PAGE_SIZE};
use ksc::Error;
use static_assertions::const_assert_eq;

pub const MAX_SEGMENTS: usize = 256;

pub const ADDR_ALIGN: usize = 4;
pub const ADDR_MASK: usize = ADDR_ALIGN - 1;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[repr(C, packed)]
pub struct Descriptor {
    attr: Attr,
    len: u16,
    addr: u64,
    _reserved: u32,
}
const_assert_eq!(mem::size_of::<Descriptor>(), mem::size_of::<u128>());

bitflags! {
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
    pub struct Attr: u16 {
        const VALID = 1 << 0;
        const END = 1 << 1;
        const GEN_INTR = 1 << 2;

        const ACTION_NONE = 0;
        const ACTION_RSVD = 0b010_000;
        const ACTION_XFER = 0b100_000;
        const ACTION_LINK = 0b110_000;

        const ACTION_MASK = 0b111_000;
    }
}

impl Descriptor {
    const MAX_LEN: usize = 1 << 16;
}

#[derive(Debug)]
pub struct DescTable {
    table: Frame,
    bounce_buffer: Frame,
}
const_assert_eq!(PAGE_SIZE, mem::size_of::<[Descriptor; MAX_SEGMENTS]>());

unsafe impl Send for DescTable {}
unsafe impl Sync for DescTable {}

macro_rules! as_mut {
    ($x:expr) => {
        unsafe { &mut *($x).table.as_mut_ptr().cast::<[Descriptor; MAX_SEGMENTS]>() }
    };
}

impl DescTable {
    pub const MAX_LEN: usize = (MAX_SEGMENTS - 2) * Descriptor::MAX_LEN;

    pub fn new() -> Result<Self, Error> {
        Ok(DescTable {
            table: Frame::new()?,
            bounce_buffer: Frame::new()?,
        })
    }

    pub fn dma_addr(&self) -> PAddr {
        self.table.base()
    }

    /// # Safety
    ///
    /// `buf` must be alive until the trasfer is complete.
    pub unsafe fn fill(&mut self, buf: &mut [u8], write: bool) -> usize {
        assert!(buf.len() <= Self::MAX_LEN);

        let mut table = as_mut!(self).iter_mut();
        let mut base = *LAddr::new(buf.as_mut_ptr()).to_paddr(ID_OFFSET);

        let mut filled = 0;

        let align_offset = (ADDR_ALIGN - (base & ADDR_MASK)) & ADDR_MASK;
        if align_offset > 0 {
            let len = align_offset.min(buf.len());
            if write {
                self.bounce_buffer[..len].copy_from_slice(&buf[..len]);
            }

            let desc = table.next().unwrap();
            desc.addr = *self.bounce_buffer.base() as u64;
            desc.len = len as u16;
            desc.attr = Attr::ACTION_XFER | Attr::VALID;

            filled += len;
            base += len;
        }

        while filled + Descriptor::MAX_LEN <= buf.len() {
            let desc = table.next().unwrap();
            desc.addr = base as u64;
            desc.len = 0;
            desc.attr = Attr::ACTION_XFER | Attr::VALID;

            filled += Descriptor::MAX_LEN;
            base += Descriptor::MAX_LEN;
        }

        if filled < buf.len() {
            let len = buf.len() - filled;

            let desc = table.next().unwrap();
            desc.addr = base as u64;
            desc.len = len as u16;
            desc.attr = Attr::ACTION_XFER | Attr::VALID;

            filled += len;
            // base += len;
        }

        let desc = table.next().unwrap();
        desc.addr = 0;
        desc.len = 0;
        desc.attr = Attr::ACTION_NONE | Attr::END | Attr::VALID;

        atomic::fence(SeqCst);
        filled
    }

    /// # Safety
    ///
    /// `buf` must be the same as the last `fill`ed.
    pub unsafe fn extract(&mut self, buf: &mut [u8], read: bool) {
        atomic::fence(SeqCst);
        if !read {
            return;
        }
        let base = *LAddr::new(buf.as_mut_ptr()).to_paddr(ID_OFFSET);
        let align_offset = (ADDR_ALIGN - (base & ADDR_MASK)) & ADDR_MASK;
        if align_offset > 0 {
            let len = align_offset.min(buf.len());
            buf[..len].copy_from_slice(&self.bounce_buffer[..len]);
        }
    }
}
