use core::sync::atomic::{self, AtomicUsize, Ordering::*};

pub struct IpiComm {
    cmd: AtomicUsize,
    result: AtomicUsize,
}

const IPI_CMD_FENCE: usize = 1;

impl IpiComm {
    pub fn receive(&self) {
        #[cfg(any(target_arch = "riscv32", target_arch = "riscv64"))]
        unsafe {
            const SIE: usize = 1 << 1;
            core::arch::asm!("csrc sip, {}", in(reg) SIE);
        }

        let cmd = self.cmd.load(Acquire);
        if let IPI_CMD_FENCE = cmd {
            atomic::fence(SeqCst)
        }
        self.result.fetch_add(1, SeqCst);
    }

    fn send(&self, mask: usize, cmd: usize) {
        let count = mask.count_ones() as usize;
        self.cmd.store(cmd, Release);

        let ret = sbi_rt::send_ipi(mask, 0).into_result();
        if ret.is_ok() {
            loop {
                let cmpxchg = self.result.compare_exchange_weak(count, 0, AcqRel, Acquire);
                if cmpxchg.is_ok() {
                    break;
                }
            }
        }
    }

    pub fn remote_fence(&self, mask: usize) {
        let me = hart_id::hart_id();
        self.send(mask & !(1 << me), IPI_CMD_FENCE);
        if mask & (1 << me) != 0 {
            atomic::fence(SeqCst);
        }
    }
}

pub static IPI: IpiComm = IpiComm {
    cmd: AtomicUsize::new(0),
    result: AtomicUsize::new(0),
};
