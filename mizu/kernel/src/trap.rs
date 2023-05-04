use co_trap::{fast_func, Tx};
use riscv::register::{
    scause::{self, Exception, Interrupt, Trap},
    sepc,
};

pub type KTrapFrame = Tx;

#[cfg(target_arch = "riscv64")]
core::arch::global_asm!(include_str!("trap.S"));

const TIMER_GRAN_DIV: u64 = 200;

#[no_mangle]
extern "C" fn ktrap_handler(_tf: &mut KTrapFrame) {
    let cause = scause::read().cause();
    log::debug!("KTRAP {cause:?} at {:x}", sepc::read());
    match cause {
        Trap::Interrupt(intr) => handle_intr(intr, "kernel"),
        Trap::Exception(excep) => match excep {
            Exception::Breakpoint => sepc::write(sepc::read() + 2),
            _ => panic!("unhandled exception in kernel: {excep:?}"),
        },
    }
}

pub fn handle_intr(intr: Interrupt, from: &str) {
    match intr {
        Interrupt::SupervisorTimer => {
            ktime::timer_tick();
            let raw = ktime::Instant::now_raw();
            sbi_rt::set_timer(raw + config::TIME_FREQ as u64 / TIMER_GRAN_DIV);
        }
        Interrupt::SupervisorExternal => crate::dev::INTR.notify(hart_id::hart_id()),
        _ => log::info!("unhandled interrupt in {from}: {intr:?}"),
    }
}

#[cfg(not(feature = "test"))]
pub unsafe fn init() {
    use riscv::register::{stvec, stvec::TrapMode};
    extern "C" {
        fn ktrap_entry();
    }
    stvec::write(ktrap_entry as _, TrapMode::Direct);
}

fast_func!();
