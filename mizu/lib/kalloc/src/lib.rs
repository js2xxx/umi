#![cfg_attr(not(test), no_std)]
#![feature(once_cell)]

mod imp;

#[cfg(not(test))]
#[global_allocator]
static GLOBAL_ALLOC: imp::Allocator = imp::Allocator::new();

/// Initialize the kernel heap
///
/// # Safety
///
/// - `end` must be greater than `start` in addresses and must be properly
///   aligned.
/// - Must be called only once at initialization.
#[cfg(not(test))]
pub unsafe fn init<T: Copy>(start: &mut T, end: &mut T) {
    let start_ptr = (start as *mut T).cast();
    let end_ptr = (end as *mut T).cast::<u8>();
    let len = end_ptr.offset_from(start_ptr);
    GLOBAL_ALLOC.init(start_ptr as usize, len as usize)
}
