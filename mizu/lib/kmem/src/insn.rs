use core::{arch::asm, ops::Range};

use rv39_paging::LAddr;

const CACHE_SIZE: usize = 1 << 6;

// pub fn thead_inval(base: LAddr) {
//     unsafe { asm!(".insn r 0b1011, 0, 1, zero, {}, x6", in(reg) base.val()) }
// }

pub fn thead_clean(base: LAddr) {
    unsafe { asm!(".insn r 0b1011, 0, 1, zero, {}, x4", in(reg) base.val()) }
}

pub fn thead_flush(base: LAddr) {
    unsafe { asm!(".insn r 0b1011, 0, 1, zero, {}, x7", in(reg) base.val()) }
}

fn cmo<F: Fn(LAddr)>(range: Range<LAddr>, f: F) {
    let range = (range.start.val() & !(CACHE_SIZE - 1))..range.end.val();
    range.step_by(CACHE_SIZE).for_each(|addr| f(addr.into()))
}

// pub fn cmo_inval(range: Range<LAddr>) {
//     cmo(range, thead_inval)
// }

pub fn cmo_clean(range: Range<LAddr>) {
    cmo(range, thead_clean)
}

pub fn cmo_flush(range: Range<LAddr>) {
    cmo(range, thead_flush)
}
