use core::{mem, ptr::NonNull};

use volatile::Volatile;

pub trait MmioReg {
    const OFFSET: usize;
    type Repr: Sized;
    type Access;

    unsafe fn at<'a>(base: NonNull<()>) -> Volatile<&'a mut Self::Repr, Self::Access> {
        mem::transmute(base.cast::<Self::Repr>())
    }
}

pub fn bitmap_index_u32(index: usize) -> (usize, u32) {
    let bit = index;
    let byte = bit / u32::BITS as usize;
    let bit_in_byte_mask = 1 << (bit % u32::BITS as usize);
    (byte, bit_in_byte_mask)
}
