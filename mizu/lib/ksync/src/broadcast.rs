use core::{
    fmt,
    pin::Pin,
    sync::atomic::{AtomicUsize, Ordering::SeqCst},
    task::{Context, Poll},
};

use arsc_rs::Arsc;
use crossbeam_queue::SegQueue;
use futures_lite::{Future, FutureExt};
use futures_util::future::try_join_all;
use hashbrown::HashMap;
use rand_riscv::RandomState;
use spin::RwLock;

use crate::{unbounded, Receiver, Recv, RecvError, RecvOnce, SendError, Sender};

pub struct Broadcast<T: Clone> {
    inner: Arsc<Inner<T>>,
    receiver: Receiver<SegQueue<T>>,
    id: usize,
}

impl<T: Clone> fmt::Debug for Broadcast<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Broadcast").finish_non_exhaustive()
    }
}

struct Inner<T: Clone> {
    senders: RwLock<HashMap<usize, Sender<SegQueue<T>>, RandomState>>,
    id: AtomicUsize,
}

impl<T: Clone> Clone for Broadcast<T> {
    fn clone(&self) -> Self {
        let (tx, rx) = unbounded();
        let inner = self.inner.clone();
        let id = inner.id.fetch_add(1, SeqCst);
        ksync_core::critical(|| {
            // SAFETY: We know that IDs are self-incremental and unique.
            inner.senders.write().insert_unique_unchecked(id, tx);
        });
        Broadcast {
            inner,
            receiver: rx,
            id,
        }
    }
}

impl<T: Clone> Broadcast<T> {
    fn senders(&self) -> HashMap<usize, Sender<SegQueue<T>>, RandomState> {
        ksync_core::critical(|| self.inner.senders.read().clone())
    }

    pub fn new() -> Self {
        let (tx, rx) = unbounded();
        let inner = Arsc::new(Inner {
            senders: RwLock::new([(0, tx)].into_iter().collect()),
            id: AtomicUsize::new(1),
        });
        Broadcast {
            inner,
            receiver: rx,
            id: 0,
        }
    }

    pub async fn send(&self, data: &T) -> Result<(), SendError<T>> {
        let senders = self.senders();
        let iter = senders.iter();
        try_join_all(iter.map(|(_, sender)| async move { sender.send(data.clone()).await }))
            .await?;
        Ok(())
    }

    pub fn recv(&self) -> Recv<SegQueue<T>> {
        self.receiver.recv()
    }

    pub fn recv_once(self) -> BroadcastRecvOnce<T> {
        let fut = self.receiver.clone().recv_once();
        BroadcastRecvOnce { _b: self, fut }
    }
}

impl<T: Clone> Drop for Broadcast<T> {
    fn drop(&mut self) {
        ksync_core::critical(|| self.inner.senders.write().remove(&self.id));
    }
}

impl<T: Clone> Default for Broadcast<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[must_use = "futures do nothing unless polled"]
pub struct BroadcastRecvOnce<T: Clone> {
    _b: Broadcast<T>,
    fut: RecvOnce<SegQueue<T>>,
}

impl<T: Clone> Future for BroadcastRecvOnce<T> {
    type Output = Result<T, RecvError<T>>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.fut.poll(cx)
    }
}
