use alloc::vec::Vec;
use core::{
    ffi::CStr,
    marker::PhantomData,
    mem::{self, MaybeUninit},
    ops::Range,
    pin::Pin,
};

use arsc_rs::Arsc;
use kmem::Virt;
use ksc::{
    Error::{self, EFAULT},
    RawReg,
};
use rv39_paging::{LAddr, PAddr, CANONICAL_PREFIX, ID_OFFSET};
use scoped_tls::scoped_thread_local;
use umifs::path::Path;

use crate::rxx::KERNEL_PAGES;

const USER_RANGE: Range<usize> = 0x1000..((!CANONICAL_PREFIX) + 1);

pub fn new_virt() -> Pin<Arsc<Virt>> {
    Virt::new(USER_RANGE.start.into()..USER_RANGE.end.into(), KERNEL_PAGES)
}

pub trait InPtr {}
pub trait OutPtr {}
pub enum In {}
pub enum InOut {}
pub enum Out {}

impl InPtr for In {}
impl InPtr for InOut {}
impl OutPtr for Out {}
impl OutPtr for InOut {}

#[derive(Debug)]
pub struct UserPtr<T: Copy, D> {
    addr: usize,
    _marker: PhantomData<(T, D)>,
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
    pub fn read(&self) -> Result<T, Error> {
        if !(self.addr as *const T).is_aligned() {
            return Err(EFAULT);
        }
        let mut dst = MaybeUninit::<T>::uninit();
        unsafe {
            checked_copy(self.addr as _, dst.as_mut_ptr().cast(), mem::size_of::<T>())?;
            Ok(dst.assume_init())
        }
    }

    pub fn read_slice(&self, data: &mut [T]) -> Result<(), Error> {
        if !(self.addr as *const T).is_aligned() {
            return Err(EFAULT);
        }
        unsafe {
            checked_copy(
                self.addr as _,
                data.as_mut_ptr().cast(),
                mem::size_of_val(data),
            )
        }
    }

    pub fn reborrow(&self) -> &UserPtr<T, In> {
        unsafe { mem::transmute(self) }
    }
}

impl<D: InPtr> UserPtr<u8, D> {
    pub fn read_path<'a>(&self, buf: &'a mut [u8]) -> Result<&'a Path, Error> {
        self.read_slice(buf)?;
        let path = CStr::from_bytes_until_nul(buf)?.to_str()?;
        Ok(Path::new(path))
    }
}

impl<T: Copy, D: OutPtr> UserPtr<T, D> {
    pub fn write(&mut self, data: T) -> Result<(), Error> {
        if !(self.addr as *const T).is_aligned() {
            return Err(EFAULT);
        }
        unsafe {
            checked_copy(
                (&data as *const T).cast(),
                self.addr as _,
                mem::size_of::<T>(),
            )
        }
    }

    pub fn write_slice(&mut self, data: &[T], add_tail_zero: bool) -> Result<(), Error> {
        if !(self.addr as *const T).is_aligned() {
            return Err(EFAULT);
        }
        unsafe {
            let count = mem::size_of_val(data);
            checked_copy(data.as_ptr().cast(), self.addr as _, count)?;
            if add_tail_zero {
                checked_zero(0, (self.addr + count) as _, mem::size_of::<T>())?;
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

unsafe fn checked_copy(src: *const u8, dst: *const u8, count: usize) -> Result<(), Error> {
    extern "C" {
        fn _checked_copy(src: *const u8, dst: *const u8, count: usize) -> usize;
        fn _checked_ua_fault();
    }
    if src.is_null() || dst.is_null() {
        return Err(EFAULT);
    }
    let ret = UA_FAULT.set(&(_checked_ua_fault as _), || unsafe {
        _checked_copy(src, dst, count)
    });
    if ret == 0 {
        Ok(())
    } else {
        log::info!("checked copy fault at {ret:?}");
        Err(EFAULT)
    }
}

unsafe fn checked_zero(src: u8, dst: *const u8, count: usize) -> Result<(), Error> {
    extern "C" {
        fn _checked_zero(src: u8, dst: *const u8, count: usize) -> usize;
        fn _checked_ua_fault();
    }
    if dst.is_null() {
        return Err(EFAULT);
    }
    let ret = UA_FAULT.set(&(_checked_ua_fault as _), || unsafe {
        _checked_zero(src, dst, count)
    });
    if ret == 0 {
        Ok(())
    } else {
        log::info!("checked copy fault at {ret:?}");
        Err(EFAULT)
    }
}

scoped_thread_local!(pub static UA_FAULT: usize);
