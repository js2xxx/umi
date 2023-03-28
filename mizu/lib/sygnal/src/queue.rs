use core::{
    array,
    sync::atomic::{AtomicU64, Ordering::SeqCst},
};

use crossbeam_queue::ArrayQueue;
use rv39_paging::LAddr;

use crate::{Sig, SigSet, NR_SIGNALS};

const CAP_PER_SIG: usize = 8;

pub struct Signals {
    set: AtomicU64,
    pending: [ArrayQueue<SigInfo>; NR_SIGNALS],
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SigInfo {
    pub sig: Sig,
    pub code: i32,
    pub fields: SigFields,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum SigFields {
    None,
    SigKill { pid: usize, uid: usize },
    SigChld { pid: usize, uid: usize, status: i32 },
    SigSys { addr: LAddr, num: u32 },
}

impl Signals {
    pub fn new() -> Self {
        Signals {
            set: AtomicU64::new(0),
            pending: array::from_fn(|_| ArrayQueue::new(CAP_PER_SIG)),
        }
    }

    pub fn push(&self, info: SigInfo) {
        let old: SigSet = self.set.fetch_or(info.sig.mask(), SeqCst).into();

        if !(info.sig.is_legacy() && old.contains(info.sig)) {
            let _ = self.pending[info.sig.index()].push(info);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.set.load(SeqCst) == 0
    }

    pub fn pop(&self, masked: SigSet) -> Option<SigInfo> {
        if self.is_empty() {
            return None;
        }
        let iter = self.pending.iter().enumerate();

        let (info, is_empty) = iter
            .filter(|&(index, _)| !masked.contains_index(index))
            .find_map(|(_, queue)| queue.pop().map(|s| (s, queue.is_empty())))?;

        if is_empty {
            self.set.fetch_and(!info.sig.mask(), SeqCst);
        }
        Some(info)
    }
}

impl Default for Signals {
    fn default() -> Self {
        Self::new()
    }
}
