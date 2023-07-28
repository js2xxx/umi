#![cfg_attr(not(test), no_std)]
#![feature(alloc_layout_extra)]
#![feature(result_option_inspect)]
#![feature(thread_local)]

extern crate alloc;

mod frame;
#[cfg(feature = "cv1811h")]
mod insn;
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
    virt::{unset_virt, Virt, VirtCommitGuard},
};

pub fn sync_dma_for_cpu(from_device: bool, to_device: bool, range: core::ops::Range<LAddr>) {
    #[cfg(not(feature = "cv1811h"))]
    let _ = (from_device, to_device, range);
    #[cfg(feature = "cv1811h")]
    if from_device {
        let _ = to_device;
        insn::cmo_flush(range)
    }
}

pub fn sync_dma_for_device(from_device: bool, to_device: bool, range: core::ops::Range<LAddr>) {
    #[cfg(not(feature = "cv1811h"))]
    let _ = (from_device, to_device, range);
    #[cfg(feature = "cv1811h")]
    match (from_device, to_device) {
        (true, false) | (false, true) => insn::cmo_clean(range),
        (true, true) => insn::cmo_flush(range),
        _ => {}
    }
}
