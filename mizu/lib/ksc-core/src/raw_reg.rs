pub trait RawReg: Copy {
    fn from_raw(raw: usize) -> Self;

    fn into_raw(self) -> usize;
}

impl RawReg for () {
    fn from_raw(_: usize) {}

    fn into_raw(self) -> usize {
        0
    }
}

impl RawReg for bool {
    fn from_raw(raw: usize) -> Self {
        raw != 0
    }

    fn into_raw(self) -> usize {
        self as _
    }
}

macro_rules! impl_int {
    ($type:ident) => {
        impl RawReg for $type {
            fn from_raw(raw: usize) -> Self {
                raw as _
            }

            fn into_raw(self) -> usize {
                self as _
            }
        }
    };
}

impl_int!(i8);
impl_int!(u8);
impl_int!(i16);
impl_int!(u16);
impl_int!(i32);
impl_int!(u32);
impl_int!(i64);
impl_int!(u64);
impl_int!(isize);
impl_int!(usize);

impl<T> RawReg for *const T {
    fn from_raw(raw: usize) -> Self {
        raw as _
    }

    fn into_raw(self) -> usize {
        self as _
    }
}

impl<T> RawReg for *mut T {
    fn from_raw(raw: usize) -> Self {
        raw as _
    }

    fn into_raw(self) -> usize {
        self as _
    }
}
