use core::{
    ffi::CStr,
    fmt,
    marker::PhantomData,
    mem::{self, MaybeUninit},
    ops::Range,
};

use futures_util::Future;
use kmem::{Virt, VirtCommitGuard};
use ksc::{
    Error::{self, EFAULT, EINVAL, ERANGE},
    RawReg,
};
use rv39_paging::{Attr, LAddr, PAGE_MASK, PAGE_SIZE};
use scoped_tls::scoped_thread_local;
use umifs::path::Path;

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
    addr: LAddr,
    _marker: PhantomData<(T, D)>,
}

impl<T: Copy, D: PtrType> fmt::Debug for UserPtr<T, D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({}) {:?}", D::DEBUG, self.addr)
    }
}

impl<T: Copy, D> UserPtr<T, D> {
    pub fn new(addr: LAddr) -> Self {
        UserPtr {
            addr,
            _marker: PhantomData,
        }
    }

    pub fn to_futex_key(&self) -> FutexKey {
        FutexKey { addr: self.addr }
    }

    pub fn addr(&self) -> LAddr {
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

    pub fn is_null(&self) -> bool {
        self.addr.is_null()
    }

    async fn paged_op<'a, U, G, F>(
        &self,
        virt: &'a Virt,
        mut f: G,
        mut len: usize,
        mut arg: U,
    ) -> Result<U, Error>
    where
        G: FnMut(&'a Virt, Range<LAddr>, U) -> F,
        F: Future<Output = Result<(U, bool), Error>> + Send + 'a,
        U: 'a,
    {
        let mut start = self.addr.val();
        let mut end = (start + PAGE_MASK) & !PAGE_MASK;
        len *= mem::size_of::<T>();

        log::trace!("UserPtr::op at {start:#x}, len = {len}");

        if end >= start.checked_add(len).ok_or(EINVAL)? {
            log::trace!("UserPtr::op direct call");
            let (ret, cont) = f(virt, start.into()..(start + len).into(), arg).await?;
            if !cont {
                Ok(ret)
            } else {
                Err(ERANGE)
            }
        } else {
            loop {
                log::trace!("UserPtr::op part at {start:#x}..{end:#x}");
                arg = match f(virt, start.into()..end.into(), arg).await? {
                    (arg, true) => arg,
                    (arg, false) => break Ok(arg),
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
            addr: raw.into(),
            _marker: PhantomData,
        }
    }

    fn into_raw(self) -> usize {
        self.addr.val()
    }
}

impl<T: Copy, D: InPtr> UserPtr<T, D> {
    pub async fn read(&self, virt: &Virt) -> Result<T, Error> {
        if !self.addr.is_aligned() || self.addr.is_null() {
            return Err(EFAULT);
        }
        let mut dst = MaybeUninit::<T>::uninit();
        unsafe {
            let dst_addr = dst.as_mut_ptr().into();
            checked_copy(
                virt,
                self.addr,
                dst_addr,
                mem::size_of::<T>(),
                Attr::READABLE,
            )
            .await?;
            Ok(dst.assume_init())
        }
    }

    pub async fn read_slice(&self, virt: &Virt, data: &mut [T]) -> Result<(), Error> {
        log::trace!(
            "UserPtr::read_slice: self = {:?}, len = {}",
            self,
            data.len()
        );

        if !self.addr.is_aligned() || self.addr.is_null() {
            return Err(EFAULT);
        }
        unsafe {
            let dst = data.as_mut_ptr().into();
            checked_copy(virt, self.addr, dst, mem::size_of_val(data), Attr::READABLE).await
        }
    }

    pub async fn read_slice_with_zero<'a>(
        &self,
        virt: &Virt,
        buf: &'a mut [T],
    ) -> Result<&'a [T], Error>
    where
        T: Default + PartialEq + Send + fmt::Debug,
    {
        async fn inner<'a, T: Copy + Default + PartialEq + fmt::Debug>(
            virt: &'a Virt,
            range: Range<LAddr>,
            buf: &'a mut [T],
        ) -> Result<(&'a mut [T], bool), Error> {
            let count = range.end.val() - range.start.val();
            unsafe {
                let dst = buf.as_mut_ptr().into();
                checked_copy(virt, range.start, dst, count, Attr::READABLE).await?;
            }
            let has_zero = buf[..count / mem::size_of::<T>()].contains(&Default::default());
            Ok((&mut buf[count / mem::size_of::<T>()..], !has_zero))
        }

        let rest_len = self
            .paged_op(virt, inner, buf.len(), &mut *buf)
            .await?
            .len();
        let pos = buf[..(buf.len() - rest_len)]
            .iter()
            .position(|&s| s == Default::default())
            .unwrap();
        Ok(&buf[..pos])
    }

    pub fn reborrow(&self) -> &UserPtr<T, In> {
        unsafe { mem::transmute(self) }
    }
}

impl<D: InPtr> UserPtr<u8, D> {
    pub async fn read_str<'a>(&self, virt: &Virt, buf: &'a mut [u8]) -> Result<&'a str, Error> {
        async fn inner<'a>(
            virt: &'a Virt,
            range: Range<LAddr>,
            buf: &'a mut [u8],
        ) -> Result<(&'a mut [u8], bool), Error> {
            let count = range.end.val() - range.start.val();
            unsafe {
                let dst = buf.as_mut_ptr().into();
                checked_copy(virt, range.start, dst, count, Attr::READABLE).await?;
            }
            let has_zero = buf[..count].contains(&0);
            Ok((&mut buf[count..], !has_zero))
        }

        self.paged_op(virt, inner, buf.len(), &mut *buf).await?;

        let ret = CStr::from_bytes_until_nul(buf)?.to_str()?;
        Ok(ret)
    }

    pub async fn read_path<'a>(
        &self,
        virt: &Virt,
        buf: &'a mut [u8],
    ) -> Result<(&'a Path, bool), Error> {
        let path = self.read_str(virt, buf).await?;
        if let Some(rel) = path.strip_prefix('.') {
            let path = rel.strip_prefix('/').unwrap_or(rel);
            return Ok((Path::new(path), false));
        }
        Ok(match path.strip_prefix('/') {
            Some(path) => (Path::new(path), true),
            None => (Path::new(path), false),
        })
    }
}

impl<T: Copy, D: OutPtr> UserPtr<T, D> {
    pub async fn write(&mut self, virt: &Virt, data: T) -> Result<(), Error> {
        if !self.addr.is_aligned() || self.addr.is_null() {
            return Err(EFAULT);
        }
        unsafe {
            let src = (&data as *const T).into();
            checked_copy(virt, src, self.addr, mem::size_of::<T>(), Attr::WRITABLE).await
        }
    }

    pub async fn write_slice(
        &mut self,
        virt: &Virt,
        data: &[T],
        add_tail_zero: bool,
    ) -> Result<(), Error> {
        log::trace!(
            "UserPtr::write_slice: self = {:?}, len = {}",
            self,
            data.len()
        );

        if !self.addr.is_aligned() || self.addr.is_null() {
            return Err(EFAULT);
        }
        unsafe {
            let count = mem::size_of_val(data);
            let src = data.as_ptr().into();
            checked_copy(virt, src, self.addr, count, Attr::WRITABLE).await?;
            if add_tail_zero {
                checked_zero(virt, 0, self.addr + count, mem::size_of::<T>()).await?;
            }
            Ok(())
        }
    }

    pub fn reborrow_mut(&mut self) -> &mut UserPtr<T, Out> {
        unsafe { mem::transmute(self) }
    }
}

#[derive(Debug, Clone, Copy)]
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

impl Default for UserBuffer {
    fn default() -> Self {
        UserBuffer {
            addr: 0usize.into(),
        }
    }
}

impl UserBuffer {
    pub fn addr(&self) -> LAddr {
        self.addr
    }

    pub async fn commit(&self, guard: &mut VirtCommitGuard<'_>, len: usize) -> Result<(), Error> {
        guard.push(self.addr..(self.addr + len)).await
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FutexKey {
    addr: LAddr,
}

impl RawReg for FutexKey {
    fn from_raw(addr: usize) -> Self {
        FutexKey { addr: addr.into() }
    }

    fn into_raw(self) -> usize {
        self.addr.val()
    }
}

impl FutexKey {
    pub async fn load(&self, virt: &Virt) -> Result<u32, Error> {
        if !self.addr.is_aligned() || self.addr.is_null() {
            return Err(EFAULT);
        }
        unsafe { checked_load_u32(virt, self.addr) }.await
    }
}

#[inline]
async unsafe fn checked_copy(
    virt: &Virt,
    src: LAddr,
    dst: LAddr,
    count: usize,
    expect_attr: Attr,
) -> Result<(), Error> {
    extern "C" {
        fn _checked_copy(src: LAddr, dst: LAddr, count: usize) -> usize;
    }
    let op = || unsafe { _checked_copy(src, dst, count) };
    checked_op(virt, op, expect_attr).await
}

#[inline]
async unsafe fn checked_zero(virt: &Virt, src: u8, dst: LAddr, count: usize) -> Result<(), Error> {
    extern "C" {
        fn _checked_zero(src: u8, dst: LAddr, count: usize) -> usize;
    }
    let op = || unsafe { _checked_zero(src, dst, count) };
    checked_op(virt, op, Attr::WRITABLE).await
}

async unsafe fn checked_load_u32(virt: &Virt, src: LAddr) -> Result<u32, Error> {
    extern "C" {
        fn _checked_load_u32(src: LAddr, dst: &mut u32) -> usize;
    }
    let mut dst = 0;
    let op = || unsafe { _checked_load_u32(src, &mut dst) };
    checked_op(virt, op, Attr::READABLE).await?;
    Ok(dst)
}

async fn checked_op<F: FnMut() -> usize>(
    virt: &Virt,
    mut op: F,
    expect_attr: Attr,
) -> Result<(), Error> {
    extern "C" {
        fn _checked_ua_fault();
    }

    let mut last_addr = None;
    loop {
        match UA_FAULT.set(&(_checked_ua_fault as _), &mut op) {
            0 => break Ok(()),
            addr if last_addr == Some(addr) => break Err(EFAULT),
            addr => {
                virt.commit((*last_addr.insert(addr)).into(), expect_attr)
                    .await?
            }
        };
    }
}

scoped_thread_local!(pub static UA_FAULT: usize);
