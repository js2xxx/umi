use core::{
    array,
    sync::atomic::{AtomicU64, Ordering::SeqCst},
};

use crossbeam_queue::ArrayQueue;
use futures_util::{future, FutureExt};
use ksync::event::{Event, EventListener};
use rv39_paging::LAddr;

use crate::{Sig, SigSet, NR_SIGNALS};

const CAP_PER_SIG: usize = 8;

struct SigPending {
    queue: ArrayQueue<SigInfo>,
    event: Event,
}

pub struct Signals {
    set: AtomicU64,
    pending: [SigPending; NR_SIGNALS],
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
            pending: array::from_fn(|index| SigPending {
                queue: ArrayQueue::new(match Sig::from_index(index) {
                    // Each legacy signal only needs 1 entry.
                    Some(sig) if sig.is_legacy() => 1,
                    _ => CAP_PER_SIG,
                }),
                event: Event::new(),
            }),
        }
    }

    pub fn push(&self, info: SigInfo) {
        let old: SigSet = self.set.fetch_or(info.sig.mask(), SeqCst).into();

        if !(info.sig.is_legacy() && old.contains(info.sig)) {
            let sig_pending = &self.pending[info.sig.index()];
            let res = sig_pending.queue.push(info);
            if res.is_ok() {
                sig_pending.event.notify_additional(1);
            }
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
            .find_map(|(_, pending)| pending.queue.pop().map(|s| (s, pending.queue.is_empty())))?;

        if is_empty {
            self.set.fetch_and(!info.sig.mask(), SeqCst);
        }
        Some(info)
    }

    pub fn wait_one(&self, sig: Sig) -> EventListener {
        self.pending[sig.index()].event.listen()
    }

    pub async fn wait(&self, sigset: SigSet) -> Sig {
        let wait_one = |sig| self.wait_one(sig).map(move |_| sig);
        let wait_any = future::select_all(sigset.map(wait_one));
        wait_any.await.0
    }
}

impl Default for Signals {
    fn default() -> Self {
        Self::new()
    }
}
