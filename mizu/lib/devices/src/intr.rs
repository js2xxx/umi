use core::num::NonZeroU32;

use crossbeam_queue::SegQueue;
use hashbrown::{hash_map::Entry, HashMap};
use ksync::{unbounded, Receiver, Sender, TryRecvError};
use rand_riscv::RandomState;
use spin::RwLock;

use crate::dev::Plic;

pub struct IntrManager {
    plic: Plic,
    map: RwLock<HashMap<u32, Sender<SegQueue<()>>, RandomState>>,
}

impl IntrManager {
    pub fn new(plic: Plic) -> Self {
        IntrManager {
            plic,
            map: RwLock::new(HashMap::with_hasher(RandomState::new())),
        }
    }

    pub fn insert(
        &self,
        cx: impl IntoIterator<Item = usize>,
        pin: NonZeroU32,
    ) -> Option<Interrupt> {
        let pin = pin.get();
        let rx = ksync::critical(|| match self.map.write().entry(pin) {
            Entry::Occupied(entry) if entry.get().is_closed() => {
                let (tx, rx) = unbounded();
                entry.replace_entry(tx);
                Some(rx)
            }
            Entry::Vacant(entry) => {
                let (tx, rx) = unbounded();
                entry.insert(tx);
                Some(rx)
            }
            _ => None,
        })?;
        cx.into_iter().for_each(|cx| {
            self.plic.enable(pin, cx, true);
            self.plic.set_priority(pin, 10)
        });
        Some(Interrupt(rx))
    }

    pub fn check_pending(&self, pin: NonZeroU32) -> bool {
        self.plic.pending(pin.get())
    }

    pub fn notify(&self, cx: usize) {
        // log::trace!("Intr::notify cx = {cx}");
        let pin = self.plic.claim(cx);
        if pin > 0 {
            let exist = ksync::critical(|| {
                let map = self.map.read();
                map.get(&pin).and_then(|sender| sender.try_send(()).ok())
            });
            if exist.is_none() {
                self.plic.enable(pin, cx, false);
                self.plic.set_priority(pin, 0);
            }
            self.plic.complete(cx, pin);
        }
    }
}

#[derive(Clone)]
pub struct Interrupt(Receiver<SegQueue<()>>);

impl Interrupt {
    pub async fn wait(&self) -> bool {
        self.0.recv().await.is_ok()
    }

    pub fn try_wait(&self) -> Option<bool> {
        match self.0.try_recv() {
            Ok(_) | Err(TryRecvError::Closed(Some(_))) => Some(true),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Closed(None)) => Some(false),
        }
    }
}
