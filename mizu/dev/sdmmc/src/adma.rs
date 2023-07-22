use core::mem;

use bitflags::bitflags;
use kmem::{Frame, LAddr, PAddr, ID_OFFSET, PAGE_SIZE};
use ksc::Error;
use static_assertions::const_assert_eq;

pub const MAX_SEGMENTS: usize = 256;

pub const ADDR_ALIGN: usize = 4;
pub const ADDR_MASK: usize = ADDR_ALIGN - 1;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[repr(transparent)]
pub struct Descriptor(u128);

bitflags! {
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
    pub struct Attr: u8 {
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
    const ADDR_MASK: u128 = (u64::MAX as u128) << 32;
    const LEN_MASK: u128 = 0xffff_ffc0;

    const MAX_LEN: u32 = 1 << 26;

    // pub const fn addr(&self) -> u64 {
    //     (self.0 >> 32) as u64
    // }

    pub fn set_addr(&mut self, addr: u64) {
        self.0 &= !Self::ADDR_MASK;
        self.0 |= (addr as u128) << 32;
    }

    // pub const fn len(&self) -> u32 {
    //     let tmp = self.0 as u32;
    //     let raw = (tmp >> 16) | ((tmp & 0xffc0) << 10);
    //     if raw == 0 {
    //         Self::MAX_LEN
    //     } else {
    //         raw
    //     }
    // }

    pub fn set_len(&mut self, len: u32) {
        assert!(len <= Self::MAX_LEN);
        self.0 &= !Self::LEN_MASK;
        if len < Self::MAX_LEN {
            self.0 |= (((len & 0xffff) << 16) | ((len & 0x3ff0000) >> 10)) as u128;
        }
    }

    // pub fn attr(&self) -> Attr {
    //     Attr::from_bits_truncate(self.0 as u8)
    // }

    pub fn set_attr(&mut self, attr: Attr) {
        self.0 &= !(Attr::all().bits() as u128);
        self.0 |= attr.bits() as u128;
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
        unsafe { &mut *($x).table.as_mut_ptr().cast::<[Descriptor; MAX_SEGMENTS]>() }
    };
}

impl DescTable {
    pub const MAX_LEN: usize = (MAX_SEGMENTS - 2) * Descriptor::MAX_LEN as usize;

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
            desc.set_addr(*self.bounce_buffer.base() as u64);
            desc.set_len(len as u32);
            desc.set_attr(Attr::ACTION_XFER | Attr::VALID);

            filled += len;
            base += len;
        }

        while filled + Descriptor::MAX_LEN as usize <= buf.len() {
            let desc = table.next().unwrap();
            desc.set_addr(base as u64);
            desc.set_len(Descriptor::MAX_LEN);
            desc.set_attr(Attr::ACTION_XFER | Attr::VALID);

            filled += Descriptor::MAX_LEN as usize;
            base += Descriptor::MAX_LEN as usize;
        }

        if filled < buf.len() {
            let len = buf.len() - filled;

            let desc = table.next().unwrap();
            desc.set_addr(base as u64);
            desc.set_len(len as u32);
            desc.set_attr(Attr::ACTION_XFER | Attr::VALID);

            filled += len;
            // base += len;
        }

        let desc = table.next().unwrap();
        desc.set_attr(Attr::ACTION_NONE | Attr::END | Attr::VALID);

        filled
    }

    /// # Safety
    ///
    /// `buf` must be the same as the last `fill`ed.
    pub unsafe fn extract(&mut self, buf: &mut [u8], read: bool) {
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
