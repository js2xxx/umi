mod user;

use alloc::{boxed::Box, sync::Arc};
use core::{ops::Range, pin::Pin};

use arsc_rs::Arsc;
use co_trap::UserCx;
use kmem::{Phys, Virt};
use ksc::{async_handler, Error};
use rv39_paging::{Attr, CANONICAL_PREFIX, PAGE_MASK, PAGE_SHIFT, PAGE_SIZE};

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
                        Arc::new(Phys::new_anon(false)),
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
                    Arc::new(Phys::new_anon(false)),
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
