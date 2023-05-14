use core::{
    mem,
    pin::Pin,
    ptr::{self, NonNull},
    sync::atomic::Ordering::SeqCst,
};

use arsc_rs::Arsc;
use futures_util::Future;
use riscv::{
    asm::{sfence_vma, sfence_vma_all},
    register::{satp, satp::Mode::Sv39},
};
use rv39_paging::{LAddr, ID_OFFSET, PAGE_SHIFT};

use crate::Virt;

#[thread_local]
static mut CUR_VIRT: *const Virt = ptr::null();

pub fn set_virt(virt: Pin<Arsc<Virt>>) -> Option<impl Future<Output = ()> + Send + 'static> {
    let addr = unsafe { ptr::addr_of_mut!(**virt.root.as_ptr()) };

    virt.cpu_mask.fetch_or(1 << hart_id::hart_id(), SeqCst);
    let new = Arsc::into_raw(unsafe { Pin::into_inner_unchecked(virt) });
    let old = unsafe { mem::replace(&mut CUR_VIRT, new) };

    let ret = NonNull::new(old.cast_mut()).map(|old| unsafe { Arsc::from_raw(old.as_ptr()) });

    if old != new {
        let paddr = *LAddr::from(addr).to_paddr(ID_OFFSET);
        log::debug!("tlb::set_virt: {old:p} => {new:p}");
        unsafe {
            satp::set(Sv39, 0, paddr >> PAGE_SHIFT);
            sfence_vma_all()
        }
        if let Some(ref old) = ret {
            old.cpu_mask.fetch_and(!(1 << hart_id::hart_id()), SeqCst);
        }
    }
    ret.and_then(|ret| (Arsc::count(&ret) == 1).then_some(async move { ret.clear().await }))
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
