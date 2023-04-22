use core::{mem, ptr, sync::atomic::Ordering::SeqCst};

use arsc_rs::Arsc;
use riscv::{
    asm::{sfence_vma, sfence_vma_all},
    register::{satp, satp::Mode::Sv39},
};
use rv39_paging::{LAddr, ID_OFFSET, PAGE_SHIFT};

use crate::Virt;

#[thread_local]
static mut CUR_VIRT: *const Virt = ptr::null();

pub fn set_virt(virt: Arsc<Virt>) {
    let addr = unsafe { ptr::addr_of_mut!(*virt.root.data_ptr()) };

    virt.cpu_mask.fetch_or(1 << hart_id::hart_id(), SeqCst);
    let new = Arsc::into_raw(virt);
    let old = unsafe { mem::replace(&mut CUR_VIRT, new) };

    if !old.is_null() && old != new {
        let paddr = *LAddr::from(addr).to_paddr(ID_OFFSET);
        unsafe {
            satp::set(Sv39, 0, paddr >> PAGE_SHIFT);
            sfence_vma_all()
        }
        let old = unsafe { Arsc::from_raw(old) };
        old.cpu_mask.fetch_and(!(1 << hart_id::hart_id()), SeqCst);
    }
}

pub fn flush(cpu_mask: usize, addr: LAddr, count: usize) {
    if count == 0 {
        return;
    }
    let others = cpu_mask & !(1 << hart_id::hart_id());
    if others != 0 {
        let _ = sbi_rt::remote_sfence_vma(others, 0, addr.val(), count << PAGE_SHIFT);
    }
    unsafe {
        if count == 1 {
            sfence_vma(0, addr.val())
        } else {
            sfence_vma_all()
        }
    }
}
