use core::{hint, mem, sync::atomic::Ordering};

use arsc_rs::Arsc;
use atomic_refcell::AtomicRefCell;

#[derive(Debug)]
pub struct AtomicArsc<T>(AtomicRefCell<Arsc<T>>);

impl<T> AtomicArsc<T> {
    pub fn new(ptr: Arsc<T>) -> Self {
        AtomicArsc(AtomicRefCell::new(ptr))
    }

    pub fn load(&self, _: Ordering) -> Arsc<T> {
        ksync_core::critical(|| loop {
            if let Ok(b) = self.0.try_borrow() {
                break b.clone();
            }
            hint::spin_loop()
        })
    }

    pub fn swap(&self, ptr: Arsc<T>, _: Ordering) -> Arsc<T> {
        ksync_core::critical(|| loop {
            if let Ok(mut b) = self.0.try_borrow_mut() {
                break mem::replace(&mut *b, ptr);
            }
            hint::spin_loop()
        })
    }
}

impl<T: Default> Default for AtomicArsc<T> {
    fn default() -> Self {
        Self::new(Default::default())
    }
}
