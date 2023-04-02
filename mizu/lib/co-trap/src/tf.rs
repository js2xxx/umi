use static_assertions::const_assert_eq;

#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct Tx {
    pub ra: usize,     // 12
    pub sp: usize,     // 13
    pub gp: usize,     // 14
    pub tp: usize,     // 15
    pub a: [usize; 8], // 16..24
    pub t: [usize; 7], // 24..31
}

#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct Gpr {
    pub s: [usize; 12], // 0..12
    pub tx: Tx,         // 12..31
}
const_assert_eq!(
    core::mem::size_of::<Gpr>(),
    core::mem::size_of::<usize>() * 31
);

#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct TrapFrame {
    pub gpr: Gpr,       // 0..31
    pub sepc: usize,    // 31
    pub sstatus: usize, // 32
    pub stval: usize,   // 33
    pub scause: usize,  // 34
}

impl TrapFrame {
    pub const fn syscall_arg<const N: usize>(&self) -> usize {
        self.gpr.tx.a[N]
    }

    pub fn set_syscall_ret(&mut self, ret: usize) {
        self.gpr.tx.a[0] = ret;
    }
}
