use core::{
    mem::ManuallyDrop,
    ptr,
    sync::atomic::{AtomicPtr, Ordering},
};

use arsc_rs::Arsc;

#[derive(Debug)]
pub struct AtomicArsc<T>(AtomicPtr<T>);

impl<T> AtomicArsc<T> {
    pub fn new(ptr: Arsc<T>) -> Self {
        AtomicArsc(AtomicPtr::new(Arsc::into_raw(ptr).cast_mut()))
    }

    pub fn load(&self, order: Ordering) -> Arsc<T> {
        unsafe {
            let ptr = self.0.load(order);
            let this = ManuallyDrop::new(Arsc::from_raw(ptr));
            (*this).clone()
        }
    }

    pub fn swap(&self, ptr: Arsc<T>, order: Ordering) -> Arsc<T> {
        unsafe {
            let ptr = self.0.swap(Arsc::into_raw(ptr).cast_mut(), order);
            Arsc::from_raw(ptr)
        }
    }

    pub fn compare_exchange(
        &self,
        current: &T,
        new: Arsc<T>,
        success: Ordering,
        failure: Ordering,
    ) -> Result<Arsc<T>, Arsc<T>> {
        match self.0.compare_exchange(
            ptr::addr_of!(*current).cast_mut(),
            Arsc::into_raw(new).cast_mut(),
            success,
            failure,
        ) {
            Ok(old) => Ok(unsafe { Arsc::from_raw(old) }),
            Err(err) => Err(unsafe { (*ManuallyDrop::new(Arsc::from_raw(err))).clone() }),
        }
    }
}

impl<T> Drop for AtomicArsc<T> {
    fn drop(&mut self) {
        let _ = unsafe { Arsc::from_raw(self.0.get_mut()) };
    }
}

impl<T: Default> Default for AtomicArsc<T> {
    fn default() -> Self {
        Self::new(Default::default())
    }
}