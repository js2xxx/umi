use alloc::{boxed::Box, sync::Arc};
use core::mem;

use co_trap::UserCx;
use kmem::Phys;
use ksc::{
    async_handler,
    Error::{self, EAGAIN, EINVAL, EISDIR, ENOSYS, EPERM, ETIMEDOUT},
};
use ktime::{TimeOutExt, Timer};
use rv39_paging::{Attr, LAddr, PAGE_MASK, PAGE_SHIFT};

use crate::{
    mem::{futex::RobustListHead, user::FutexKey, In, InOut, Out, UserPtr},
    syscall::{ffi::Ts, ScRet},
    task::TaskState,
};

#[async_handler]
pub async fn brk(ts: &mut TaskState, cx: UserCx<'_, fn(usize) -> Result<usize, Error>>) -> ScRet {
    const BRK_START: usize = 0x12345000;
    const BRK_END: usize = 0x56789000;

    let addr = cx.args();
    let fut = async {
        if ts.brk == 0 {
            ts.brk = BRK_START;
        }
        if !(BRK_START..BRK_END).contains(&addr) {
            return Ok(ts.brk);
        }
        if addr > ts.brk {
            let old_page = (ts.brk + PAGE_MASK) & !PAGE_MASK;
            let new_page = (addr + PAGE_MASK) & !PAGE_MASK;
            let count = (new_page - old_page) >> PAGE_SHIFT;
            if count > 0 {
                let phys = Arc::new(Phys::new_anon(true));
                ts.virt
                    .map(Some(old_page.into()), phys, 0, count, Attr::USER_RW)
                    .await?;
            }
        }
        ts.brk = addr;
        Ok(addr)
    };
    let ret = fut.await;
    log::trace!("user brk addr = {addr:x}, ret = {ret:x?}");
    cx.ret(ret);
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
                    let timeout = t.read(ts.virt.as_ref()).await?.into();
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

    #[derive(Default, Debug, Clone, Copy)]
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
            crate::mem::new_phys(entry.to_io().ok_or(EISDIR)?, cow)
        };

        let addr = (flags.contains(Flags::FIXED) || addr != 0).then(|| LAddr::from(addr));

        log::trace!("user mmap at {addr:?}, len = {len}, prot = {prot:?}, flags = {flags:?}");
        log::trace!("user mmap: fd = {fd}, offset = {offset}");

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

        if flags.contains(Flags::FIXED) {
            let addr = addr.unwrap().val() & !PAGE_MASK;
            let len = (len + PAGE_MASK) & !PAGE_MASK;
            ts.virt.unmap(addr.into()..(addr + len).into()).await?;
        }

        let count = (len + PAGE_MASK) >> PAGE_SHIFT;
        let addr = ts
            .virt
            .map(addr, Arc::new(phys), offset, count, attr)
            .await?;

        if flags.contains(Flags::POPULATE) {
            ts.virt
                .commit_range(addr..(addr + len), Default::default())
                .await?;
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

#[async_handler]
pub async fn membarrier(
    _: &mut TaskState,
    cx: UserCx<'_, fn(i32, u32, usize) -> Result<i32, Error>>,
) -> ScRet {
    let (cmd, flags, hid) = cx.args();
    cx.ret(if cmd == 0 {
        Ok(i32::MAX)
    } else {
        if flags == 1 {
            crate::cpu::IPI.remote_fence(1 << hid);
        } else {
            crate::cpu::IPI.remote_fence(hart_id::hart_ids())
        }
        Ok(0)
    });
    ScRet::Continue(None)
}
