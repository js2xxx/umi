use alloc::{boxed::Box, sync::Arc};
use core::{mem, pin::Pin, time::Duration};

use co_trap::UserCx;
use kmem::{Phys, Virt};
use ksc::{
    async_handler,
    Error::{self, EAGAIN, EINVAL, EISDIR, ENOMEM, ENOSYS, EPERM, ETIMEDOUT},
};
use ktime::{TimeOutExt, Timer};
use rv39_paging::{Attr, LAddr, PAGE_MASK, PAGE_SHIFT, PAGE_SIZE};
use umifs::traits::IntoAnyExt;

use crate::{
    mem::{futex::RobustListHead, user::FutexKey, In, InOut, Out, UserPtr},
    syscall::{ScRet, Ts},
    task::TaskState,
};

#[async_handler]
pub async fn brk(ts: &mut TaskState, cx: UserCx<'_, fn(usize) -> Result<usize, Error>>) -> ScRet {
    async fn inner(virt: Pin<&Virt>, brk: &mut usize, addr: usize) -> Result<(), Error> {
        const BRK_START: usize = 0x12345000;
        const BRK_END: usize = 0x56789000;
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
            let new_page = (addr + PAGE_MASK) & !PAGE_MASK;
            if new_page >= BRK_END {
                return Err(ENOMEM);
            }
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

#[async_handler]
pub async fn futex(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(FutexKey, i32, u32, usize, FutexKey, u32) -> Result<usize, Error>>,
) -> ScRet {
    const FUTEX_WAIT: i32 = 0;
    const FUTEX_WAKE: i32 = 1;
    const FUTEX_REQUEUE: i32 = 3;
    const FUTEX_CMP_REQUEUE: i32 = 4;
    const FUTEX_PRIVATE_FLAG: i32 = 128;

    let (key, op, val, spec, key2, val3) = cx.args();
    let fut = async move {
        if op & FUTEX_PRIVATE_FLAG == 0 {
            return Err(ENOSYS);
        }
        Ok(match op & !FUTEX_PRIVATE_FLAG {
            FUTEX_WAIT => {
                let c = key.load(ts.virt.as_ref()).await?;
                if c != val {
                    return Err(EAGAIN);
                }
                let t = UserPtr::<Ts, In>::new(spec.into());
                if t.is_null() {
                    ts.futex.wait(key).await
                } else {
                    let t = t.read(ts.virt.as_ref()).await?;
                    let timeout = Duration::from_secs(t.sec) + Duration::from_nanos(t.nsec);
                    let wait = ts.futex.wait(key);
                    wait.ok_or_timeout(Timer::after(timeout), || ETIMEDOUT)
                        .await?;
                }
                0
            }
            FUTEX_WAKE => ts.futex.notify(key, val as usize),
            FUTEX_REQUEUE => ts.futex.requeue(key, key2, val as usize, spec),
            FUTEX_CMP_REQUEUE => {
                let c = key.load(ts.virt.as_ref()).await?;
                if c != val3 {
                    return Err(EAGAIN);
                }
                ts.futex.requeue(key, key2, val as usize, spec)
            }
            _ => return Err(ENOSYS),
        })
    };
    cx.ret(fut.await);
    ScRet::Continue(None)
}

#[async_handler]
pub async fn set_robust_list(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(UserPtr<RobustListHead, InOut>, usize) -> Result<(), Error>>,
) -> ScRet {
    let (ptr, len) = cx.args();
    cx.ret(if len == mem::size_of::<RobustListHead>() {
        ts.futex.set_robust_list(ptr);
        Ok(())
    } else {
        Err(EINVAL)
    });
    ScRet::Continue(None)
}

#[async_handler]
pub async fn get_robust_list(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(usize, UserPtr<LAddr, Out>, UserPtr<usize, Out>) -> Result<(), Error>>,
) -> ScRet {
    let (tid, mut ptr, mut len) = cx.args();
    let fut = async move {
        if tid != 0 {
            return Err(EPERM);
        }
        let rl = ts.futex.robust_list();
        let virt = ts.virt.as_ref();

        len.write(virt, mem::size_of::<RobustListHead>()).await?;
        ptr.write(virt, rl.addr()).await
    };
    cx.ret(fut.await);
    ScRet::Continue(None)
}

bitflags::bitflags! {
    #[derive(Default, Debug, Clone, Copy)]
    struct Prot: i32 {
        const READ     = 0x1;
        const WRITE    = 0x2;
        const EXEC     = 0x4;
    }

    struct Flags: i32 {
        const SHARED	= 0x01;		/* Share changes */
        const PRIVATE	= 0x02;		/* Changes are private */

        const FIXED     = 0x10;  /* Interpret addr exactly */
        const ANONYMOUS = 0x20;  /* don't use a file */

        const POPULATE  = 0x8000;  /* populate (prefault) pagetables */
    }
}

#[async_handler]
pub async fn mmap(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(usize, usize, i32, i32, i32, usize) -> Result<usize, Error>>,
) -> ScRet {
    let (addr, len, prot, flags, fd, offset) = cx.args();
    let fut = async move {
        let prot = Prot::from_bits(prot).ok_or(ENOSYS)?;
        let flags = Flags::from_bits_truncate(flags);

        let cow = flags.contains(Flags::PRIVATE);
        let phys = if flags.contains(Flags::ANONYMOUS) {
            Phys::new_anon(cow)
        } else {
            let entry = ts.files.get(fd).await?;
            match entry.clone().downcast::<Phys>() {
                Some(phys) => phys.clone_as(cow, 0, None),
                None => crate::mem::new_phys(entry.to_io().ok_or(EISDIR)?, cow),
            }
        };

        let addr = flags.contains(Flags::FIXED).then(|| LAddr::from(addr));

        let offset = if offset & PAGE_MASK != 0 {
            return Err(EINVAL);
        } else {
            offset >> PAGE_SHIFT
        };

        let attr = Attr::builder()
            .user_access(true)
            .readable(prot.contains(Prot::READ))
            .writable(prot.contains(Prot::WRITE))
            .executable(prot.contains(Prot::EXEC))
            .build();

        let count = (len + PAGE_MASK) >> PAGE_SHIFT;
        let addr = ts
            .virt
            .map(addr, Arc::new(phys), offset, count, attr)
            .await?;

        if flags.contains(Flags::POPULATE) {
            ts.virt.commit_range(addr..(addr + len)).await?;
        }

        Ok(addr.val())
    };
    cx.ret(fut.await);
    ScRet::Continue(None)
}

#[async_handler]
pub async fn mprotect(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(usize, usize, i32) -> Result<(), Error>>,
) -> ScRet {
    let (addr, len, prot) = cx.args();
    let fut = async move {
        let prot = Prot::from_bits(prot).ok_or(ENOSYS)?;

        let attr = Attr::builder()
            .user_access(true)
            .readable(prot.contains(Prot::READ))
            .writable(prot.contains(Prot::WRITE))
            .executable(prot.contains(Prot::EXEC))
            .build();

        let len = (len + PAGE_MASK) & !PAGE_MASK;
        ts.virt
            .reprotect(addr.into()..(addr + len).into(), attr)
            .await
    };
    cx.ret(fut.await);
    ScRet::Continue(None)
}

#[async_handler]
pub async fn munmap(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(usize, usize) -> Result<(), Error>>,
) -> ScRet {
    let (addr, len) = cx.args();
    let len = (len + PAGE_MASK) & !PAGE_MASK;
    cx.ret(ts.virt.unmap(addr.into()..(addr + len).into()).await);
    ScRet::Continue(None)
}
