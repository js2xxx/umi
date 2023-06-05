use core::{
    num::NonZeroUsize,
    ops::Range,
    ptr,
    sync::atomic::{AtomicUsize, Ordering::*},
};

use rv39_paging::{LAddr, PageAlloc, PAGE_MASK, PAGE_SHIFT, PAGE_SIZE};
use static_assertions::const_assert_eq;

const COUNT_SHIFT: u32 = 5;
const MAX_COUNT: usize = 1 << (PAGE_SHIFT - COUNT_SHIFT);
const COUNT_MASK: usize = (MAX_COUNT - 1) << COUNT_SHIFT;

const ID_MASK: usize = (1 << COUNT_SHIFT) - 1;

const_assert_eq!(COUNT_MASK + ID_MASK, PAGE_MASK);

// `count` field in the composition is currently unused.
fn compose(addr: LAddr, count: usize, id: u16) -> usize {
    debug_assert!(count <= MAX_COUNT);

    (addr.val() & !PAGE_MASK) | (count << COUNT_SHIFT) | (id as usize & ID_MASK)
}

fn decompose(head: usize) -> (LAddr, usize, u16) {
    let addr = LAddr::from(head & !PAGE_MASK);
    let count = (head & COUNT_MASK) >> COUNT_SHIFT;
    let id = (head & ID_MASK) as u16;

    (addr, count, id)
}

#[derive(Clone, Copy)]
struct Node {
    next: *mut Node,
    count: usize,
}

impl Default for Node {
    fn default() -> Self {
        Node {
            next: ptr::null_mut(),
            count: 0,
        }
    }
}

pub struct Arena {
    head: AtomicUsize,
    top: AtomicUsize,
    base: LAddr,
    end: LAddr,

    count: AtomicUsize,
}

impl Arena {
    /// Creates a new [`Arena`].
    ///
    /// # Safety
    ///
    /// `range` must contains a bunch of valid free pages.
    pub unsafe fn new(range: Range<LAddr>) -> Self {
        Arena {
            head: AtomicUsize::new(0),
            top: AtomicUsize::new(range.start.val()),
            base: range.start,
            end: range.end,
            count: AtomicUsize::new(0),
        }
    }
}

impl Arena {
    fn allocate_fresh(&self, count: NonZeroUsize) -> Option<LAddr> {
        let mut top = self.top.load(Acquire);
        loop {
            if !(self.base.val()..self.end.val())
                .contains(&top.wrapping_add((count.get() - 1) * PAGE_SIZE))
            {
                break None;
            }
            let next = top.wrapping_add(count.get() * PAGE_SIZE);
            match self.top.compare_exchange_weak(top, next, AcqRel, Acquire) {
                Ok(_) => break Some(LAddr::from(top)),
                Err(ptr) => top = ptr,
            }
        }
    }

    fn allocate_list(&self, count: NonZeroUsize) -> Option<LAddr> {
        let mut head = self.head.load(Acquire);
        loop {
            let (addr, _, id) = decompose(head);
            let ptr = match addr.as_non_null() {
                Some(ptr) => ptr.cast::<Node>(),
                None => break None,
            };

            let (next, nn, rest) = match unsafe { ptr.as_ref().count }.checked_sub(count.get()) {
                Some(rest) => unsafe {
                    let next = ptr.as_ref().next;
                    let nn = addr.add(count.get() * PAGE_SIZE);
                    (next, nn, rest)
                },
                None => break None,
            };
            let next_head = compose(next.into(), 0, id.wrapping_add(1));
            match self.head.compare_exchange(head, next_head, AcqRel, Acquire) {
                Ok(_) => {
                    if let Some(rest) = NonZeroUsize::new(rest) {
                        unsafe { self.deallocate_list(nn.into(), rest) }
                    }
                    break Some(addr);
                }
                Err(h) => head = h,
            }
        }
    }

    unsafe fn deallocate_list(&self, addr: LAddr, count: NonZeroUsize) {
        let mut next = self.head.load(Acquire);
        loop {
            let (next_addr, _, id) = decompose(next);
            addr.cast::<Node>().write(Node {
                next: next_addr.cast(),
                count: count.get(),
            });
            let head = compose(addr, 0, id.wrapping_add(1));
            match self.head.compare_exchange(next, head, AcqRel, Acquire) {
                Ok(_) => break,
                Err(h) => next = h,
            }
        }
    }

    pub fn allocate(&self, count: NonZeroUsize) -> Option<LAddr> {
        self.allocate_list(count)
            .or_else(|| self.allocate_fresh(count))
            .inspect(|addr| {
                log::trace!("frame allocation at {addr:?}, count = {count}");
                unsafe { addr.write_bytes(0, PAGE_SIZE) };
                self.count.fetch_add(count.get(), SeqCst);
            })
    }

    /// # Safety
    ///
    /// `addr` must contains `count` valid pages which is no longer used and was
    /// previous allocated by this arena.
    pub unsafe fn deallocate(&self, addr: LAddr, count: NonZeroUsize) {
        log::trace!("frame deallocation at {addr:?}, count = {count}");
        self.deallocate_list(addr, count);
        self.count.fetch_sub(count.get(), SeqCst);
    }

    pub fn used_count(&self) -> usize {
        self.count.load(Relaxed)
    }

    pub fn total_count(&self) -> usize {
        (self.end.val() - self.base.val()) >> PAGE_SHIFT
    }
}

unsafe impl PageAlloc for Arena {
    fn alloc(&self) -> Option<ptr::NonNull<rv39_paging::Table>> {
        self.allocate(NonZeroUsize::MIN)
            .map(|addr| addr.as_non_null().unwrap().cast())
    }

    unsafe fn dealloc(&self, ptr: ptr::NonNull<rv39_paging::Table>) {
        self.deallocate(ptr.into(), NonZeroUsize::MIN)
    }
}

static mut FRAMES: Option<Arena> = None;

pub fn frames() -> &'static Arena {
    unsafe { FRAMES.as_ref().expect("uninit frame allocator") }
}

/// # Safety
///
/// The function must be called only once during initialization.
pub unsafe fn init_frames(range: Range<LAddr>) {
    FRAMES = Some(Arena::new(range))
}

#[cfg(test)]
#[allow(dead_code)]
pub fn init_frames_for_test() {
    use std::sync::Once;

    static INIT: Once = Once::new();
    INIT.call_once(|| {
        #[repr(align(4096))]
        struct Memory([u8; PAGE_SIZE * 20]);

        let memory = Box::leak(Box::new(Memory([0; PAGE_SIZE * 20])));
        let range = memory.0.as_mut_ptr_range();
        // SAFETY: THe function is wrapped in `Once`.
        unsafe { init_frames(range.start.into()..range.end.into()) }
    })
}
