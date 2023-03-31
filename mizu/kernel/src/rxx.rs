#[cfg(not(feature = "test"))]
use core::arch::asm;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};

use rv39_paging::{table_1g, AddrExt, Attr, Entry, Level, PAddr, Table, ID_OFFSET};

#[no_mangle]
static BOOT_PAGES: Table = const {
    let low_start = config::KERNEL_START_PHYS.round_down(Level::max());
    let high_start = config::KERNEL_START.round_down(Level::max());
    let delta = Level::max().page_size();

    table_1g![
        low_start => low_start, Attr::KERNEL_RWX;
        low_start + delta => low_start + delta, Attr::KERNEL_RWX;
        high_start => low_start, Attr::KERNEL_RWX;
        high_start + delta => low_start + delta, Attr::KERNEL_RWX;
    ]
};

static BSP_ID: AtomicUsize = AtomicUsize::new(0);
static GLOBAL_INIT: AtomicBool = AtomicBool::new(false);

pub fn bsp_id() -> usize {
    BSP_ID.load(Relaxed)
}

pub fn is_bsp(hartid: usize) -> bool {
    hartid == bsp_id()
}

#[cfg(not(feature = "test"))]
#[no_mangle]
unsafe extern "C" fn __rt_init(hartid: usize, payload: usize) {
    use core::sync::atomic::Ordering::Release;

    extern "C" {
        static mut _sbss: u32;
        static mut _ebss: u32;

        static _stdata: u32;
        static _etdata: u32;

        static mut _sheap: u32;
        static mut _eheap: u32;
    }

    if !GLOBAL_INIT.load(Relaxed) {
        r0::zero_bss(&mut _sbss, &mut _ebss);

        // Can't use cmpxchg here, because `init_data` will reinitialize it to zero.
        GLOBAL_INIT.store(true, Release);
        BSP_ID.store(hartid, Release);
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

    if is_bsp(hartid) {
        // Init logger.
        unsafe { klog::init_logger(log::Level::Debug) };

        // Init the kernel heap.
        unsafe { kalloc::init(&mut _sheap, &mut _eheap) };
    }

    // unsafe {
    //     static mut A: usize = 12345;

    //     assert_eq!(A, 12345);
    // }
    crate::main(hartid, payload)
}

#[cfg(not(feature = "test"))]
#[naked]
#[no_mangle]
#[link_section = ".init"]
unsafe extern "C" fn _start() -> ! {
    asm!("
        csrw sie, 0
        csrw sip, 0

        // Load ID offset to jump to higher half
        li t3, {ID_OFFSET}

        // Set the boot page tables
        la t0, {BOOT_PAGES}
        sub t0, t0, t3
        srli t0, t0, 12
        li t1, 0x8000000000000000
        add t0, t0, t1
        csrw satp, t0
        sfence.vma
    
        // Set global pointer
        .option push
        .option norelax
        la gp, __global_pointer$
        .option pop

        // Set thread pointer
        .option push
        .option norelax
        la tp, _stp
        la t0, _tdata_size
        mul t0, a0, t0
        add tp, tp, t0
        .option pop

        // Set stack pointer
        la sp, _sstack
        la t0, _stack_size
        addi t1, a0, 1
        mul t0, t1, t0
        add sp, sp, t0

        mv s0, sp

        la t0, {_init}
        jr t0
        ", 
        _init = sym __rt_init,
        BOOT_PAGES = sym BOOT_PAGES,
        ID_OFFSET = const ID_OFFSET,
        options(noreturn)
    )
}

#[cfg(any(target_arch = "riscv32", target_arch = "riscv64"))]
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    use sbi_rt::{Shutdown, SystemFailure};
    log::error!("kernel {info}");
    sbi_rt::system_reset(Shutdown, SystemFailure);
    loop {
        unsafe { core::arch::asm!("wfi") }
    }
}
