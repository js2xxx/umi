#![no_std]

pub use rand_chacha::rand_core;
use rand_chacha::rand_core::SeedableRng;

pub fn seed64() -> u64 {
    #[cfg(target_arch = "riscv64")]
    unsafe {
        let ret: u64;
        core::arch::asm!("rdcycle {}", out(reg) ret);
        ret
    }
    #[cfg(target_arch = "riscv32")]
    unsafe {
        loop {
            let (h1, low, high): (u32, u32, u32);
            core::arch::asm!(
                "rdcycleh {};
                rdcycle {}; 
                rdcycleh {}", 
                out(reg) h1,
                out(reg) low,
                out(reg) high
            );
            if h1 == high {
                break low as u64 | ((high as u64) << u32::BITS);
            }
        }
    }
    #[cfg(not(any(target_arch = "riscv32", target_arch = "riscv64")))]
    unimplemented!("not on RISC-V architecture")
}

pub fn seed(fill: &mut [u8]) {
    fill.chunks_mut(core::mem::size_of::<u64>())
        .for_each(|fill| {
            let ne_bytes = seed64().to_ne_bytes();
            let len = fill.len();
            fill.copy_from_slice(&ne_bytes[..len])
        })
}

pub type Rng = rand_chacha::ChaChaRng;

pub fn rng() -> Rng {
    Rng::from_seed({
        let mut seed = [0; 32];
        self::seed(&mut seed);
        seed
    })
}
