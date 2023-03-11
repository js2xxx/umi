use core::{
    alloc::{GlobalAlloc, Layout},
    ptr::{self, NonNull},
};

use buddy_system_allocator::Heap;
use spin::Mutex;

pub struct Allocator(Mutex<Heap<30>>);

impl Allocator {
    pub const fn new() -> Self {
        Allocator(Mutex::new(Heap::new()))
    }

    pub unsafe fn init(&self, start: usize, len: usize) {
        ksync::critical(|| self.0.lock().init(start, len));
    }
}

unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let res = ksync::critical(|| self.0.lock().alloc(layout));
        res.map_or(ptr::null_mut(), NonNull::as_ptr)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if let Some(ptr) = NonNull::new(ptr) {
            ksync::critical(|| self.0.lock().dealloc(ptr, layout))
        }
    }
}
