use core::arch::{asm, global_asm};

#[no_mangle]
unsafe extern "C" fn _init(hartid: usize, _: usize, _: usize) {
    extern "C" {
        static mut _sbss: u32;
        static mut _ebss: u32;

        static mut _sdata: u32;
        static mut _edata: u32;

        static _sidata: u32;

        static _stdata: u32;
        static _etdata: u32;
    }
    if hartid == 0 {
        r0::zero_bss(&mut _sbss, &mut _ebss);
        r0::init_data(&mut _sdata, &mut _edata, &_sidata);
    }

    // Initialize TLS
    unsafe {
        let tp: usize;
        asm!("mv {0}, tp", out(reg) tp);

        let dst = tp as *mut u32;
        dst.copy_from_nonoverlapping(
            &_stdata,
            ((&_etdata) as *const u32).offset_from(&_stdata) as usize,
        );
    }

    unsafe {
        static mut A: usize = 12345;

        assert_eq!(A, 12345);
    }
    crate::main(hartid)
}

global_asm! {
"
.section .init
.global _start
_start:
    .cfi_startproc

    csrw sie, 0
    csrw sip, 0
    
    .option push
    .option norelax
    la gp, __global_pointer$
    .option pop

    .option push
    .option norelax
    la tp, _stp
    lui t0, %hi(_tdata_size)
    add t0, t0, %lo(_tdata_size)
    mul t0, a0, t0
    add tp, tp, t0
    .option pop

    la sp, _estack
    lui t0, %hi(_stack_size)
    add t0, t0, %lo(_stack_size)
    mul t0, a0, t0
    sub sp, sp, t0

    mv s0, sp

    j _init

    .cfi_endproc
"
}

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        unsafe { core::arch::asm!("wfi") }
    }
}
