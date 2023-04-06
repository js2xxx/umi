use crossbeam_queue::SegQueue;
use hashbrown::{hash_map::Entry, HashMap};
use ksync::{unbounded, Receiver, Sender};
use rand_riscv::RandomState;

use crate::Plic;

pub struct IntrManager {
    cx: usize,
    plic: Plic,
    map: HashMap<u32, Sender<SegQueue<()>>, RandomState>,
}

impl IntrManager {
    pub fn new(cx: usize, plic: Plic) -> Self {
        IntrManager {
            cx,
            plic,
            map: HashMap::with_hasher(RandomState::new()),
        }
    }

    pub fn insert(&mut self, pin: u32) -> Option<Interrupt> {
        let rx = match self.map.entry(pin) {
            Entry::Occupied(entry) if entry.get().is_closed() => {
                let (tx, rx) = unbounded();
                entry.replace_entry(tx);
                rx
            }
            Entry::Vacant(entry) => {
                let (tx, rx) = unbounded();
                entry.insert(tx);
                rx
            }
            _ => return None,
        };
        self.plic.enable(pin, self.cx, true);
        Some(Interrupt(rx))
    }

    pub fn notify(&mut self) {
        let pin = self.plic.claim(self.cx);
        if let Entry::Occupied(sender) = self.map.entry(pin) {
            if sender.get().try_send(()).is_err() {
                sender.remove();
                self.plic.enable(pin, self.cx, false);
            }
        }
        self.plic.complete(self.cx, pin);
    }
}

#[derive(Clone)]
pub struct Interrupt(Receiver<SegQueue<()>>);

impl Interrupt {
    pub async fn wait(&self) -> bool {
        self.0.recv().await.is_ok()
    }
}
