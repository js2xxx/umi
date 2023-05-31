use alloc::vec::Vec;

use arsc_rs::Arsc;
use spin::{Mutex, Once};

use super::mpmc::{Flavor, Sender};

#[derive(Debug)]
pub struct Broadcast<F: Flavor> {
    senders: Arsc<Mutex<Vec<Sender<F>>>>,
    closed: Once,
}

impl<F: Flavor> Broadcast<F> {
    pub fn new() -> Self {
        Broadcast {
            senders: Arsc::new(Mutex::new(Vec::new())),
            closed: Once::new(),
        }
    }

    pub fn subscribe(&self, sender: Sender<F>) {
        ksync_core::critical(|| self.senders.lock().push(sender))
    }

    pub fn close(&self) {
        self.closed.call_once(|| ());
    }

    pub fn is_closed(&self) -> bool {
        self.closed.is_completed()
    }

    pub async fn send(&self, data: &F::Item)
    where
        F::Item: Clone,
    {
        if self.closed.is_completed() {
            return;
        }
        let mut senders = ksync_core::critical(|| self.senders.lock().clone());
        if senders.is_empty() {
            return;
        }
        let mut pos = senders.len() - 1;
        loop {
            let sender = &senders[pos];
            if sender.send(data.clone()).await.is_err() {
                senders.swap_remove(pos);
            }
            if pos == 0 {
                ksync_core::critical(|| *self.senders.lock() = senders);
                break;
            }
            pos -= 1;
        }
    }
}

impl<F: Flavor> Default for Broadcast<F> {
    fn default() -> Self {
        Self::new()
    }
}
