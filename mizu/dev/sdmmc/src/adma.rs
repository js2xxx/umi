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

    fn set(&mut self, addr: u64, len: u16, attr: Attr) {
        self.addr = addr;
        self.len = len;
        self.attr = attr;
        self._reserved = 0;
    }
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
        unsafe {
            &mut *(($x).table.base().to_laddr(ID_OFFSET)).cast::<[Descriptor; MAX_SEGMENTS]>()
        }
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
    /// `buf` must be alive until the transfer is complete.
    pub unsafe fn fill(&mut self, buf: &mut [u8], read: bool) -> usize {
        assert!(buf.len() <= Self::MAX_LEN);
        kmem::sync_dma_for_device(read, !read, LAddr::from_slice(&*buf));

        let table = as_mut!(self);
        let mut iter = table.iter_mut();
        let mut base = LAddr::new(buf.as_mut_ptr()).to_paddr(ID_OFFSET);

        let mut filled = 0;
        let mut count = 0;

        let align_offset = (ADDR_ALIGN - (*base & ADDR_MASK)) & ADDR_MASK;
        if align_offset > 0 {
            let len = align_offset.min(buf.len());
            if !read {
                self.bounce_buffer[..len].copy_from_slice(&buf[..len]);
            }
            let addr = self.bounce_buffer.base();

            let desc = iter.next().unwrap();
            desc.set(*addr as u64, len as u16, Attr::ACTION_XFER | Attr::VALID);
            kmem::sync_dma_for_device(read, !read, LAddr::from_slice(&self.bounce_buffer[..len]));
            // log::trace!("Set at {desc:p}: {desc:#x?}");

            filled += len;
            base += len;
            count += 1;
        }

        while filled + Descriptor::MAX_LEN <= buf.len() {
            let desc = iter.next().unwrap();
            desc.set(*base as u64, 0, Attr::ACTION_XFER | Attr::VALID);
            // log::trace!("Set at {desc:p}: {desc:#x?}");

            filled += Descriptor::MAX_LEN;
            base += Descriptor::MAX_LEN;
            count += 1;
        }

        if filled < buf.len() {
            let len = buf.len() - filled;

            let desc = iter.next().unwrap();
            desc.set(*base as u64, len as u16, Attr::ACTION_XFER | Attr::VALID);
            // log::trace!("Set at {desc:p}: {desc:#x?}");

            filled += len;
            // base += len;
            count += 1;
        }

        let desc = iter.next().unwrap();
        desc.set(0, 0, Attr::ACTION_NONE | Attr::END | Attr::VALID);
        // log::trace!("Set at {desc:p}: {desc:#x?}");
        count += 1;

        atomic::fence(SeqCst);
        kmem::sync_dma_for_device(true, true, LAddr::from_slice(&table[..count]));
        filled
    }

    /// # Safety
    ///
    /// `buf` must be the same as the last `fill`ed.
    pub unsafe fn extract(&mut self, buf: &mut [u8], read: bool) {
        kmem::sync_dma_for_cpu(read, !read, LAddr::from_slice(&*buf));
        atomic::fence(SeqCst);

        if !read {
            return;
        }
        let base = *LAddr::new(buf.as_mut_ptr()).to_paddr(ID_OFFSET);
        let align_offset = (ADDR_ALIGN - (base & ADDR_MASK)) & ADDR_MASK;
        if align_offset > 0 {
            let len = align_offset.min(buf.len());
            let src = &self.bounce_buffer[..len];

            kmem::sync_dma_for_cpu(read, !read, LAddr::from_slice(src));
            buf[..len].copy_from_slice(src);
        }
    }
}
