#![cfg_attr(not(feature = "test"), no_std)]
#![feature(once_cell)]
#![feature(result_option_inspect)]

mod imp;

pub use imp::{Allocator, Stat};

#[cfg(not(feature = "test"))]
#[global_allocator]
static GLOBAL_ALLOC: imp::Allocator = imp::Allocator::new();

/// Initialize the kernel heap
///
/// # Safety
///
/// - `end` must be greater than `start` in addresses and must be properly
///   aligned.
/// - Must be called only once at initialization.
#[cfg(not(feature = "test"))]
pub unsafe fn init<T: Copy>(start: &mut T, end: &mut T) {
    let start_ptr = (start as *mut T).cast();
    let end_ptr = (end as *mut T).cast::<u8>();
    let len = end_ptr.offset_from(start_ptr);
    GLOBAL_ALLOC.init(start_ptr as usize, len as usize)
}

pub fn stat() -> Stat {
    #[cfg(not(feature = "test"))]
    return GLOBAL_ALLOC.stat();
    #[cfg(feature = "test")]
    Default::default()
}
