use co_trap::{fast_func, Tx};
use riscv::register::scause;

pub type KTrapFrame = Tx;

#[cfg(target_arch = "riscv64")]
core::arch::global_asm!(include_str!("trap.S"));

#[no_mangle]
extern "C" fn ktrap_handler(_tf: &mut KTrapFrame) {
    match scause::read().cause() {
        scause::Trap::Interrupt(_) => todo!(),
        scause::Trap::Exception(_excep) => {
            todo!()
        }
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
