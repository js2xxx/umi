#![no_std]
#![feature(split_array)]

pub mod block;
pub mod net;

extern crate alloc;

use core::{num::NonZeroUsize, ptr::NonNull};

use kmem::{LAddr, PAddr, ID_OFFSET};
use virtio_drivers::{BufferDirection, Hal as VirtioHal};

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
