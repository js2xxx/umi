use crossbeam_queue::SegQueue;
use hashbrown::{hash_map::Entry, HashMap};
use ksync::{unbounded, Receiver, Sender};
use rand_riscv::RandomState;
use spin::{Lazy, RwLock};

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

    pub fn insert(&self, cx: usize, pin: u32) -> Option<Interrupt> {
        if pin == 0 {
            return None;
        }
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
        self.plic.enable(pin, cx, true);
        Some(Interrupt(rx))
    }

    pub fn notify(&self, cx: usize) {
        let pin = self.plic.claim(cx);
        if pin > 0 {
            let exist = ksync::critical(|| {
                let map = self.map.read();
                map.get(&pin).and_then(|sender| sender.try_send(()).ok())
            });
            if exist.is_none() {
                self.plic.enable(pin, cx, false);
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
}

pub static INTR: Lazy<IntrManager> = Lazy::new(|| {
    let plic = crate::dev::PLIC.get().cloned();
    IntrManager::new(plic.expect("PLIC not initialized"))
});
