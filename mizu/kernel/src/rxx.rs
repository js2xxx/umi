#[cfg(not(feature = "test"))]
use core::arch::asm;

use rv39_paging::{table_1g, AddrExt, Attr, Entry, Level, PAddr, Table, ID_OFFSET};
use static_assertions::const_assert_eq;

const_assert_eq!(config::KERNEL_START_PHYS + ID_OFFSET, config::KERNEL_START);
#[no_mangle]
pub static BOOT_PAGES: Table = const {
    let low_start = config::KERNEL_START_PHYS.round_down(Level::max());
    let delta = Level::max().page_size();

    let addrs = [0, delta, delta * 2, delta * 3];

    table_1g![
        // The temporary identity mappings.
        low_start => low_start, Attr::KERNEL_RWX;

        // The temporary higher half mappings.
        ID_OFFSET + addrs[0] => addrs[0], Attr::KERNEL_RWX;
        ID_OFFSET + addrs[1] => addrs[1], Attr::KERNEL_RWX;
        ID_OFFSET + addrs[2] => addrs[2], Attr::KERNEL_RWX;
        ID_OFFSET + addrs[3] => addrs[3], Attr::KERNEL_RWX;
    ]
};

#[cfg(not(feature = "test"))]
#[no_mangle]
unsafe extern "C" fn __rt_init(hartid: usize, payload: usize) {
    use core::{
        mem,
        sync::atomic::{
            AtomicBool,
            Ordering::{Relaxed, Release},
        },
    };

    use config::VIRT_END;

    static GLOBAL_INIT: AtomicBool = AtomicBool::new(false);

    extern "C" {
        static mut _sbss: u32;
        static mut _ebss: u32;

        static _stdata: u32;
        static _tdata_size: u32;

        static mut _sheap: u32;
        static mut _eheap: u32;

        static _end: u8;
    }

    if !GLOBAL_INIT.load(Relaxed) {
        r0::zero_bss(&mut _sbss, &mut _ebss);

        // Can't use cmpxchg here, because `zero_bss` will reinitialize it to zero.
        GLOBAL_INIT.store(true, Release);
        hart_id::init_bsp_id(hartid);
    }

    // Initialize TLS
    // SAFETY: `tp` is initialized in the `_start` function
    unsafe {
        let tp: *mut u32;
        asm!("mv {0}, tp", out(reg) tp);

        let len = (&_tdata_size) as *const u32 as usize;
        tp.copy_from_nonoverlapping(&_stdata, len / mem::size_of::<u32>());
        hart_id::init_hart_id(hartid);
    }

    // Disable interrupt in `ksync`.
    unsafe { ksync::disable() };

    // Init default kernel trap handler.
    unsafe { crate::trap::init() };

    if hart_id::is_bsp() {
        // Init logger.
        unsafe { klog::init_logger(log::Level::Debug) };

        // Init the kernel heap.
        unsafe { kalloc::init(&mut _sheap, &mut _eheap) };

        // Init the frame allocator.
        unsafe {
            let range = (&_end as *const u8).into()..VIRT_END.into();
            kmem::init_frames(range)
        }
    }

    crate::main(payload)
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
    log::error!("#{} kernel {info}", hart_id::hart_id());
    sbi_rt::system_reset(Shutdown, SystemFailure);
    loop {
        unsafe { core::arch::asm!("wfi") }
    }
}
