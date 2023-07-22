//! New integer types used in this crate, and trait implementations for those
//! types

use super::*;
#[cfg(feature = "serde")]
use serde::{Deserializer, Serializer};

/// Assert that the given type is valid for any representation thereof
macro_rules! always_valid {
    ($($elem: ty),*) => {
        $(
        // Safety:
        // This is correct: stdlib types are always valid
        unsafe impl <P> ValidCheck<P> for $elem {
            const ALWAYS_VALID: bool = true;
        }
        )*
    };
}

/// Implement the [`BitCount`] trait easily for the built-in base types
macro_rules! bit_counts {
    ($($num: ty = $count: literal),*) => {
        $(
        // Safety:
        // This is correct for the built-in types
        unsafe impl BitCount for $num {
            const COUNT: usize = $count;
        }
        )*
    };
}

bit_counts!(u8 = 8, u16 = 16, u32 = 32, u64 = 64, u128 = 128, bool = 1);

/// Implement the [`FieldStorage`] trait for some built-in types
macro_rules! impl_field_storage {
    ($(($type:ty, $base:ty)),+ $(,)?) => {
        $(
        impl FieldStorage for $type {
            type StoredType = $base;

            fn inner_raw(self) -> Self::StoredType {
                self.into()
            }
        }
        )+
    };
}
impl_field_storage!(
    (bool, u8),
    (u8, Self),
    (u16, Self),
    (u32, Self),
    (u64, Self),
    (u128, Self),
);
macro_rules! impl_signed_field_storage {
    ($(($type:ty, $base:ty)),+ $(,)?) => {
        $(
        impl FieldStorage for $type {
            type StoredType = $base;

            fn inner_raw(self) -> Self::StoredType {
                <$base>::from_le_bytes(self.to_le_bytes())
            }
        }
        )+
    };
}
impl_signed_field_storage!((i8, u8), (i16, u16), (i32, u32), (i64, u64), (i128, u128),);

always_valid!(u8, u16, u32, u64, u128, i8, i16, i32, i64, i128, bool);
/// Create a type for representing signed integers of sizes not provided by rust
macro_rules! new_signed_types {
    (
        $($name: ident($count: literal, $inner: ty, $signed: ty)),*
    ) => {
        $(

        #[doc = concat!("An unsigned integer which contains ", stringify!($count), " bits")]
        #[allow(non_camel_case_types)]
        #[derive(Copy, Clone, Eq, PartialEq, Hash)]
        pub struct $name($inner);

        always_valid!($name);

        #[cfg(feature = "serde")]
        impl serde::Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                self.value().serialize(serializer)
            }
        }

        #[cfg(feature = "serde")]
        impl <'de> serde::Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let inner = <$signed>::deserialize(deserializer)?;
                $name::new(inner).ok_or(serde::de::Error::custom("invalid size"))
            }
        }

        impl PartialOrd for $name {
            fn partial_cmp(&self, other: &Self) -> Option<::core::cmp::Ordering> {
                self.value().partial_cmp(&other.value())
            }
        }

        impl Ord for $name {
            fn cmp(&self, other: &Self) -> ::core::cmp::Ordering {
                self.value().cmp(&other.value())
            }
        }

        impl Debug for $name {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                f.write_fmt(format_args!("{}", self.value()))
            }
        }

        impl Display for $name {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                f.write_fmt(format_args!("{}", self.value()))
            }
        }

        #[doc = concat!("Produce a value of type ", stringify!($name))]
        ///
        /// This macro checks at compile-time that it fits. To check at run-time see the
        #[doc = concat!("[`", stringify!($name), "::new`] function.")]
        #[macro_export]
        macro_rules! $name {
            ($value: expr) => {
                {
                    const VALUE: $signed = $value;
                    const _: () = assert!(VALUE <= $crate::$name::MAX, "The provided value is too large");
                    const _: () = assert!(VALUE >= $crate::$name::MIN, "The provided value is too small");
                    let res: $name = unsafe {$crate::$name::new_unchecked(VALUE)};
                    res
                }
            };
        }

        // Safety:
        // This is guaranteed to be the correct arguments
        unsafe impl BitCount for $name {
            const COUNT: usize = $count;
        }

        num_traits!($name, $signed);

        impl $name {
            /// Create a new $name from value
            /// # Safety
            /// - value must fit within the number of bits defined in the type
            pub const unsafe fn new_unchecked(value: $signed) -> Self {
                let unsigned_value = value as $inner;
                if value >= 0 {
                    Self(unsigned_value)
                } else {
                    // we can do this
                    let value = unsigned_value & Self::MAX_UNSIGNED;
                    Self(value | Self::POLARITY_FLAG)
                }
            }


            /// Create a new $name from value
            /// # Safety
            /// - value must fit within the number of bits defined in the type
            pub fn new(value: $signed) -> Option<Self> {
                if (Self::MIN..=Self::MAX).contains(&value) {
                    // SAFETY:
                    // We've just checked that this is safe to call
                    Some(unsafe {Self::new_unchecked(value)})
                } else {
                    None
                }
            }

            const POLARITY_FLAG: $inner = (1 << ($count - 1));
            const MAX_UNSIGNED: $inner = (1 << ($count-1)) - 1;
            /// The largest value this type can hold
            pub const MAX: $signed = Self::MAX_UNSIGNED as $signed;
            /// The smallest value this type can hold
            pub const MIN: $signed = -(Self::MAX_UNSIGNED as $signed) - 1;

            /// Get the value stored in here, as a signed integer
            pub const fn value(self) -> $signed {
                match self.0 >> ($count - 1) {
                    0 => self.0 as $signed,
                    _ => {
                        // 0's out negative
                        let rem = self.0 ^ Self::POLARITY_FLAG;
                        let amount = Self::MAX_UNSIGNED - rem;
                        -(amount as $signed) - 1
                    }
                }
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self(0)
            }
        }

        impl FieldStorage for $name {
            type StoredType = $inner;

            fn inner_raw(self) -> Self::StoredType {
                self.0
            }
        }
        )*
    };
}

/// Implement traits from the [`num_traits`] crate for our new number types
macro_rules! num_traits {
    ($num:ident, $super_kind:ty) => {
        impl Zero for $num {
            fn zero() -> Self {
                $num::new(0).unwrap()
            }

            fn is_zero(&self) -> bool {
                self.0 == 0
            }
        }

        impl Add for $num {
            type Output = Self;

            fn add(self, rhs: Self) -> Self::Output {
                $num::new(self.value() + rhs.value()).unwrap()
            }
        }

        impl One for $num {
            fn one() -> Self {
                $num::new(1).unwrap()
            }
        }

        impl Mul for $num {
            type Output = Self;

            fn mul(self, rhs: Self) -> Self::Output {
                $num::new(self.value() * rhs.value()).unwrap()
            }
        }

        impl Sub for $num {
            type Output = $num;

            fn sub(self, rhs: Self) -> Self::Output {
                $num::new(self.value() - rhs.value()).unwrap()
            }
        }

        impl Div for $num {
            type Output = Self;

            fn div(self, rhs: Self) -> Self::Output {
                $num::new(self.value() / rhs.value()).unwrap()
            }
        }

        impl Rem for $num {
            type Output = Self;

            fn rem(self, rhs: Self) -> Self::Output {
                $num::new(self.value() % rhs.value()).unwrap()
            }
        }

        impl Num for $num {
            type FromStrRadixErr = ();

            fn from_str_radix(str: &str, radix: u32) -> Result<Self, Self::FromStrRadixErr> {
                let parse = <$super_kind>::from_str_radix(str, radix).map_err(|_| ())?;
                $num::new(parse).ok_or(())
            }
        }

        impl ::core::str::FromStr for $num {
            type Err = <Self as Num>::FromStrRadixErr;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                <Self as Num>::from_str_radix(s, 10)
            }
        }

        impl Shr<usize> for $num {
            type Output = $num;

            fn shr(self, rhs: usize) -> Self::Output {
                $num::new(self.value() >> rhs).unwrap()
            }
        }

        impl Shl<usize> for $num {
            type Output = $num;

            fn shl(self, rhs: usize) -> Self::Output {
                $num::new(self.value() << rhs).unwrap()
            }
        }

        impl ShrAssign<usize> for $num {
            fn shr_assign(&mut self, rhs: usize) {
                let got = *self >> rhs;
                *self = got;
            }
        }

        impl ShlAssign<usize> for $num {
            fn shl_assign(&mut self, rhs: usize) {
                let got = *self << rhs;
                *self = got;
            }
        }

        impl Bounded for $num {
            fn min_value() -> Self {
                $num::new(Self::MIN).unwrap()
            }

            fn max_value() -> Self {
                $num::new(Self::MAX).unwrap()
            }
        }

        impl BitAnd for $num {
            type Output = $num;

            fn bitand(self, rhs: Self) -> Self::Output {
                $num(self.0 & rhs.0)
            }
        }

        impl BitXor for $num {
            type Output = $num;

            fn bitxor(self, rhs: Self) -> Self::Output {
                $num(self.0 ^ rhs.0)
            }
        }

        impl BitXorAssign for $num {
            fn bitxor_assign(&mut self, rhs: Self) {
                self.0 ^= rhs.0
            }
        }

        impl BitAndAssign for $num {
            fn bitand_assign(&mut self, rhs: Self) {
                self.0 &= rhs.0
            }
        }

        impl BitOr for $num {
            type Output = Self;

            fn bitor(self, rhs: Self) -> Self::Output {
                $num(self.0 | rhs.0)
            }
        }

        impl BitOrAssign for $num {
            fn bitor_assign(&mut self, rhs: Self) {
                self.0 |= rhs.0;
            }
        }
    };
}

/// Create a type for representing unsigned integers of sizes not provided by
/// rust
macro_rules! new_unsigned_types {
    (
        $($name: ident($count: literal, $inner: ty)),*
    ) => {
        $(

        #[doc = concat!("An unsigned integer which contains ", stringify!($count), " bits")]
        #[allow(non_camel_case_types)]
        #[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
        pub struct $name($inner);

        always_valid!($name);

        impl Debug for $name {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                f.write_fmt(format_args!("{}", self.0))
            }
        }

        impl Display for $name {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                f.write_fmt(format_args!("{}", self.0))
            }
        }

        #[cfg(feature = "serde")]
        impl serde::Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                self.value().serialize(serializer)
            }
        }

        #[cfg(feature = "serde")]
        impl <'de> serde::Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let inner = <$inner>::deserialize(deserializer)?;
                $name::new(inner).ok_or(serde::de::Error::custom("invalid size"))
            }
        }

        #[doc = concat!("Produce a value of type ", stringify!($name))]
        ///
        /// This macro checks at compile-time that it fits. To check at run-time see the
        #[doc = concat!("[`", stringify!($name), "::new`] function.")]
        #[macro_export]
        macro_rules! $name {
            ($value: literal) => {
                {
                    const VALUE: $inner = $value;

                    // this is always valid because we have one more bit than we need in $inner
                    // type
                    const _: () = assert!($crate::$name::MAX >= VALUE, "The provided value is too large");
                    unsafe {$crate::$name::new_unchecked(VALUE)}
                }
            };
        }


        // SAFETY:
        // This is correct (guaranteed by macro arguments)
        unsafe impl BitCount for $name {
            /// The number of bits this type takes up
            ///
            /// Note that this is the conceptual amount it needs in a bit struct, not the amount it
            /// will use as its own variable on the stack.
            const COUNT: usize = $count;
        }

        impl $name {
            /// The largest value that can be stored
            pub const MAX: $inner = (1 << ($count)) - 1;
            /// The smallest value that can be stored
            pub const MIN: $inner = 0;

            #[doc = concat!("Create a new ", stringify!($name), " from an inner value")]
            ///
            /// This method does not do any checks that the value passed is valid. To check that,
            #[doc = concat!("use the [`", stringify!($name), "::new`] function.")]
            ///
            /// # Safety
            /// The value must be valid value of the given type.
            pub const unsafe fn new_unchecked(value: $inner) -> Self {
                Self(value)
            }

            #[doc = concat!("Create a new ", stringify!($name), " from an inner value")]
            ///
            /// This method checks that the inner value is valid, and return `None` if it isn't.
            pub fn new(value: $inner) -> Option<Self> {
                if (Self::MIN..=Self::MAX).contains(&value) {
                    // SAFETY:
                    // We've checked that this is safe to do in the above `if`
                    Some(unsafe {Self::new_unchecked(value)})
                } else {
                    None
                }
            }

            /// Get the stored value
            pub const fn value(self) -> $inner {
                self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self(0)
            }
        }

        num_traits!($name, $inner);

        impl FieldStorage for $name {
            type StoredType = $inner;

            fn inner_raw(self) -> Self::StoredType {
                self.0
            }
        }
        )*
    };
}

new_signed_types!(
    i2(2, u8, i8),
    i3(3, u8, i8),
    i4(4, u8, i8),
    i5(5, u8, i8),
    i6(6, u8, i8),
    i7(7, u8, i8),
    i9(9, u16, i16),
    i10(10, u16, i16),
    i11(11, u16, i16),
    i12(12, u16, i16),
    i13(13, u16, i16),
    i14(14, u16, i16),
    i15(15, u16, i16),
    i17(17, u32, i32),
    i18(18, u32, i32),
    i19(19, u32, i32),
    i20(20, u32, i32),
    i21(21, u32, i32),
    i22(22, u32, i32),
    i23(23, u32, i32),
    i24(24, u32, i32),
    i25(25, u32, i32),
    i26(26, u32, i32),
    i27(27, u32, i32),
    i28(28, u32, i32),
    i29(29, u32, i32),
    i30(30, u32, i32),
    i31(31, u32, i32),
    i33(33, u64, i64),
    i34(34, u64, i64),
    i35(35, u64, i64),
    i36(36, u64, i64),
    i37(37, u64, i64),
    i38(38, u64, i64),
    i39(39, u64, i64),
    i40(40, u64, i64),
    i41(41, u64, i64),
    i42(42, u64, i64),
    i43(43, u64, i64),
    i44(44, u64, i64),
    i45(45, u64, i64),
    i46(46, u64, i64),
    i47(47, u64, i64),
    i48(48, u64, i64),
    i49(49, u64, i64),
    i50(50, u64, i64),
    i51(51, u64, i64),
    i52(52, u64, i64),
    i53(53, u64, i64),
    i54(54, u64, i64),
    i55(55, u64, i64),
    i56(56, u64, i64),
    i57(57, u64, i64),
    i58(58, u64, i64),
    i59(59, u64, i64),
    i60(60, u64, i64),
    i61(61, u64, i64),
    i62(62, u64, i64),
    i63(63, u64, i64)
);

new_unsigned_types!(
    u1(1, u8),
    u2(2, u8),
    u3(3, u8),
    u4(4, u8),
    u5(5, u8),
    u6(6, u8),
    u7(7, u8),
    u9(9, u16),
    u10(10, u16),
    u11(11, u16),
    u12(12, u16),
    u13(13, u16),
    u14(14, u16),
    u15(15, u16),
    u17(17, u32),
    u18(18, u32),
    u19(19, u32),
    u20(20, u32),
    u21(21, u32),
    u22(22, u32),
    u23(23, u32),
    u24(24, u32),
    u25(25, u32),
    u26(26, u32),
    u27(27, u32),
    u28(28, u32),
    u29(29, u32),
    u30(30, u32),
    u31(31, u32),
    u33(33, u64),
    u34(34, u64),
    u35(35, u64),
    u36(36, u64),
    u37(37, u64),
    u38(38, u64),
    u39(39, u64),
    u40(40, u64),
    u41(41, u64),
    u42(42, u64),
    u43(43, u64),
    u44(44, u64),
    u45(45, u64),
    u46(46, u64),
    u47(47, u64),
    u48(48, u64),
    u49(49, u64),
    u50(50, u64),
    u51(51, u64),
    u52(52, u64),
    u53(53, u64),
    u54(54, u64),
    u55(55, u64),
    u56(56, u64),
    u57(57, u64),
    u58(58, u64),
    u59(59, u64),
    u60(60, u64),
    u61(61, u64),
    u62(62, u64),
    u63(63, u64)
);

/// Implement functions for converting to/from byte arrays
///
/// Used for our numeric types that are an integer number of bytes.
macro_rules! byte_from_impls {
    ($($kind: ident: $super_kind: ty)*) => {
        $(
        impl $kind {
            /// The size of byte array equal to this value
            const ARR_SIZE: usize = <$kind>::COUNT / 8;
            /// The size of byte array equal to the underlying storage for this value
            const SUPER_BYTES: usize = ::core::mem::size_of::<$super_kind>();
            /// Convert from an array of bytes, in big-endian order
            pub fn from_be_bytes(bytes: [u8; Self::ARR_SIZE]) -> Self {
                let mut res_bytes = [0_u8; Self::SUPER_BYTES];
                for (set, &get) in res_bytes.iter_mut().rev().zip(bytes.iter().rev()) {
                    *set = get;
                }
                Self(<$super_kind>::from_be_bytes(res_bytes))
            }

            /// Convert `self` into an array of bytes, in big-endian order
            pub fn to_be_bytes(self) -> [u8; Self::ARR_SIZE] {
                let mut res = [0; Self::ARR_SIZE];
                let inner_bytes = self.0.to_be_bytes();
                for (&get, set) in inner_bytes.iter().rev().zip(res.iter_mut().rev()) {
                    *set = get;
                }
                res
            }

            /// Convert from an array of bytes, in little-endian order
            pub fn from_le_bytes(bytes: [u8; Self::ARR_SIZE]) -> Self {
                let mut res_bytes = [0_u8; Self::SUPER_BYTES];
                for (set, &get) in res_bytes.iter_mut().zip(bytes.iter()) {
                    *set = get;
                }
                Self(<$super_kind>::from_le_bytes(res_bytes))
            }

            /// Convert `self` into an array of bytes, in little-endian order
            pub fn to_le_bytes(self) -> [u8; Self::ARR_SIZE] {
                let mut res = [0; Self::ARR_SIZE];
                let inner_bytes = self.0.to_le_bytes();
                for (&get, set) in inner_bytes.iter().zip(res.iter_mut()) {
                    *set = get;
                }
                res
            }
        }

        impl From<u8> for $kind {
            fn from(byte: u8) -> Self {
                let inner = <$super_kind>::from(byte);
                $kind(inner)
            }
        }
        )*
    };
}

byte_from_impls! {
    u24: u32
    u40: u64
    u48: u64
    u56: u64
    i24: u32
    i40: u64
    i48: u64
    i56: u64
}

impl u1 {
    /// The 1-bit representation of true (1)
    pub const TRUE: Self = Self(1);
    /// The 1-bit representation of false (0)
    pub const FALSE: Self = Self(0);

    /// Get the opposite of this value (i.e. `TRUE` <--> `FALSE`)
    #[must_use]
    pub const fn toggle(self) -> Self {
        match self {
            Self::FALSE => Self::TRUE,
            _ => Self::FALSE,
        }
    }
}

/// Implement `BitsFitIn` for the given pair of types, using the given method
macro_rules! bits_fit_in_impl {
    ($basety:ty => $target:ty : from) => {
        impl BitsFitIn<$target> for $basety {
            fn fit(self) -> $target {
                self.inner_raw().into()
            }
        }
    };
    ($basety:ty => $target:ty : new_unchecked) => {
        impl BitsFitIn<$target> for $basety {
            fn fit(self) -> $target {
                // Safety:
                // The caller of this macro should only implement it with safe conversions
                unsafe { <$target>::new_unchecked(self.inner_raw().into()) }
            }
        }
    };
}

/// Implement `BitsFitIn` easily for a large number of unsigned types
macro_rules! bits_fit_in_impls {
    () => {};
    (
        // The types we generate from in this pass
        ($basety:ty: $funcname:ident, $extra_ty:ty)
        // The remaining target types
        $( ,
            ($first_target:ty: $target_funcname:ident $(, $extra_sources:ty)* $(,)?)
        )* $(,)?
    ) => {
        bits_fit_in_impl!($basety => $basety: $funcname);
        $(
            bits_fit_in_impl!($basety => $first_target: $target_funcname);
            bits_fit_in_impl!($extra_ty => $first_target: $target_funcname);
        )*
        bits_fit_in_impls!($(($first_target: $target_funcname $(, $extra_sources)*)),*);
    }
}

bits_fit_in_impls!(
    (u1: new_unchecked, bool),
    (u2: new_unchecked, i2),
    (u3: new_unchecked, i3),
    (u4: new_unchecked, i4),
    (u5: new_unchecked, i5),
    (u6: new_unchecked, i6),
    (u7: new_unchecked, i7),
    (u8: from, i8),
    (u9: new_unchecked, i9),
    (u10: new_unchecked, i10),
    (u11: new_unchecked, i11),
    (u12: new_unchecked, i12),
    (u13: new_unchecked, i13),
    (u14: new_unchecked, i14),
    (u15: new_unchecked, i15),
    (u16: from, i16),
    (u17: new_unchecked, i17),
    (u18: new_unchecked, i18),
    (u19: new_unchecked, i19),
    (u20: new_unchecked, i20),
    (u21: new_unchecked, i21),
    (u22: new_unchecked, i22),
    (u23: new_unchecked, i23),
    (u24: new_unchecked, i24),
    (u25: new_unchecked, i25),
    (u26: new_unchecked, i26),
    (u27: new_unchecked, i27),
    (u28: new_unchecked, i28),
    (u29: new_unchecked, i29),
    (u30: new_unchecked, i30),
    (u31: new_unchecked, i31),
    (u32: from, i32),
    (u33: new_unchecked, i33),
    (u34: new_unchecked, i34),
    (u35: new_unchecked, i35),
    (u36: new_unchecked, i36),
    (u37: new_unchecked, i37),
    (u38: new_unchecked, i38),
    (u39: new_unchecked, i39),
    (u40: new_unchecked, i40),
    (u41: new_unchecked, i41),
    (u42: new_unchecked, i42),
    (u43: new_unchecked, i43),
    (u44: new_unchecked, i44),
    (u45: new_unchecked, i45),
    (u46: new_unchecked, i46),
    (u47: new_unchecked, i47),
    (u48: new_unchecked, i48),
    (u49: new_unchecked, i49),
    (u50: new_unchecked, i50),
    (u51: new_unchecked, i51),
    (u52: new_unchecked, i52),
    (u53: new_unchecked, i53),
    (u54: new_unchecked, i54),
    (u55: new_unchecked, i55),
    (u56: new_unchecked, i56),
    (u57: new_unchecked, i57),
    (u58: new_unchecked, i58),
    (u59: new_unchecked, i59),
    (u60: new_unchecked, i60),
    (u61: new_unchecked, i61),
    (u62: new_unchecked, i62),
    (u63: new_unchecked, i63),
    (u64: from, i64),
);
