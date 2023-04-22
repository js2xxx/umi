use core::{mem, ptr, sync::atomic::Ordering::SeqCst};

use arsc_rs::Arsc;
use riscv::asm::{sfence_vma, sfence_vma_all};
use rv39_paging::{LAddr, PAGE_SHIFT};

use crate::Virt;

#[thread_local]
static mut CUR_VIRT: *const Virt = ptr::null();

pub fn set_virt(virt: Arsc<Virt>) -> bool {
    virt.cpu_mask.fetch_or(1 << hart_id::hart_id(), SeqCst);
    let new = Arsc::into_raw(virt);
    let old = unsafe { mem::replace(&mut CUR_VIRT, new) };

    let mut should_update_satp = false;
    if !old.is_null() {
        if old != new {
            should_update_satp = true;
        }
        let old = unsafe { Arsc::from_raw(old) };
        old.cpu_mask.fetch_and(!(1 << hart_id::hart_id()), SeqCst);
    }
    should_update_satp
}

pub fn flush(cpu_mask: usize, addr: LAddr, count: usize) {
    if count == 0 {
        return;
    }
    let others = cpu_mask & !(1 << hart_id::hart_id());
    if others != 0 {
        let _ = sbi_rt::remote_sfence_vma(cpu_mask, 0, addr.val(), count << PAGE_SHIFT);
    }
    unsafe {
        if count == 1 {
            sfence_vma(0, addr.val())
        } else {
            sfence_vma_all()
        }
    }
}
