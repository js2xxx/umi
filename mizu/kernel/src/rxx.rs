#[cfg(not(feature = "test"))]
use core::arch::asm;

use rv39_paging::{table_1g, AddrExt, Attr, Entry, Level, PAddr, Table, ID_OFFSET};

#[no_mangle]
static BOOT_PAGES: Table = const {
    let low_start = config::KERNEL_START.round_down(Level::max());
    let high_start = low_start + ID_OFFSET;
    let delta = Level::max().page_size();

    table_1g![
        low_start => low_start, Attr::KERNEL_RWX;
        low_start + delta => low_start + delta, Attr::KERNEL_RWX;
        high_start => low_start, Attr::KERNEL_RWX;
        high_start + delta => low_start + delta, Attr::KERNEL_RWX;
    ]
};

#[cfg(not(feature = "test"))]
#[no_mangle]
unsafe extern "C" fn __rt_init(hartid: usize) {
    extern "C" {
        static mut _sbss: u32;
        static mut _ebss: u32;

        static mut _sdata: u32;
        static mut _edata: u32;

        static _sidata: u32;

        static _stdata: u32;
        static _etdata: u32;

        static mut _sheap: u32;
        static mut _eheap: u32;
    }
    if hartid == 0 {
        r0::zero_bss(&mut _sbss, &mut _ebss);
        r0::init_data(&mut _sdata, &mut _edata, &_sidata);
    }

    // Initialize TLS
    // SAFETY: `tp` is initialized in the `_start` function
    unsafe {
        let tp: usize;
        asm!("mv {0}, tp", out(reg) tp);

        let dst = tp as *mut u32;
        dst.copy_from_nonoverlapping(
            &_stdata,
            ((&_etdata) as *const u32).offset_from(&_stdata) as usize,
        );
    }

    // Disable interrupt in `ksync`.
    unsafe { ksync::disable() };

    // Init logger.
    unsafe { klog::init_logger(log::Level::Debug) };

    // Init the kernel heap.
    unsafe { kalloc::init(&mut _sheap, &mut _eheap) };

    unsafe {
        static mut A: usize = 12345;

        assert_eq!(A, 12345);
    }
    crate::main(hartid)
}

#[cfg(not(feature = "test"))]
#[naked]
#[no_mangle]
#[link_section = ".init"]
unsafe extern "C" fn _start() -> ! {
    asm!("
        csrw sie, 0
        csrw sip, 0

        // Set the boot page tables
        la t0, {BOOT_PAGES}
        srli t0, t0, 12
        li t1, 0x8000000000000000
        add t0, t0, t1
        csrw satp, t0

        // Load ID offset to jump to higher half
        li t3, {ID_OFFSET}
    
        // Set global pointer
        .option push
        .option norelax
        la gp, __global_pointer$
        add gp, gp, t3
        .option pop

        // Set thread pointer
        .option push
        .option norelax
        la tp, _stp
        lui t0, %hi(_tdata_size)
        add t0, t0, %lo(_tdata_size)
        mul t0, a0, t0
        add tp, tp, t0
        add tp, tp, t3
        .option pop

        // Set stack pointer
        la sp, _estack
        lui t0, %hi(_stack_size)
        add t0, t0, %lo(_stack_size)
        mul t0, a0, t0
        sub sp, sp, t0
        add sp, sp, t3

        mv s0, sp

        la t0, {_init}
        add t0, t0, t3
        jr t0
        ", 
        _init = sym __rt_init,
        BOOT_PAGES = sym BOOT_PAGES,
        ID_OFFSET = const ID_OFFSET,
        options(noreturn)
    )
}

#[cfg(not(feature = "test"))]
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    use sbi_rt::{Shutdown, SystemFailure};
    klog::print!(""); // MAGIC, test it before removing.
    log::error!("kernel {info}");
    sbi_rt::system_reset(Shutdown, SystemFailure);
    loop {
        unsafe { core::arch::asm!("wfi") }
    }
}
