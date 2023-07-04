//! Based on [PLIC specification](https://github.com/riscv/riscv-plic-spec).

use core::{mem, ptr::NonNull};

use static_assertions::const_assert_eq;
use volatile::{
    access::{ReadOnly, ReadWrite},
    map_field,
};

use crate::dev::common::{bitmap_index_u32, MmioReg};

pub const NR_INTR: usize = 1024;
const NR_INTR_BY_BITS: usize = NR_INTR / u32::BITS as usize;
pub const NR_INTR_CX: usize = 15872;

type IntrValues = [u32; NR_INTR];
type IntrBitmap = [u32; NR_INTR_BY_BITS];

struct Priority;
impl MmioReg for Priority {
    const OFFSET: usize = 0;
    type Repr = IntrValues;
    type Access = ReadWrite;
}
const_assert_eq!(mem::size_of::<<Priority as MmioReg>::Repr>(), 0x1000);

struct Pending;
impl MmioReg for Pending {
    const OFFSET: usize = 0x1000;
    type Repr = IntrBitmap;
    type Access = ReadOnly;
}
const_assert_eq!(mem::size_of::<<Pending as MmioReg>::Repr>(), 0x80);

struct Enable;
impl MmioReg for Enable {
    const OFFSET: usize = 0x2000;
    type Repr = [IntrBitmap; NR_INTR_CX];
    type Access = ReadWrite;
}
const_assert_eq!(mem::size_of::<<Enable as MmioReg>::Repr>(), 0x1f0000);

#[repr(C, align(0x1000))]
struct CxStruct {
    priority_threshold: u32,
    claim_complete: u32,
}
struct Cx;
impl MmioReg for Cx {
    const OFFSET: usize = 0x200000;
    type Repr = [CxStruct; NR_INTR_CX];
    type Access = ReadWrite;
}
const_assert_eq!(mem::size_of::<<Cx as MmioReg>::Repr>(), 0x3e00000);

macro_rules! map_index {
    ($volatile:ident, $index:expr) => {
        unsafe { $volatile.map(|ptr| core::ptr::NonNull::get_unchecked_mut(ptr, $index)) }
    };
}

#[derive(Debug, Clone)]
pub struct Plic(NonNull<()>);

unsafe impl Send for Plic {}
unsafe impl Sync for Plic {}

impl Plic {
    /// Creates a new [`Plic`] at a specified base.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the memory region at `base` sized 0x4000000
    /// is valid for exclusive, uncached read and write access in the `'static`
    /// lifetime.
    pub unsafe fn new(base: NonNull<()>) -> Self {
        Plic(base)
    }

    pub fn priority(&self, pin: u32) -> u32 {
        let cell = unsafe { Priority::at(self.0) };
        unsafe {
            cell.map(|s| NonNull::get_unchecked_mut(s, pin as usize))
                .read()
        }
    }

    pub fn set_priority(&self, pin: u32, priority: u32) {
        let cell = unsafe { Priority::at(self.0) };
        map_index!(cell, pin as usize).write(priority)
    }

    pub fn pending(&self, pin: u32) -> bool {
        let (byte, bit_in_byte_mask) = bitmap_index_u32(pin as usize);
        let cell = unsafe { Pending::at(self.0) };
        map_index!(cell, byte).read() & bit_in_byte_mask != 0
    }

    pub fn is_enabled(&self, pin: u32, cx: usize) -> bool {
        let (byte, bit_in_byte_mask) = bitmap_index_u32(pin as usize);
        let all_cell = unsafe { Enable::at(self.0) };
        let cx_cell = map_index!(all_cell, cx);
        map_index!(cx_cell, byte).read() & bit_in_byte_mask != 0
    }

    pub fn enable(&self, pin: u32, cx: usize, enable: bool) {
        log::trace!(
            "Plic::enable base = {:p}, pin = {pin}, cx = {cx}, {}",
            self.0,
            if enable { "enable" } else { "disable" }
        );

        let (byte, bit_in_byte_mask) = bitmap_index_u32(pin as usize);
        let all_cell = unsafe { Enable::at(self.0) };
        let cx_cell = map_index!(all_cell, cx);
        let cell = map_index!(cx_cell, byte);

        log::trace!(
            "Plic::enable byte get_unchecked_mut = {:#x}, bit_in_byte_mask = {:#b}, cell base = {:p}",
            byte,
            bit_in_byte_mask,
            cell.as_raw_ptr()
        );

        cell.update(|value| {
            if enable {
                value | bit_in_byte_mask
            } else {
                value & !bit_in_byte_mask
            }
        })
    }

    pub fn priority_threshold(&self, cx: usize) -> u32 {
        let cx_cell = unsafe { Cx::at(self.0) };
        let cell = map_index!(cx_cell, cx);
        map_field!(cell.priority_threshold).read()
    }

    pub fn set_priority_threshold(&self, cx: usize, threshold: u32) {
        let cx_cell = unsafe { Cx::at(self.0) };
        let cell = map_index!(cx_cell, cx);
        map_field!(cell.priority_threshold).write(threshold)
    }

    pub fn claim(&self, cx: usize) -> u32 {
        let cx_cell = unsafe { Cx::at(self.0) };
        let cell = map_index!(cx_cell, cx);
        map_field!(cell.claim_complete).read()
    }

    pub fn complete(&self, cx: usize, pin: u32) {
        let cx_cell = unsafe { Cx::at(self.0) };
        let cell = map_index!(cx_cell, cx);
        map_field!(cell.claim_complete).write(pin)
    }
}
