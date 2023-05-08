#![no_std]
#![feature(build_hasher_simple_hash_one)]
#![feature(const_trait_impl)]

use core::{
    fmt,
    hash::{BuildHasher, Hash},
    marker::Destruct,
};

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

/// A wrapper around `ahash::RandomState`, using built-in seeds.
#[derive(Clone)]
pub struct RandomState(ahash::RandomState);

impl fmt::Debug for RandomState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RandomState").finish_non_exhaustive()
    }
}

impl RandomState {
    pub fn new() -> Self {
        #[cfg(not(feature = "test"))]
        return RandomState(ahash::RandomState::with_seeds(
            seed64(),
            seed64(),
            seed64(),
            seed64(),
        ));
        #[cfg(feature = "test")]
        RandomState(ahash::RandomState::with_seed(rand::random()))
    }

    #[inline]
    pub fn hash_one<T: Hash>(&self, x: T) -> u64 {
        self.0.hash_one(x)
    }
}

impl Default for RandomState {
    fn default() -> Self {
        Self::new()
    }
}

impl BuildHasher for RandomState {
    type Hasher = ahash::AHasher;

    fn build_hasher(&self) -> Self::Hasher {
        self.0.build_hasher()
    }

    fn hash_one<T: ~const Hash + ~const Destruct>(&self, x: T) -> u64 {
        self.0.hash_one(x)
    }
}
