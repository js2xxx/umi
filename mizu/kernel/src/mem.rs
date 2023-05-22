mod futex;
mod syscall;
mod user;

use alloc::sync::Arc;
use core::{ops::Range, pin::Pin};

use arsc_rs::Arsc;
use kmem::{CreateSub, Phys, Virt};
use ksc::Error;
use rv39_paging::{CANONICAL_PREFIX, PAGE_SIZE};
use umifs::traits::{IntoAnyExt, Io, IoExt};

pub use self::{
    futex::{FutexWait, Futexes},
    syscall::*,
    user::{In, InOut, Out, UserBuffer, UserPtr, UA_FAULT},
};
use crate::rxx::KERNEL_PAGES;

pub const USER_RANGE: Range<usize> = 0x1000..((!CANONICAL_PREFIX) + 1);

pub fn new_virt() -> Pin<Arsc<Virt>> {
    Virt::new(USER_RANGE.start.into()..USER_RANGE.end.into(), KERNEL_PAGES)
}

pub fn new_phys(from: Arc<dyn Io>, cow: bool) -> Phys {
    if let Some(phys) = from.clone().downcast::<Phys>() {
        return phys.clone_as(cow, None);
    }
    let (phys, flusher) = Phys::new(from, 0, cow);
    crate::executor().spawn(flusher).detach();
    phys
}

pub async fn deep_fork(virt: &Pin<Arsc<Virt>>) -> Result<Pin<Arsc<Virt>>, Error> {
    virt.as_ref().deep_fork(KERNEL_PAGES).await
}

#[allow(dead_code)]
pub async fn test_phys() {
    let p = Arc::new(Phys::new_anon(false));
    p.write_all_at(0, &[1, 2, 3, 4, 5]).await.unwrap();
    p.write_all_at(PAGE_SIZE, &[6, 7, 8, 9, 10]).await.unwrap();
    // log::debug!("#1: p = {p:#?}");

    let mut buf = [0; 5];
    let p1 = p.clone_as(
        true,
        Some(CreateSub {
            index_offset: 0,
            fixed_count: Some(1),
        }),
    );
    // log::debug!("#2: p = {p:#?}");
    // log::debug!("#2: p1 = {p1:#?}");
    p1.read_exact_at(0, &mut buf).await.unwrap();
    assert_eq!(buf, [1, 2, 3, 4, 5]);

    let p2 = p.clone_as(
        false,
        Some(CreateSub {
            index_offset: 1,
            fixed_count: Some(1),
        }),
    );

    // log::debug!("#3: p = {p:#?}");
    // log::debug!("#3: p1 = {p1:#?}");
    // log::debug!("#3: p2 = {p2:#?}");
    p2.read_exact_at(0, &mut buf).await.unwrap();
    assert_eq!(buf, [6, 7, 8, 9, 10]);

    p1.write_all_at(0, &[5, 4, 3, 2, 1]).await.unwrap();
    // log::debug!("#4: p = {p:#?}");
    // log::debug!("#4: p1 = {p1:#?}");
    // log::debug!("#4: p2 = {p2:#?}");
    p.read_exact_at(0, &mut buf).await.unwrap();
    assert_eq!(buf, [1, 2, 3, 4, 5]);

    p2.write_all_at(0, &[10, 9, 8, 7, 6]).await.unwrap();
    // log::debug!("#5: p = {p:#?}");
    // log::debug!("#5: p1 = {p1:#?}");
    // log::debug!("#5: p2 = {p2:#?}");
    p.read_exact_at(PAGE_SIZE, &mut buf).await.unwrap();
    assert_eq!(buf, [10, 9, 8, 7, 6]);

    p.write_all_at(0, &[0, 0, 0, 0, 0]).await.unwrap();
    // log::debug!("#6: p = {p:#?}");
    // log::debug!("#6: p1 = {p1:#?}");
    // log::debug!("#6: p2 = {p2:#?}");
    p1.read_exact_at(0, &mut buf).await.unwrap();
    assert_eq!(buf, [5, 4, 3, 2, 1]);

    p.write_all_at(PAGE_SIZE, &[0, 0, 0, 0, 0]).await.unwrap();
    // log::debug!("#7: p = {p:#?}");
    // log::debug!("#7: p1 = {p1:#?}");
    // log::debug!("#7: p2 = {p2:#?}");
    p2.read_exact_at(0, &mut buf).await.unwrap();
    assert_eq!(buf, [0, 0, 0, 0, 0]);

    p.flush_all().await.unwrap();
    p1.flush_all().await.unwrap();
    p2.flush_all().await.unwrap();

    // log::debug!("#-1: p = {p:#?}");
    // log::debug!("#-1: p1 = {p1:#?}");
    // log::debug!("#-1: p2 = {p2:#?}");
}
