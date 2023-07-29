#[cfg(not(feature = "test"))]
use core::arch::asm;

use arsc_rs::Arsc;
use art::Executor;
use rv39_paging::{table_1g, AddrExt, Attr, Entry, Level, PAddr, Table, ID_OFFSET};
use spin::Once;
use static_assertions::const_assert_eq;

const_assert_eq!(config::KERNEL_START_PHYS + ID_OFFSET, config::KERNEL_START);

const fn boot_pages() -> Table {
    let low_start = config::KERNEL_START_PHYS.round_down(Level::max());
    let delta = Level::max().page_size();

    let addrs = [0, delta, delta * 2, delta * 3];

    table_1g![
        // The identity mappings.
        low_start => low_start, Attr::KERNEL_MEM;

        // The higher half mappings.
        ID_OFFSET + addrs[0] => addrs[0], Attr::KERNEL_DEV;
        ID_OFFSET + addrs[1] => addrs[1], Attr::KERNEL_DEV;
        ID_OFFSET + addrs[2] => addrs[2], Attr::KERNEL_MEM;
        ID_OFFSET + addrs[3] => addrs[3], Attr::KERNEL_MEM;
    ]
}
#[no_mangle]
static BOOT_PAGES: Table = const { boot_pages() };

pub const KERNEL_PAGES: Table = const {
    let delta = Level::max().page_size();
    let addrs = [0, delta, delta * 2, delta * 3];
    table_1g![
        ID_OFFSET + addrs[0] => addrs[0], Attr::KERNEL_DEV;
        ID_OFFSET + addrs[1] => addrs[1], Attr::KERNEL_DEV;
        ID_OFFSET + addrs[2] => addrs[2], Attr::KERNEL_MEM;
        ID_OFFSET + addrs[3] => addrs[3], Attr::KERNEL_MEM;
    ]
};

static EXECUTOR: Once<Arsc<Executor>> = Once::new();

#[track_caller]
pub fn executor() -> &'static Arsc<Executor> {
    EXECUTOR.get().unwrap()
}

#[cfg(not(feature = "test"))]
fn run_art(payload: usize) {
    use alloc::boxed::Box;

    type Payload = *mut Box<dyn FnOnce() + Send>;
    if hart_id::is_bsp() {
        log::debug!("Starting ART");
        let mut runners = Executor::start(config::MAX_HARTS, move |e| async move {
            EXECUTOR.call_once(|| e);
            crate::main(payload).await;
            EXECUTOR.get().unwrap().shutdown()
        });

        let me = runners.next().unwrap();
        for (id, runner) in config::HART_RANGE
            .filter(|&id| id != hart_id::bsp_id())
            .zip(runners)
        {
            log::debug!("Starting #{id}");

            let payload: Payload = Box::into_raw(Box::new(Box::new(runner)));

            let ret = sbi_rt::hart_start(id, config::KERNEL_START_PHYS, payload as usize);

            if let Some(err) = ret.err() {
                log::error!("failed to start hart {id} due to error {err:?}");
            }
        }
        me();
    } else {
        log::debug!("Running ART from #{}", hart_id::hart_id());

        let runner = payload as Payload;
        // SAFETY: The payload must come from the BSP.
        unsafe { Box::from_raw(runner)() };
    }
}

#[cfg(not(feature = "test"))]
#[no_mangle]
unsafe extern "C" fn __rt_init(hartid: usize, payload: usize) {
    use core::mem;

    use config::VIRT_END;
    use riscv::register::{sie, sstatus};
    use sbi_rt::{NoReason, Shutdown};
    use spin::Lazy;

    extern "C" {
        static mut _sbss: u32;
        static mut _ebss: u32;

        static _stdata: u32;
        static _tdata_size: u32;
        static _tbss_size: u32;

        static mut _sheap: u32;
        static mut _eheap: u32;

        static _end: u8;
    }

    let boot_hart = payload & (1 << 63) == 0;

    if boot_hart {
        r0::zero_bss(&mut _sbss, &mut _ebss);
    }

    // Initialize TLS
    // SAFETY: `tp` is initialized in the `_start` function
    unsafe {
        let tp: *mut u32;
        asm!("mv {0}, tp", out(reg) tp);

        let tdata_count = (&_tdata_size) as *const u32 as usize / mem::size_of::<u32>();
        tp.copy_from_nonoverlapping(&_stdata, tdata_count);
        let tbss_count = ((&_tbss_size) as *const u32 as usize + 8) / mem::size_of::<u32>();
        tp.add(tdata_count).write_bytes(0, tbss_count);
    }

    // Disable interrupt in `ksync`.
    unsafe { ksync::disable() };

    // Init default kernel trap handler.
    unsafe { crate::trap::init() };

    if boot_hart {
        // Init the kernel heap.
        unsafe { kalloc::init(&mut _sheap, &mut _eheap) };

        hart_id::init_bsp_id(hartid);

        // Init the frame allocator.
        unsafe {
            let range = (&_end as *const u8).into()..VIRT_END.into();
            kmem::init_frames(range)
        }

        // Init lazies.
        Lazy::force(&crate::syscall::SYSCALL);
        Lazy::force(&kmem::ZERO);

        unsafe { crate::dev::init_logger() };
    }
    hart_id::init_hart_id(hartid);

    unsafe {
        sie::set_sext();
        sie::set_stimer();
        sie::set_ssoft();
        sstatus::set_spie();
        sstatus::set_sum();

        ksync::enable(usize::MAX);
    }
    sbi_rt::set_timer(0);

    run_art(payload);

    unsafe { ksync::disable() };

    if hart_id::is_bsp() {
        sbi_rt::system_reset(Shutdown, NoReason);
    }
    loop {
        core::hint::spin_loop()
    }
}

#[cfg(not(feature = "test"))]
#[naked]
#[no_mangle]
unsafe extern "C" fn _fix_args() {
    #[cfg(not(feature = "cv1811h"))]
    asm!("ret", options(noreturn));
    #[cfg(feature = "cv1811h")]
    asm!("li a0, 0; li a1, 0; ret", options(noreturn));
}

#[cfg(not(feature = "test"))]
#[naked]
#[no_mangle]
#[link_section = ".init"]
unsafe extern "C" fn _start() -> ! {
    asm!("
        csrw sie, zero
        csrw sip, zero
        csrw satp, zero

        call {_fix_args}

        // Load ID offset to jump to higher half
        li t3, {ID_OFFSET}

        // Set the boot page tables
        la t0, {BOOT_PAGES}
        sub t0, t0, t3
        srli t0, t0, 12
        li t1, 8 << 60
        add t0, t0, t1
        sfence.vma
        csrw satp, t0
    
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
        la t1, _tbss_size
        add t0, t0, t1
        addi t0, t0, 8
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
        _fix_args = sym _fix_args,
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

#[macro_export]
macro_rules! tryb {
    ($expr:expr) => {
        match $expr {
            Ok(value) => value,
            Err(_) => return false,
        }
    };
}

#[macro_export]
macro_rules! someb {
    ($expr:expr) => {
        match $expr {
            Some(value) => value,
            None => return false,
        }
    };
}
