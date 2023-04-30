use core::{mem, num::NonZeroUsize, ptr::NonNull};

use rv39_paging::{LAddr, PAddr, ID_OFFSET};
use volatile::Volatile;

pub trait MmioReg {
    const OFFSET: usize;
    type Repr: Sized;
    type Access;

    /// # Safety
    ///
    /// The caller must ensure the exclusive access of the given `Self::Access`
    /// at the given base during the given `'a` lifetime.
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

pub use virtio_drivers::{BufferDirection, Hal as VirtioHal};

pub struct HalImpl;

unsafe impl VirtioHal for HalImpl {
    fn dma_alloc(pages: usize, _: BufferDirection) -> (virtio_drivers::PhysAddr, NonNull<u8>) {
        match NonZeroUsize::new(pages).and_then(|count| kmem::frames().allocate(count)) {
            Some(addr) => (*addr.to_paddr(ID_OFFSET), unsafe {
                addr.as_non_null_unchecked()
            }),
            None => (0, NonNull::dangling()),
        }
    }

    unsafe fn dma_dealloc(_: virtio_drivers::PhysAddr, ptr: NonNull<u8>, pages: usize) -> i32 {
        if let Some(count) = NonZeroUsize::new(pages) {
            let addr = LAddr::from(ptr);
            unsafe { kmem::frames().deallocate(addr, count) }
        }
        0
    }

    unsafe fn mmio_phys_to_virt(paddr: virtio_drivers::PhysAddr, _: usize) -> NonNull<u8> {
        let laddr = PAddr::new(paddr).to_laddr(ID_OFFSET);
        laddr.as_non_null().expect("invalid address")
    }

    unsafe fn share(buffer: NonNull<[u8]>, _: BufferDirection) -> virtio_drivers::PhysAddr {
        *LAddr::from(buffer).to_paddr(ID_OFFSET)
    }

    unsafe fn unshare(_: virtio_drivers::PhysAddr, _: NonNull<[u8]>, _: BufferDirection) {}
}
