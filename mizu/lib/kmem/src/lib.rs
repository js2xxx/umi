#![cfg_attr(not(test), no_std)]
#![feature(alloc_layout_extra)]
#![feature(result_option_inspect)]
#![feature(thread_local)]

extern crate alloc;

mod frame;
mod lru;
mod phys;
mod virt;

pub use rv39_paging::{
    Attr, AttrBuilder, LAddr, PAddr, CANONICAL_PREFIX, ID_OFFSET, PAGE_LAYOUT, PAGE_MASK,
    PAGE_SHIFT, PAGE_SIZE,
};

pub use self::{
    frame::{frames, init_frames, Arena},
    lru::LruCache,
    phys::{Frame, Phys, ZERO},
    virt::{unset_virt, Virt},
};
