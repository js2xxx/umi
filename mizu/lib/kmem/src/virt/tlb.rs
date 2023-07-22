use core::{
    mem,
    ptr::{self, NonNull},
    sync::atomic::Ordering::SeqCst,
};

use arsc_rs::Arsc;
use riscv::{
    asm::{sfence_vma, sfence_vma_all},
    register::{satp, satp::Mode::Sv39},
};
use rv39_paging::{LAddr, PAddr, ID_OFFSET, PAGE_SHIFT};

use crate::Virt;

#[thread_local]
static mut CUR_VIRT: *const Virt = ptr::null();

pub fn set_virt(virt: Arsc<Virt>) {
    let addr = unsafe { (*virt.root.as_ptr()).as_mut_ptr() };

    virt.cpu_mask.fetch_or(1 << hart_id::hart_id(), SeqCst);
    let new = Arsc::into_raw(virt);
    let old = unsafe { mem::replace(&mut CUR_VIRT, new) };

    let ret = NonNull::new(old.cast_mut()).map(|old| unsafe { Arsc::from_raw(old.as_ptr()) });

    if old != new {
        let paddr = *LAddr::from(addr).to_paddr(ID_OFFSET);
        unsafe {
            satp::set(Sv39, 0, paddr >> PAGE_SHIFT);
            sfence_vma_all()
        }
        if let Some(old) = ret {
            log::debug!("tlb::set_virt: {:p} => {:p}", old.root.as_ptr(), addr);
            old.cpu_mask.fetch_and(!(1 << hart_id::hart_id()), SeqCst);
        } else {
            log::debug!("tlb::set_virt: K => {:p}", addr);
        }
    }
}

/// # Safety
///
/// The caller must ensure the validity of the page tables contained in
/// `default_pt`.
pub unsafe fn unset_virt(default_pt: PAddr) {
    let old = unsafe { mem::replace(&mut CUR_VIRT, ptr::null()) };

    let ret = NonNull::new(old.cast_mut()).map(|old| unsafe { Arsc::from_raw(old.as_ptr()) });

    unsafe {
        satp::set(Sv39, 0, *default_pt >> PAGE_SHIFT);
        sfence_vma_all()
    }
    if let Some(ref old) = ret {
        log::debug!("tlb::set_virt: {:p} => K", old.root.as_ptr());
        old.cpu_mask.fetch_and(!(1 << hart_id::hart_id()), SeqCst);
    }
}

pub fn flush(cpu_mask: usize, addr: LAddr, count: usize) {
    if count == 0 {
        return;
    }
    log::trace!("tlb::flush cpu_mask = {cpu_mask:#b}, addr = {addr:?}, count = {count}");
    let others = cpu_mask & !(1 << hart_id::hart_id());
    if others != 0 {
        let _ = sbi_rt::remote_sfence_vma(others, 0, addr.val(), count << PAGE_SHIFT);
    }
    if cpu_mask != others {
        unsafe {
            if count == 1 {
                sfence_vma(0, addr.val())
            } else {
                sfence_vma_all()
            }
        }
    }
}
