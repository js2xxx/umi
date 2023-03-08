#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

use sbi_spec::{
    binary::SbiRet,
    srst::{EID_SRST, RESET_REASON_NO_REASON, RESET_TYPE_SHUTDOWN, SYSTEM_RESET},
};

#[cfg(not(test))]
extern crate panic_halt;

#[cfg_attr(not(test), riscv_rt::entry)]
fn main() -> ! {
    sbi_call(
        EID_SRST,
        SYSTEM_RESET,
        RESET_TYPE_SHUTDOWN,
        RESET_REASON_NO_REASON,
    );
    loop {
        core::hint::spin_loop()
    }
}

#[inline(always)]
fn sbi_call(extension: usize, function: usize, arg0: u32, arg1: u32) -> SbiRet {
    #[cfg(any(target_arch = "riscv32", target_arch = "riscv64"))]
    {
        let (error, value);
        unsafe {
            core::arch::asm!(
                "ecall",
                in("a0") arg0, in("a1") arg1,
                in("a6") function, in("a7") extension,
                lateout("a0") error, lateout("a1") value,
            )
        };
        SbiRet { error, value }
    }
    #[cfg(not(any(target_arch = "riscv32", target_arch = "riscv64")))]
    {
        let _ = (extension, function, arg0, arg1);
        unimplemented!("not RISC-V instruction set architecture")
    }
}
