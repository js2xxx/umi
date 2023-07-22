mod futex;
mod shm;
mod syscall;
mod user;

use alloc::sync::Arc;
use core::ops::Range;

use arsc_rs::Arsc;
use kmem::{Phys, Virt};
use ksc::Error;
use rv39_paging::{CANONICAL_PREFIX, PAGE_SIZE};
use umifs::traits::{IntoAnyExt, Io, IoExt};

pub use self::{
    futex::{FutexWait, Futexes},
    shm::Shm,
    syscall::*,
    user::{In, InOut, Out, UserBuffer, UserPtr, UA_FAULT},
};
use crate::rxx::KERNEL_PAGES;

pub const USER_RANGE: Range<usize> = 0x1000..((!CANONICAL_PREFIX) + 1);

pub fn new_virt() -> Arsc<Virt> {
    Virt::new(USER_RANGE.start.into()..USER_RANGE.end.into(), KERNEL_PAGES)
}

pub fn new_phys(from: Arc<dyn Io>, cow: bool) -> Phys {
    if let Some(phys) = from.clone().downcast::<Phys>() {
        return phys.clone_as(cow, 0, None);
    }
    let (phys, flusher) = Phys::with_backend(from, 0, cow);
    crate::executor().spawn(flusher).detach();
    phys
}

pub async fn deep_fork(virt: &Arsc<Virt>) -> Result<Arsc<Virt>, Error> {
    virt.deep_fork(KERNEL_PAGES).await
}

#[allow(dead_code)]
pub async fn test_phys() {
    let p = Phys::new(false);

    p.write_all_at(0, &[1, 2, 3, 4, 5]).await.unwrap();

    let p1 = p.clone_as(true, 0, None);

    let mut buf = [0; 5];
    p1.read_exact_at(0, &mut buf).await.unwrap();
    assert_eq!(buf, [1, 2, 3, 4, 5]);

    let p2 = p.clone_as(false, 0, None);

    p.write_all_at(PAGE_SIZE, &[6, 7, 8, 9, 10]).await.unwrap();

    p2.read_exact_at(0, &mut buf).await.unwrap();
    assert_eq!(buf, [1, 2, 3, 4, 5]);
    p2.read_exact_at(PAGE_SIZE, &mut buf).await.unwrap();
    assert_eq!(buf, [6, 7, 8, 9, 10]);
    p1.read_exact_at(PAGE_SIZE, &mut buf).await.unwrap();
    assert_eq!(buf, [0; 5]);
}
