use core::{num::NonZeroU32, ptr::NonNull};

use devices::intr::{Completion, IntrHandler};
use ksc::Handlers;
use spin::{RwLock, RwLockUpgradableGuard};

use crate::dev::Plic;

pub struct IntrManager {
    plic: Plic,
    map: RwLock<Handlers<u32, &'static Completion, bool>>,
}

impl IntrManager {
    pub fn new(plic: Plic) -> Self {
        hart_id::for_each_hart(|hid| plic.set_priority_threshold(Self::hid_to_cx(hid), 0));
        IntrManager {
            plic,
            map: RwLock::new(Default::default()),
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

    pub fn insert(&self, pin: NonZeroU32, handler: impl IntrHandler) -> bool {
        let pin = pin.get();
        if !ksync::critical(|| self.map.write().try_insert(pin, handler)) {
            return false;
        }
        hart_id::for_each_hart(|hid| self.plic.enable(pin, Self::hid_to_cx(hid), true));
        self.plic.set_priority(pin, 1);
        true
    }

    pub fn check_pending(&self, pin: NonZeroU32) -> bool {
        self.plic.pending(pin.get())
    }

    pub fn notify(&'static self, hid: usize) {
        let cx = Self::hid_to_cx(hid);
        let pin = self.plic.claim(cx);
        if pin == 0 {
            return;
        }
        // log::trace!("Intr::notify cx = {cx}, pin = {pin}");
        let exist = ksync::critical(move || {
            let map = self.map.upgradeable_read();
            let ret = map.handle(pin, &move || self.plic.complete(cx, pin));
            match ret {
                Some(false) => {
                    let mut map = RwLockUpgradableGuard::upgrade(map);
                    map.remove(pin);
                    false
                }
                Some(true) => true,
                None => false,
            }
        });
        if !exist {
            self.plic.enable(pin, cx, false);
            self.plic.set_priority(pin, 0);
        }
    }
}
