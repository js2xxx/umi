mod user;

use alloc::{boxed::Box, sync::Arc};
use core::{ops::Range, pin::Pin};

use arsc_rs::Arsc;
use co_trap::UserCx;
use kmem::{CreateSub, Phys, Virt};
use ksc::{async_handler, Error};
use rv39_paging::{Attr, CANONICAL_PREFIX, PAGE_MASK, PAGE_SHIFT, PAGE_SIZE};
use umifs::traits::IoExt;

pub use self::user::{In, InOut, Out, UserBuffer, UserPtr, UA_FAULT};
use crate::{rxx::KERNEL_PAGES, syscall::ScRet, task::TaskState};

pub const USER_RANGE: Range<usize> = 0x1000..((!CANONICAL_PREFIX) + 1);

pub fn new_virt() -> Pin<Arsc<Virt>> {
    Virt::new(USER_RANGE.start.into()..USER_RANGE.end.into(), KERNEL_PAGES)
}

pub async fn deep_fork(virt: &Pin<Arsc<Virt>>) -> Result<Pin<Arsc<Virt>>, Error> {
    virt.as_ref().deep_fork(KERNEL_PAGES).await
}

#[async_handler]
pub async fn brk(ts: &mut TaskState, cx: UserCx<'_, fn(usize) -> Result<usize, Error>>) -> ScRet {
    async fn inner(virt: Pin<&Virt>, brk: &mut usize, addr: usize) -> Result<(), Error> {
        const BRK_START: usize = 0x12345000;
        if addr == 0 {
            if (*brk) == 0 {
                let laddr = virt
                    .map(
                        Some(BRK_START.into()),
                        Arc::new(Phys::new_anon(true)),
                        0,
                        1,
                        Attr::USER_RW,
                    )
                    .await?;
                *brk = laddr.val();
            }
        } else {
            let old_page = *brk & !PAGE_MASK;
            let new_page = addr & !PAGE_MASK;
            let count = (new_page - old_page) >> PAGE_SHIFT;
            if count > 0 {
                virt.map(
                    Some((old_page + PAGE_SIZE).into()),
                    Arc::new(Phys::new_anon(true)),
                    0,
                    count,
                    Attr::USER_RW,
                )
                .await?;
            }
            *brk = addr;
        }
        Ok(())
    }

    let addr = cx.args();
    let res = inner(ts.virt.as_ref(), &mut ts.brk, addr).await;
    cx.ret(res.map(|_| ts.brk));

    ScRet::Continue(None)
}

#[allow(dead_code)]
pub async fn test_phys() {
    let p = Arc::new(Phys::new_anon(false));
    p.write_all_at(0, &[1, 2, 3, 4, 5]).await.unwrap();
    p.write_all_at(PAGE_SIZE, &[6, 7, 8, 9, 10]).await.unwrap();
    // log::debug!("#1: p = {p:#?}");

    let mut buf = [0; 5];
    let p1 = p
        .clone_as(
            false,
            Some(CreateSub {
                index_offset: 0,
                fixed_count: Some(1),
            }),
        )
        .await;
    // log::debug!("#2: p = {p:#?}");
    // log::debug!("#2: p1 = {p1:#?}");
    p1.read_exact_at(0, &mut buf).await.unwrap();
    assert_eq!(buf, [1, 2, 3, 4, 5]);

    let p2 = p
        .clone_as(
            true,
            Some(CreateSub {
                index_offset: 1,
                fixed_count: Some(1),
            }),
        )
        .await;

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
