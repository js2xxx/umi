use core::{
    alloc::Layout,
    mem,
    num::NonZeroUsize,
    ops::{Add, AddAssign, Deref, DerefMut, Range, Sub, SubAssign},
    ptr::NonNull,
    slice,
};

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Default, Hash)]
#[repr(transparent)]
pub struct PAddr(usize);

impl PAddr {
    #[inline]
    pub const fn new(addr: usize) -> Self {
        PAddr(addr)
    }

    pub const fn as_non_zero(self) -> Option<NonZeroUsize> {
        NonZeroUsize::new(self.0)
    }

    #[inline]
    pub fn to_laddr(self, id_off: usize) -> LAddr {
        LAddr::from(self.0 + id_off)
    }

    pub fn in_page_offset(self) -> usize {
        self.0 & crate::PAGE_MASK
    }

    #[inline]
    pub fn val(self) -> usize {
        self.0
    }

    pub fn range_to_laddr(this: Range<Self>, id_off: usize) -> Range<LAddr> {
        this.start.to_laddr(id_off)..this.end.to_laddr(id_off)
    }
}

impl Add<usize> for PAddr {
    type Output = Self;

    fn add(self, rhs: usize) -> Self::Output {
        PAddr::new(*self + rhs)
    }
}

impl AddAssign<usize> for PAddr {
    fn add_assign(&mut self, rhs: usize) {
        **self += rhs
    }
}

impl Sub<usize> for PAddr {
    type Output = Self;

    fn sub(self, rhs: usize) -> Self::Output {
        PAddr::new(*self - rhs)
    }
}

impl SubAssign<usize> for PAddr {
    fn sub_assign(&mut self, rhs: usize) {
        **self -= rhs
    }
}

impl const Deref for PAddr {
    type Target = usize;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl const DerefMut for PAddr {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl core::fmt::Debug for PAddr {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "PAddr({:#x})", self.0)
    }
}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct LAddr(usize);

impl LAddr {
    #[inline]
    pub fn new(ptr: *mut u8) -> Self {
        LAddr(ptr as _)
    }

    #[inline]
    pub fn val(&self) -> usize {
        self.0
    }

    #[inline]
    pub fn as_non_null(self) -> Option<NonNull<u8>> {
        NonNull::new(self.0 as _)
    }

    /// # Safety
    ///
    /// `self` must be non-null.
    #[inline]
    pub unsafe fn as_non_null_unchecked(self) -> NonNull<u8> {
        NonNull::new_unchecked(self.0 as _)
    }

    /// transfer kernel va to corresponding la
    #[inline]
    pub fn to_paddr(self, id_off: usize) -> PAddr {
        PAddr(self.val() - id_off)
    }

    pub fn in_page_offset(self) -> usize {
        self.val() & crate::PAGE_MASK
    }

    pub fn to_range(self, layout: Layout) -> Range<Self> {
        self..Self::new(self.wrapping_add(layout.size()))
    }

    /// # Safety
    ///
    /// See ['slice::from_raw_parts'] for more info.
    pub unsafe fn as_slice<'a>(this: Range<Self>) -> &'a [u8] {
        unsafe { slice::from_raw_parts(*this.start, this.end.val() - this.start.val()) }
    }

    /// # Safety
    ///
    /// See ['slice::from_raw_parts_mut'] for more info.
    pub unsafe fn as_mut_slice<'a>(this: Range<Self>) -> &'a mut [u8] {
        unsafe { slice::from_raw_parts_mut(*this.start, this.end.val() - this.start.val()) }
    }
}

impl const Deref for LAddr {
    type Target = *mut u8;

    #[inline]
    fn deref(&self) -> &Self::Target {
        unsafe { mem::transmute(&self.0) }
    }
}

impl const DerefMut for LAddr {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { mem::transmute(&mut self.0) }
    }
}

impl const From<usize> for LAddr {
    #[inline]
    fn from(val: usize) -> Self {
        LAddr(val as _)
    }
}

impl const From<u64> for LAddr {
    #[inline]
    fn from(val: u64) -> Self {
        LAddr(val as _)
    }
}

impl<T> From<*const T> for LAddr {
    #[inline]
    fn from(val: *const T) -> Self {
        LAddr(val as _)
    }
}

impl<T> From<*mut T> for LAddr {
    #[inline]
    fn from(val: *mut T) -> Self {
        LAddr(val as _)
    }
}

impl<T: ?Sized> From<NonNull<T>> for LAddr {
    #[inline]
    fn from(ptr: NonNull<T>) -> Self {
        LAddr::new(ptr.as_ptr().cast())
    }
}

impl Add<usize> for LAddr {
    type Output = Self;

    fn add(self, rhs: usize) -> Self::Output {
        LAddr::from(self.val() + rhs)
    }
}

impl AddAssign<usize> for LAddr {
    fn add_assign(&mut self, rhs: usize) {
        *self = *self + rhs;
    }
}

impl Sub<usize> for LAddr {
    type Output = Self;

    fn sub(self, rhs: usize) -> Self::Output {
        LAddr::from(self.val() - rhs)
    }
}

impl SubAssign<usize> for LAddr {
    fn sub_assign(&mut self, rhs: usize) {
        *self = *self - rhs;
    }
}

impl core::fmt::Debug for LAddr {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "LAddr({:#x})", self.0)
    }
}
