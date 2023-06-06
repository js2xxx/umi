use core::sync::atomic::{AtomicU8, Ordering::Relaxed};

use co_trap::{fast_func, FastResult, TrapFrame, Tx};
use riscv::register::{
    scause::{self, Exception, Interrupt, Scause, Trap},
    sepc, sstatus, stval,
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
    use riscv::register::{fcsr, stvec, stvec::TrapMode};
    extern "C" {
        fn ktrap_entry();
    }
    stvec::write(ktrap_entry as _, TrapMode::Direct);

    fcsr::set_rounding_mode(fcsr::RoundingMode::RoundToNearestEven);
    fcsr::clear_flag(fcsr::Flag::DZ);
    fcsr::clear_flag(fcsr::Flag::OF);
    fcsr::clear_flag(fcsr::Flag::UF);
    fcsr::clear_flag(fcsr::Flag::NX);

    sstatus::set_fs(sstatus::FS::Off);
}

fast_func!();

const CLEAN: u8 = 0;
const DIRTY: u8 = 1;
const YIELD: u8 = 2;
const RESET: u8 = 3;

#[repr(C)]
pub struct Fp {
    regs: [u64; 32],

    state: AtomicU8,
}

impl Default for Fp {
    fn default() -> Self {
        Fp {
            regs: [0; 32],
            state: AtomicU8::new(YIELD),
        }
    }
}

impl Fp {
    pub fn copy(other: &Self) -> Self {
        Fp {
            regs: other.regs,
            state: AtomicU8::new(YIELD),
        }
    }

    fn enter_user(&self) {
        extern "C" {
            fn _load_fp(regs: *const [u64; 32]);
        }
        if self.state.swap(CLEAN, Relaxed) == YIELD {
            unsafe {
                sstatus::set_fs(sstatus::FS::Clean);
                _load_fp(&self.regs);
                sstatus::set_fs(sstatus::FS::Off);
            }
        }
    }

    fn leave_user(&self, sstatus: &mut usize) {
        let fs = (*sstatus & 0x6000) >> 13;
        *sstatus &= !0x6000;
        *sstatus |= 0x4000; // Set clean
        unsafe { sstatus::set_fs(sstatus::FS::Off) }
        if fs == sstatus::FS::Dirty as _ {
            self.state.store(DIRTY, Relaxed);
        }
    }

    pub fn mark_reset(&self) {
        self.state.store(RESET, Relaxed);
    }

    pub fn yield_now(&mut self) {
        extern "C" {
            fn _save_fp(regs: *mut [u64; 32]);
        }
        match self.state.swap(YIELD, Relaxed) {
            DIRTY => unsafe {
                sstatus::set_fs(sstatus::FS::Clean);
                _save_fp(&mut self.regs);
                sstatus::set_fs(sstatus::FS::Off);
            },
            RESET => self.regs.fill(0),
            _ => {}
        }
    }
}

scoped_tls::scoped_thread_local!(pub static FP: Fp);

pub fn yield_to_user(tf: &mut TrapFrame) -> (Scause, FastResult) {
    FP.with(|fp| {
        fp.enter_user();
        let ret = co_trap::yield_to_user(&mut *tf);
        fp.leave_user(&mut tf.sstatus);
        ret
    })
}
