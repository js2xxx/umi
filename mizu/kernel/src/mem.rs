use alloc::{boxed::Box, sync::Arc, vec::Vec};
use core::{
    ffi::CStr,
    fmt,
    marker::PhantomData,
    mem::{self, MaybeUninit},
    ops::Range,
    pin::Pin,
};

use arsc_rs::Arsc;
use co_trap::UserCx;
use futures_util::Future;
use kmem::{Phys, Virt};
use ksc::{
    async_handler,
    Error::{self, EFAULT, EINVAL, ERANGE},
    RawReg,
};
use rv39_paging::{
    Attr, LAddr, PAddr, CANONICAL_PREFIX, ID_OFFSET, PAGE_MASK, PAGE_SHIFT, PAGE_SIZE,
};
use scoped_tls::scoped_thread_local;
use umifs::path::Path;

use crate::{rxx::KERNEL_PAGES, syscall::ScRet, task::TaskState};

const USER_RANGE: Range<usize> = 0x1000..((!CANONICAL_PREFIX) + 1);

pub fn new_virt() -> Pin<Arsc<Virt>> {
    Virt::new(USER_RANGE.start.into()..USER_RANGE.end.into(), KERNEL_PAGES)
}

#[async_handler]
pub async fn brk(ts: &mut TaskState, cx: UserCx<'_, fn(usize) -> Result<usize, Error>>) -> ScRet {
    async fn inner(virt: Pin<&Virt>, brk: &mut usize, addr: usize) -> Result<(), Error> {
        const BRK_START: usize = 0x1234567000;
        if addr == 0 {
            if (*brk) == 0 {
                let laddr = virt
                    .map(
                        Some(BRK_START.into()),
                        Arc::new(Phys::new_anon()),
                        0,
                        1,
                        Attr::USER_RW,
                    )
                    .await?;
                *brk = laddr.val();
            }
        } else {
            let old_page = *brk & PAGE_MASK;
            let new_page = addr & PAGE_MASK;
            let count = (new_page - old_page) >> PAGE_SHIFT;
            if count > 0 {
                virt.map(
                    Some((old_page + PAGE_SIZE).into()),
                    Arc::new(Phys::new_anon()),
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
    let res = inner(ts.task.virt(), &mut ts.brk, addr).await;
    cx.ret(res.map(|_| ts.brk));

    ScRet::Continue(None)
}

pub trait PtrType {
    const DEBUG: &'static str;
}
pub trait InPtr: PtrType {}
pub trait OutPtr: PtrType {}
pub enum In {}
pub enum InOut {}
pub enum Out {}

impl PtrType for In {
    const DEBUG: &'static str = "In";
}
impl PtrType for InOut {
    const DEBUG: &'static str = "InOut";
}
impl PtrType for Out {
    const DEBUG: &'static str = "Out";
}
impl InPtr for In {}
impl InPtr for InOut {}
impl OutPtr for Out {}
impl OutPtr for InOut {}

pub struct UserPtr<T: Copy, D> {
    addr: usize,
    _marker: PhantomData<(T, D)>,
}

impl<T: Copy, D: PtrType> fmt::Debug for UserPtr<T, D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({}) {:#x}", D::DEBUG, self.addr)
    }
}

impl<T: Copy, D> UserPtr<T, D> {
    pub fn addr(&self) -> usize {
        self.addr
    }

    pub fn advance(&mut self, offset: usize) {
        self.addr += offset
    }

    pub fn cast<U: Copy>(self) -> UserPtr<U, D> {
        UserPtr {
            addr: self.addr,
            _marker: PhantomData,
        }
    }

    async fn op<'a, U, G, F>(
        &self,
        virt: Pin<&'a Virt>,
        mut f: G,
        mut len: usize,
        mut arg: U,
    ) -> Result<(), Error>
    where
        G: FnMut(Pin<&'a Virt>, Range<LAddr>, U) -> F,
        F: Future<Output = Result<Option<U>, Error>> + Send + 'a,
        U: 'a,
    {
        let mut start = self.addr;
        let mut end = (start + PAGE_MASK) & !PAGE_MASK;

        log::trace!("UserPtr::op at {start:#x}, len = {len}");

        if end >= start.checked_add(len).ok_or(EINVAL)? {
            log::trace!("UserPtr::op direct call");
            let ret = f(virt, start.into()..(start + len).into(), arg).await?;
            if ret.is_none() {
                Ok(())
            } else {
                Err(ERANGE)
            }
        } else {
            loop {
                log::trace!("UserPtr::op part at {start:#x}..{end:#x}");
                arg = match f(virt, start.into()..end.into(), arg).await? {
                    Some(arg) => arg,
                    None => break Ok(()),
                };

                len -= end - start;
                if len == 0 {
                    break Err(ERANGE);
                }
                start = end;
                end = (end + PAGE_SIZE).min(start + len);
            }
        }
    }
}

impl<T: Copy, D> RawReg for UserPtr<T, D> {
    fn from_raw(raw: usize) -> Self {
        UserPtr {
            addr: raw,
            _marker: PhantomData,
        }
    }

    fn into_raw(self) -> usize {
        self.addr
    }
}

impl<T: Copy, D: InPtr> UserPtr<T, D> {
    pub async fn read(&self, virt: Pin<&Virt>) -> Result<T, Error> {
        if !(self.addr as *const T).is_aligned() {
            return Err(EFAULT);
        }
        let mut dst = MaybeUninit::<T>::uninit();
        unsafe {
            let dst_addr = dst.as_mut_ptr().into();
            checked_copy(virt, self.addr.into(), dst_addr, mem::size_of::<T>()).await?;
            Ok(dst.assume_init())
        }
    }

    pub async fn read_slice(&self, virt: Pin<&Virt>, data: &mut [T]) -> Result<(), Error> {
        log::trace!(
            "UserPtr::read_slice: self = {:?}, len = {}",
            self,
            data.len()
        );

        if !(self.addr as *const T).is_aligned() {
            return Err(EFAULT);
        }
        unsafe {
            let dst = data.as_mut_ptr().into();
            checked_copy(virt, self.addr.into(), dst, mem::size_of_val(data)).await
        }
    }

    pub fn reborrow(&self) -> &UserPtr<T, In> {
        unsafe { mem::transmute(self) }
    }
}

impl<D: InPtr> UserPtr<u8, D> {
    pub async fn read_str<'a>(
        &self,
        virt: Pin<&Virt>,
        buf: &'a mut [u8],
    ) -> Result<&'a str, Error> {
        async fn inner<'a>(
            virt: Pin<&'a Virt>,
            range: Range<LAddr>,
            buf: &'a mut [u8],
        ) -> Result<Option<&'a mut [u8]>, Error> {
            let count = range.end.val() - range.start.val();
            unsafe {
                let dst = buf.as_mut_ptr().into();
                checked_copy(virt, range.start, dst, count).await?;
            }
            let has_zero = buf[..count].contains(&0);
            Ok((!has_zero).then_some(&mut buf[count..]))
        }

        self.op(virt, inner, buf.len(), &mut *buf).await?;

        let ret = CStr::from_bytes_until_nul(buf)?.to_str()?;
        Ok(ret)
    }

    pub async fn read_path<'a>(
        &self,
        virt: Pin<&Virt>,
        buf: &'a mut [u8],
    ) -> Result<&'a Path, Error> {
        let path = self.read_str(virt, buf).await?;
        let path = path.strip_prefix('/').unwrap_or(path);
        let path = path.strip_prefix('.').unwrap_or(path);
        Ok(Path::new(path))
    }
}

impl<T: Copy, D: OutPtr> UserPtr<T, D> {
    pub async fn write(&mut self, virt: Pin<&Virt>, data: T) -> Result<(), Error> {
        if !(self.addr as *const T).is_aligned() {
            return Err(EFAULT);
        }
        unsafe {
            let src = (&data as *const T).into();
            checked_copy(virt, src, self.addr.into(), mem::size_of::<T>()).await
        }
    }

    pub async fn write_slice(
        &mut self,
        virt: Pin<&Virt>,
        data: &[T],
        add_tail_zero: bool,
    ) -> Result<(), Error> {
        log::trace!(
            "UserPtr::write_slice: self = {:?}, len = {}",
            self,
            data.len()
        );

        if !(self.addr as *const T).is_aligned() {
            return Err(EFAULT);
        }
        unsafe {
            let count = mem::size_of_val(data);
            let src = data.as_ptr().into();
            checked_copy(virt, src, self.addr.into(), count).await?;
            if add_tail_zero {
                checked_zero(virt, 0, (self.addr + count).into(), mem::size_of::<T>()).await?;
            }
            Ok(())
        }
    }

    pub fn reborrow_mut(&mut self) -> &mut UserPtr<T, Out> {
        unsafe { mem::transmute(self) }
    }
}

#[derive(Debug)]
pub struct UserBuffer {
    addr: LAddr,
}

impl RawReg for UserBuffer {
    fn from_raw(addr: usize) -> Self {
        UserBuffer { addr: addr.into() }
    }

    fn into_raw(self) -> usize {
        self.addr.val()
    }
}

impl UserBuffer {
    pub async fn as_slice(&self, virt: Pin<&Virt>, len: usize) -> Result<Vec<&[u8]>, Error> {
        let paddrs = virt.commit_range(self.addr..(self.addr + len)).await?;
        Ok(paddrs
            .into_iter()
            .map(|range| unsafe { LAddr::as_slice(PAddr::range_to_laddr(range, ID_OFFSET)) })
            .collect::<Vec<_>>())
    }

    pub async fn as_mut_slice(
        &mut self,
        virt: Pin<&Virt>,
        len: usize,
    ) -> Result<Vec<&mut [u8]>, Error> {
        let paddrs = virt.commit_range(self.addr..(self.addr + len)).await?;
        Ok(paddrs
            .into_iter()
            .map(|range| unsafe { LAddr::as_mut_slice(PAddr::range_to_laddr(range, ID_OFFSET)) })
            .collect::<Vec<_>>())
    }
}

async unsafe fn checked_copy(
    virt: Pin<&Virt>,
    src: LAddr,
    dst: LAddr,
    count: usize,
) -> Result<(), Error> {
    extern "C" {
        fn _checked_copy(src: LAddr, dst: LAddr, count: usize) -> usize;
        fn _checked_ua_fault();
    }
    if src.is_null() || dst.is_null() {
        return Err(EFAULT);
    }

    let addr = match UA_FAULT.set(&(_checked_ua_fault as _), || unsafe {
        _checked_copy(src, dst, count)
    }) {
        0 => return Ok(()),
        addr => addr,
    };

    virt.commit(addr.into()).await?;
    match UA_FAULT.set(&(_checked_ua_fault as _), || unsafe {
        _checked_copy(src, dst, count)
    }) {
        0 => Ok(()),
        addr => {
            log::info!("checked copy fault at {addr:?}");
            Err(EFAULT)
        }
    }
}

async unsafe fn checked_zero(
    virt: Pin<&Virt>,
    src: u8,
    dst: LAddr,
    count: usize,
) -> Result<(), Error> {
    extern "C" {
        fn _checked_zero(src: u8, dst: LAddr, count: usize) -> usize;
        fn _checked_ua_fault();
    }
    if dst.is_null() {
        return Err(EFAULT);
    }
    let addr = match UA_FAULT.set(&(_checked_ua_fault as _), || unsafe {
        _checked_zero(src, dst, count)
    }) {
        0 => return Ok(()),
        addr => addr,
    };

    virt.commit(addr.into()).await?;
    match UA_FAULT.set(&(_checked_ua_fault as _), || unsafe {
        _checked_zero(src, dst, count)
    }) {
        0 => Ok(()),
        addr => {
            log::info!("checked zero fault at {addr:?}");
            Err(EFAULT)
        }
    }
}

scoped_thread_local!(pub static UA_FAULT: usize);
