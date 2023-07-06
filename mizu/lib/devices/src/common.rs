use core::{mem, ptr::NonNull};

use volatile::VolatilePtr;

pub trait MmioReg {
    const OFFSET: usize;
    type Repr: Sized;
    type Access;

    /// # Safety
    ///
    /// The caller must ensure the exclusive access of the given `Self::Access`
    /// at the given base during the given `'a` lifetime.
    unsafe fn at<'a>(base: NonNull<()>) -> VolatilePtr<'a, Self::Repr, Self::Access> {
        mem::transmute(base.as_ptr().byte_add(Self::OFFSET).cast::<Self::Repr>())
    }
}

pub fn bitmap_index_u32(index: usize) -> (usize, u32) {
    let bit = index;
    let byte = bit / u32::BITS as usize;
    let bit_in_byte_mask = 1 << (bit % u32::BITS as usize);
    (byte, bit_in_byte_mask)
}

#[derive(Debug, Clone, Copy)]
pub struct Token(pub u32);
