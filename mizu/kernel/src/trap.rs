use core::{
    sync::atomic::{AtomicU8, AtomicUsize, Ordering::Relaxed},
    time::Duration,
};

use co_trap::{fast_func, FastResult, TrapFrame, Tx};
use futures_util::Future;
use ksc::Error::{self, EAGAIN, ETIMEDOUT};
use ktime::TimeOutExt;
use riscv::register::{
    scause::{self, Exception, Interrupt, Scause, Trap},
    sepc, sstatus, stval,
};

pub type KTrapFrame = Tx;

pub fn poll_once<T>(fut: impl Future<Output = Result<T, Error>>) -> Result<T, Error> {
    match ksync::poll_once(fut) {
        Some(res) => res,
        None => Err(EAGAIN),
    }
}

pub async fn poll_with<T>(
    fut: impl Future<Output = Result<T, Error>>,
    timeout: Option<Duration>,
) -> Result<T, Error> {
    match timeout {
        None => fut.await,
        Some(Duration::ZERO) => poll_once(fut),
        Some(dur) => fut.on_timeout(dur, || Err(ETIMEDOUT)).await,
    }
}

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
                    "unhandled exception in kernel: {excep:?} at {:#x?}, stval = {:#x}",
                    sepc::read(),
                    stval::read(),
                )
            }
            _ => panic!(
                "unhandled exception in kernel: {excep:?} at {:#x}, stval = {:#x}",
                sepc::read(),
                stval::read(),
            ),
        },
    }
}

pub static TIMER_COUNT: AtomicUsize = AtomicUsize::new(0);

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
            TIMER_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
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
    /// 0 ~ 31 => f0 ~ f31, 32 => fcsr
    regs: [u64; 33],

    state: AtomicU8,
}

impl Default for Fp {
    fn default() -> Self {
        Fp {
            regs: [0; 33],
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
            fn _load_fp(regs: *const [u64; 33]);
        }
        if self.state.swap(CLEAN, Relaxed) == YIELD {
            unsafe { _load_fp(&self.regs) }
        }
    }

    fn leave_user(&self, sstatus: &mut usize) {
        let fs = (*sstatus & 0x6000) >> 13;
        if fs == sstatus::FS::Dirty as _ {
            self.state.store(DIRTY, Relaxed);
        }
    }

    pub fn mark_reset(&self) {
        self.state.store(RESET, Relaxed);
    }

    pub fn yield_now(&mut self) {
        extern "C" {
            fn _save_fp(regs: *mut [u64; 33]);
        }
        match self.state.swap(YIELD, Relaxed) {
            DIRTY => unsafe { _save_fp(&mut self.regs) },
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
