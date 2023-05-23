use co_trap::{fast_func, Tx};
use riscv::register::{
    scause::{self, Exception, Interrupt, Trap},
    sepc, stval,
};

pub type KTrapFrame = Tx;

#[cfg(target_arch = "riscv64")]
core::arch::global_asm!(include_str!("trap.S"));

const TIMER_GRAN_DIV: u64 = 200;

#[no_mangle]
extern "C" fn ktrap_handler(_tf: &mut KTrapFrame) {
    match scause::read().cause() {
        Trap::Interrupt(intr) => handle_intr(intr, "kernel"),
        Trap::Exception(excep) => match excep {
            Exception::Breakpoint => sepc::write(sepc::read() + 2),
            Exception::LoadPageFault | Exception::StorePageFault => {
                if let Some(cf) = crate::mem::UA_FAULT.try_with(|&s| s) {
                    sepc::write(cf);
                    _tf.a[0] = stval::read();
                    return;
                }
                panic!(
                    "unhandled exception in kernel: {excep:?} at {:#x?}",
                    sepc::read()
                )
            }
            _ => panic!(
                "unhandled exception in kernel: {excep:?} at {:#x}",
                sepc::read()
            ),
        },
    }
}

pub fn handle_intr(intr: Interrupt, from: &str) {
    match intr {
        Interrupt::SupervisorTimer => {
            ktime::timer_tick();
            #[cfg(not(feature = "test"))]
            let raw = ktime::Instant::now_raw();
            #[cfg(feature = "test")]
            let raw = 0;
            // log::trace!("timer tick at {raw}");
            sbi_rt::set_timer(raw + config::TIME_FREQ as u64 / TIMER_GRAN_DIV);
        }
        Interrupt::SupervisorExternal => crate::dev::INTR.notify(hart_id::hart_id()),
        Interrupt::SupervisorSoft => crate::cpu::IPI.receive(),
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
