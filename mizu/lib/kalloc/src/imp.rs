use core::{
    alloc::{GlobalAlloc, Layout},
    ptr::{self, NonNull},
};

use buddy_system_allocator::Heap;
use spin::Mutex;

pub struct Allocator(Mutex<Heap<30>>);

#[derive(Debug, Default)]
pub struct Stat {
    pub total: usize,
    pub used: usize,
}

impl Allocator {
    pub const fn new() -> Self {
        Allocator(Mutex::new(Heap::new()))
    }

    pub fn stat(&self) -> Stat {
        ksync_core::critical(|| {
            let heap = self.0.lock();
            Stat {
                total: heap.stats_total_bytes(),
                used: heap.stats_alloc_actual(),
            }
        })
    }

    /// # Safety
    ///
    /// The function must be called only once during initialization
    pub unsafe fn init(&self, start: usize, len: usize) {
        ksync_core::critical(|| self.0.lock().init(start, len));
    }
}

unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let res = ksync_core::critical(|| self.0.lock().alloc(layout));
        res.map_or(ptr::null_mut(), NonNull::as_ptr)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if let Some(ptr) = NonNull::new(ptr) {
            ksync_core::critical(|| self.0.lock().dealloc(ptr, layout))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alloc_dealloc() {
        static mut SPACE: [u64; 256] = [0; 256];
        let layout = Layout::from_size_align(4, 8).unwrap();
        unsafe {
            let allocator = Allocator::new();
            assert!(allocator.alloc(layout).is_null());

            allocator.init(SPACE.as_ptr() as usize, SPACE.len() * 8);

            let ptr = allocator.alloc(layout);
            assert!(!ptr.is_null());
            allocator.dealloc(ptr, layout);
        }
    }
}
