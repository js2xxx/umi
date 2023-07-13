use core::{num::NonZeroU32, ptr::NonNull};

use crossbeam_queue::SegQueue;
use devices::intr::Interrupt;
use hashbrown::{hash_map::Entry, HashMap};
use ksync::channel::{mpmc::Sender, unbounded};
use rand_riscv::RandomState;
use spin::RwLock;

use crate::dev::Plic;

pub struct IntrManager {
    plic: Plic,
    map: RwLock<HashMap<u32, Sender<SegQueue<()>>, RandomState>>,
}

impl IntrManager {
    pub fn new(plic: Plic) -> Self {
        hart_id::for_each_hart(|hid| plic.set_priority_threshold(Self::hid_to_cx(hid), 0));
        IntrManager {
            plic,
            map: RwLock::new(HashMap::with_hasher(RandomState::new())),
        }
    }

    /// # Safety
    ///
    /// See [`crate::dev::Plic`] for more information.
    pub unsafe fn from_raw(base: NonNull<()>) -> Self {
        Self::new(Plic::new(base))
    }

    fn hid_to_cx(hid: usize) -> usize {
        hid * 2 + 1
    }

    pub fn insert(&self, pin: NonZeroU32) -> Option<Interrupt> {
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
        hart_id::for_each_hart(|hid| self.plic.enable(pin, Self::hid_to_cx(hid), true));
        self.plic.set_priority(pin, 1);
        Some(Interrupt(rx))
    }

    pub fn check_pending(&self, pin: NonZeroU32) -> bool {
        self.plic.pending(pin.get())
    }

    pub fn notify(&self, hid: usize) {
        let cx = Self::hid_to_cx(hid);
        let pin = self.plic.claim(cx);
        // log::trace!("Intr::notify cx = {cx}, pin = {pin}");
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
