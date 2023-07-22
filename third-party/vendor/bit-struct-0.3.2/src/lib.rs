#![doc = include_str!("../README.md")]
#![no_std]

use core::{
    fmt::{Debug, Display},
    marker::PhantomData,
    ops::{
        Add, BitAnd, BitAndAssign, BitOr, BitOrAssign, BitXor, BitXorAssign, Div, Mul, Rem, Shl,
        ShlAssign, Shr, ShrAssign, Sub,
    },
};

use num_traits::{Bounded, Num, One, Zero};
/// Import serde here so we can reference it inside macros
#[doc(hidden)]
#[cfg(feature = "serde")]
pub use serde;

mod types;

pub use types::{
    i10, i11, i12, i13, i14, i15, i17, i18, i19, i2, i20, i21, i22, i23, i24, i25, i26, i27, i28,
    i29, i3, i30, i31, i33, i34, i35, i36, i37, i38, i39, i4, i40, i41, i42, i43, i44, i45, i46,
    i47, i48, i49, i5, i50, i51, i52, i53, i54, i55, i56, i57, i58, i59, i6, i60, i61, i62, i63,
    i7, i9, u1, u10, u11, u12, u13, u14, u15, u17, u18, u19, u2, u20, u21, u22, u23, u24, u25, u26,
    u27, u28, u29, u3, u30, u31, u33, u34, u35, u36, u37, u38, u39, u4, u40, u41, u42, u43, u44,
    u45, u46, u47, u48, u49, u5, u50, u51, u52, u53, u54, u55, u56, u57, u58, u59, u6, u60, u61,
    u62, u63, u7, u9,
};

/// [`UnsafeStorage`] is used to mark that there are some arbitrary invariants
/// which must be maintained in storing its inner value. Therefore, creation and
/// modifying of the inner value is an "unsafe" behavior. Although it might not
/// be unsafe in traditional Rust terms (no memory unsafety), behavior might be
/// "undefined"—or at least undocumented, because invariants are expected to be
/// upheld.
///
/// This is useful in macros which do not encapsulate their storage in modules.
/// This makes the macros for the end-user more ergonomic, as they can use the
/// macro multiple times in a single module.
#[repr(transparent)]
#[derive(Copy, Clone, PartialOrd, PartialEq, Eq, Ord, Hash)]
pub struct UnsafeStorage<T>(T);

impl<T> UnsafeStorage<T> {
    /// Create a new `UnsafeStorage` with the given inner value.
    ///
    /// # Safety
    /// - See the broader scope that this is called in and which invariants are
    ///   mentioned
    pub const unsafe fn new_unsafe(inner: T) -> Self {
        Self(inner)
    }

    /// Mutably access the value stored inside
    ///
    /// # Safety
    /// This should be a safe operation assuming that when modifying T to T',
    /// `UnsafeStorage::new_unsafe`(T') is safe
    pub unsafe fn as_ref_mut(&mut self) -> &mut T {
        &mut self.0
    }
}

impl<T> AsRef<T> for UnsafeStorage<T> {
    /// Access the value stored inside
    fn as_ref(&self) -> &T {
        &self.0
    }
}

impl<T: Copy> UnsafeStorage<T> {
    /// Access the value stored inside
    pub const fn inner(&self) -> T {
        self.0
    }
}

/// A trait which defines how many bits are needed to store a struct.
///
/// # Safety
/// Define `Num` as `{i,u}{8,16,32,64,128}`.
/// - when calling `core::mem::transmute` on `Self`, only bits [0, COUNT) can be
///   non-zero
/// - `TryFrom<Num>` produces `Some(x)` <=> `core::mem::transmute(num)` produces
///   a valid Self(x)
/// - `TryFrom<Num>` produces `None` <=> `core::mem::transmute(num)` produces an
///   invalid state for Self
pub unsafe trait BitCount {
    /// The number of bits associated with this type
    const COUNT: usize;
}

/// A type which can be a field of a `bit_struct`
pub trait FieldStorage {
    /// The type this field stores as
    type StoredType;
    /// Get the raw representation of this value
    fn inner_raw(self) -> Self::StoredType;
}

/// A conversion type for fitting the bits of one type into the bits of another
/// type
///
/// This differs from [`Into`] because the value may not be semantically the
/// same, this trait just asserts that the conversion can be done injectively.
///
/// The default implementation for our numeric types is to zero-extend the bits
/// to fit the target size.
pub trait BitsFitIn<T> {
    /// Fits `self` into the target type
    fn fit(self) -> T;
}

/// Check whether the underlying bits are valid
///
/// The type implementing this trait checks if the value stored in a bit
/// representation of type `P` is a valid representation of this type. The
/// [`enums`] macro implements this type for all of the integer-byte-width types
/// from this crate.
///
/// # Safety
///
/// The [`ValidCheck::is_valid`] function must be correctly implemented or else
/// other functions in this crate won't work correctly. Implementation of this
/// trait is preferably done by public macros in this crate, which will
/// implement it correctly.
pub unsafe trait ValidCheck<P> {
    /// Set this to true if, at compile-time, we can tell that all bit
    /// representations which contain the appropriate number of bits are valid
    /// representations of this type
    const ALWAYS_VALID: bool = false;
    /// Return whether or not the underlying bits of `P` are valid
    /// representation of this type
    fn is_valid(_input: P) -> bool {
        true
    }
}

/// A struct which allows for getting/setting a given property
pub struct GetSet<'a, P, T, const START: usize, const STOP: usize> {
    /// The referenced bitfield type.
    parent: &'a mut P,
    /// The type in the get/set operations
    _phantom: PhantomData<&'a mut T>,
}

impl<'a, P, T, const START: usize, const STOP: usize> GetSet<'a, P, T, START, STOP> {
    /// The bit offset at which this `GetSet` instance starts
    pub const fn start(&self) -> usize {
        START
    }

    /// The bit offset at which this `GetSet` instance ends
    pub const fn stop(&self) -> usize {
        STOP
    }
}

impl<
        'a,
        P: Num + Bounded + ShlAssign<usize> + ShrAssign<usize> + BitCount,
        T,
        const START: usize,
        const STOP: usize,
    > GetSet<'a, P, T, START, STOP>
{
    /// Create a new [`GetSet`]. This should be called from methods generated by
    /// the [`bit_struct`] macro
    pub fn new(parent: &'a mut P) -> Self {
        Self {
            parent,
            _phantom: PhantomData::default(),
        }
    }

    /// Get a mask of `STOP-START + 1` length. This doesn't use the shift left
    /// and subtract one trick because of the special case where `(0b1 <<
    /// (STOP - START + 1)) - 1` will cause an overflow
    // Because `GetSet` has a lot of type parameters, it's easiest to be able to invoke this method
    // directly on a value instead of having to match the type parameters.
    #[allow(clippy::unused_self)]
    fn mask(&self) -> P {
        let num_bits = P::COUNT;
        let mut max_value = P::max_value();
        let keep_bits = STOP - START + 1;

        max_value >>= num_bits - keep_bits;
        max_value
    }
}

impl<
        'a,
        P: Num
            + Shl<usize, Output = P>
            + Shr<usize, Output = P>
            + ShlAssign<usize>
            + ShrAssign<usize>
            + Bounded
            + BitAnd<Output = P>
            + Copy
            + BitCount,
        T: ValidCheck<P>,
        const START: usize,
        const STOP: usize,
    > GetSet<'a, P, T, START, STOP>
{
    /// Get the property this `GetSet` points at
    pub fn get(&self) -> T {
        let section = self.get_raw();
        // Safety:
        // This is guaranteed to be safe because the underlying storage must be bigger
        // than any fields stored within
        unsafe { core::mem::transmute_copy(&section) }
    }

    /// Returns true if the memory this `GetSet` points at is a valid
    /// representation of `T`
    pub fn is_valid(&self) -> bool {
        let section = self.get_raw();
        T::is_valid(section)
    }

    /// Get the raw bits being pointed at, without type conversion nor any form
    /// of validation
    pub fn get_raw(&self) -> P {
        let parent = *self.parent;
        let mask = self.mask();
        (parent >> START) & mask
    }
}

impl<'a, P, T, const START: usize, const STOP: usize> GetSet<'a, P, T, START, STOP>
where
    T: FieldStorage + BitsFitIn<P>,
    P: Num
        + Shl<usize, Output = P>
        + Copy
        + BitOrAssign
        + BitXorAssign
        + BitAnd<Output = P>
        + ShlAssign<usize>
        + ShrAssign<usize>
        + PartialOrd
        + Bounded
        + Sized
        + BitCount,
{
    /// Set the property in the slice being pointed to by this `GetSet`
    pub fn set(&mut self, value: T) {
        // SAFETY:
        // This is safe because we produce it from a valid value of `T`, so we meet the
        // safety condition on `set_raw`
        unsafe { self.set_raw(value.fit()) }
    }

    /// Set the field to a raw value.
    /// # Safety
    /// value must be a valid representation of the field. i.e.,
    /// `core::mem::transmute` between P and T must be defined.
    pub unsafe fn set_raw(&mut self, value: P) {
        let mask = self.mask();
        let mask_shifted = mask << START;

        // zero out parent
        *self.parent |= mask_shifted;
        *self.parent ^= mask_shifted;

        let to_set = value & mask;
        *self.parent |= to_set << START;
    }
}

/// A trait that all bit structs implement
///
/// See the [`bit_struct`] macro for more details.
pub trait BitStruct<const ALWAYS_VALID: bool> {
    /// The underlying type used to store the bit struct
    type Kind;
    /// Produce a bit struct from the given underlying storage, without checking
    /// for validity.
    ///
    /// # Safety
    ///
    /// The caller is responsible for verifying that this value is a valid value
    /// for the bit struct.
    ///
    /// If this is guaranteed to be safe (i.e. all possibly inputs for `value`
    /// are valid), then the bit struct will also implement [`BitStructExt`]
    /// which has the [`BitStructExt::exact_from`] method, that you should
    /// use instead.
    unsafe fn from_unchecked(value: Self::Kind) -> Self;
}

/// An extension trait for bit structs which can be safely made from any value
/// in their underlying storage type.
pub trait BitStructExt: BitStruct<true> {
    /// Produce a bit struct from the given underlying storage
    fn exact_from(value: Self::Kind) -> Self;
}

impl<T: BitStruct<true>> BitStructExt for T {
    fn exact_from(value: Self::Kind) -> Self {
        // SAFETY:
        // This is safe because this method only exists for bitfields for which it is
        // always safe to call `from_unchecked`
        unsafe { Self::from_unchecked(value) }
    }
}

#[doc(hidden)]
#[macro_export]
macro_rules! impl_fields {
    ($on: expr, $kind: ty =>[$($first_field_meta: meta),*], $head_field: ident, $head_actual: ty $(, [$($field_meta: meta),*], $field: ident, $actual: ty)*) => {
        $(#[$first_field_meta])*
        pub fn $head_field(&mut self) -> $crate::GetSet<'_, $kind, $head_actual, {$on - <$head_actual as $crate::BitCount>::COUNT}, {$on - 1}> {
            $crate::GetSet::new(unsafe {self.0.as_ref_mut()})
        }

        $crate::impl_fields!($on - <$head_actual as $crate::BitCount>::COUNT, $kind => $([$($field_meta),*], $field, $actual),*);
    };
    ($on: expr, $kind: ty =>) => {};
}

/// Helper macro
#[doc(hidden)]
#[macro_export]
macro_rules! bit_struct_impl {
    (
        $(#[$meta: meta])*
        $struct_vis: vis struct $name: ident ($kind: ty) {
        $(
            $(#[$field_meta: meta])*
            $field: ident: $actual: ty
        ),* $(,)?
        }
    ) => {

        impl $name {

            /// Creates an empty struct. This may or may not be valid
            pub unsafe fn empty() -> Self {
                unsafe { Self::from_unchecked(<$kind as $crate::BitStructZero>::bs_zero()) }
            }

            #[doc = concat!("Returns a valid representation for [`", stringify!($name), "`] where all values are")]
            /// the defaults
            ///
            /// This is different than [`Self::default()`], because the actual default implementation
            /// might not be composed of only the defaults of the given fields.
            pub fn of_defaults() -> Self {
                let mut res = unsafe { Self::from_unchecked(<$kind as $crate::BitStructZero>::bs_zero()) };
                $(
                    res.$field().set(Default::default());
                )*
                res
            }
        }

        impl ::core::fmt::Debug for $name {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> core::fmt::Result {
                let mut copied = *self;
                f.debug_struct(stringify!($name))
                    $(
                        .field(stringify!($field), &copied.$field().get())
                    )*
                    .finish()
            }
        }
    };
}

/// `serde` feature is not provided, so don't implement it
#[doc(hidden)]
#[macro_export]
#[cfg(not(feature = "serde"))]
macro_rules! bit_struct_serde_impl {
    (
        $(#[$meta:meta])*
        $struct_vis:vis struct
        $name:ident($kind:ty) { $($(#[$field_meta:meta])* $field:ident : $actual:ty),* $(,)? }
    ) => {};
}
/// `serde` feature is provided, so implement it
#[doc(hidden)]
#[macro_export]
#[cfg(feature = "serde")]
macro_rules! bit_struct_serde_impl {
    (
        $(#[$meta:meta])*
        $struct_vis: vis struct $name: ident ($kind: ty) {
        $(
            $(#[$field_meta:meta])*
            $field: ident: $actual: ty
        ),* $(,)?
        }
    ) => {
        #[allow(clippy::used_underscore_binding)]
        impl $crate::serde::Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error> where S: $crate::serde::Serializer {
                use $crate::serde::ser::SerializeStruct;

                let mut v = *self;

                let mut serializer = serializer.serialize_struct(
                    stringify!($name),
                    $crate::count_idents!( 0, [$( $field ),*] ),
                )?;
                $(
                    serializer.serialize_field(
                        stringify!($field),
                        &v.$field().get()
                    )?;
                )*
                serializer.end()
            }
        }

        #[allow(clippy::used_underscore_binding)]
        impl<'de> $crate::serde::Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error> where D: $crate::serde::Deserializer<'de> {

                use $crate::serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
                use ::core::fmt;

                const FIELDS: &'static [&'static str] = &[ $( stringify!( $field ) ),* ];

                #[allow(non_camel_case_types)]
                enum Fields { $( $field ),* }
                impl<'de> Deserialize<'de> for Fields {
                    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                        struct FieldVisitor;
                        impl<'de> Visitor<'de> for FieldVisitor {
                            type Value = Fields;

                            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                                f.write_str(stringify!( $( $field ),* ))
                            }

                            fn visit_str<E: de::Error>(self, value: &str) -> Result<Fields, E> {
                                match value {
                                    $( stringify!( $field ) => Ok(Fields::$field), )*
                                    _ => Err(de::Error::unknown_field(value, FIELDS)),
                                }
                            }
                        }

                        deserializer.deserialize_identifier(FieldVisitor)
                    }
                }

                struct BitStructVisitor;
                impl<'de> Visitor<'de> for BitStructVisitor {
                    type Value = $name;

                    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                        f.write_str(concat!("struct ", stringify!($name)))
                    }

                    fn visit_map<V: MapAccess<'de>>(self, mut map: V) -> Result<$name, V::Error> {
                        $( let mut $field: Option<$actual> = None; )*
                        while let Some(key) = map.next_key::<Fields>()? {
                            match key {
                                $( Fields::$field => {
                                    if $field.is_some() {
                                        return Err(de::Error::duplicate_field(stringify!($field)));
                                    }
                                    $field = Some(map.next_value()?);
                                },)*
                            }
                        }
                        $(
                            let $field = $field.ok_or_else(|| de::Error::missing_field(stringify!($field)))?;
                        )*
                        Ok($name::new( $( $field ),* ))
                    }

                    fn visit_seq<V: SeqAccess<'de>>(self, mut seq: V) -> Result<$name, V::Error> {
                        let mut count = 0;
                        $(
                            let $field = seq.next_element()?
                                .ok_or_else(|| de::Error::invalid_length(count, &self))?;
                            count += 1;
                        )*
                        Ok($name::new( $( $field ),* ))
                    }
                }
                deserializer.deserialize_struct(stringify!($name), FIELDS, BitStructVisitor)
            }
        }
    }
}

/// A bit struct which has a zero value we can get
pub trait BitStructZero: Zero {
    /// Get a zero value for this bit struct
    fn bs_zero() -> Self {
        Self::zero()
    }
}

impl<T: Zero> BitStructZero for T {}

// the main is actually needed

#[allow(clippy::needless_doctest_main)]
/// Create a bit struct.
///
///
/// This macro can only be used once for each module.
/// This is because the macro creates sub-module to limit access to certain
/// unsafe access. In the macro, bit-structs can be defined just like a struct
/// outside of the the macro. The catch is a **base type** must be specified.
/// Valid base types are u{8,16,32,64,128}. The elements stored in the struct
/// are statically guaranteed to not exceed the number of bits in the base type.
/// This means we cannot store a `u16` in a `u8`, but it also means we cannot
/// store 9 `u1`s in a u8.
///
/// Elements start at the top of the number (for a u16 this would be the 15th
/// bit) and progress down.
///
/// # Example
/// ```
/// bit_struct::enums! {
///     /// The default value for each enum is always the first
///     pub ThreeVariants { Zero, One, Two }
///
///     /// This is syntax to set the default value to Cat
///     pub Animal(Cat) { Cow, Bird, Cat, Dog }
///
///     pub Color { Orange, Red, Blue, Yellow, Green }
/// }
///
/// bit_struct::bit_struct! {
///     /// We can write documentation for the struct here.
///     struct BitStruct1 (u16){
///         /// a 1 bit element. This is stored in u16[15]
///         a: bit_struct::u1,
///
///         /// This is calculated to take up 2 bits. This is stored in u16[13..=14]
///         variant: ThreeVariants,
///
///         /// This also takes 2 bits. This is stored in u16[11..=12]
///         animal: Animal,
///
///         /// This takes up 3 bits. This is stored u16[8..=10]
///         color: Color,
///     }
///
///     struct BitStruct2(u32) {
///         /// We could implement for this too. Note, this does not have a default
///         a_color: Color,
///         b: bit_struct::u3,
///     }
/// }
///
/// fn main() {
///     use std::convert::TryFrom;
///     let mut bit_struct: BitStruct1 = BitStruct1::of_defaults();
///
///     assert_eq!(bit_struct.a().start(), 15);
///     assert_eq!(bit_struct.a().stop(), 15);
///
///     assert_eq!(bit_struct.color().start(), 8);
///     assert_eq!(bit_struct.color().stop(), 10);
///
///     assert_eq!(
///         format!("{:?}", bit_struct),
///         "BitStruct1 { a: 0, variant: Zero, animal: Cat, color: Orange }"
///     );
///     assert_eq!(bit_struct.raw(), 4096);
///
///     let reverse_bit_struct = BitStruct1::try_from(4096);
///     assert_eq!(
///         format!("{:?}", reverse_bit_struct),
///         "Ok(BitStruct1 { a: 0, variant: Zero, animal: Cat, color: Orange })"
///     );
///
///     // u3! macro provides a static assert that the number is not too large
///     let mut other_struct = BitStruct2::new(Color::Green, bit_struct::u3!(0b101));
///     assert_eq!(
///         format!("{:?}", other_struct),
///         "BitStruct2 { a_color: Green, b: 5 }"
///     );
///
///     assert_eq!(other_struct.a_color().get(), Color::Green);
///
///     other_struct.a_color().set(Color::Red);
///
///     assert_eq!(other_struct.a_color().get(), Color::Red);
/// }
/// ```
#[macro_export]
macro_rules! bit_struct {
    (
        $(
        $(#[$meta:meta])*
        $struct_vis: vis struct $name: ident ($kind: ty) {
        $(
            $(#[$field_meta:meta])*
            $field: ident: $actual: ty
        ),* $(,)?
        }
        )*
    ) => {
        $(
        $(#[$meta])*
        #[derive(Copy, Clone, PartialOrd, PartialEq, Eq, Ord, Hash)]
        pub struct $name($crate::UnsafeStorage<$kind>);

        $crate::bit_struct_serde_impl! {
            $(#[$meta])*
            $struct_vis struct $name ($kind) {
            $(
                $(#[$field_meta])*
                $field: $actual
            ),*
            }
        }

        #[allow(clippy::used_underscore_binding)]
        impl TryFrom<$kind> for $name {
            type Error = ();
            fn try_from(elem: $kind) -> Result<$name, ()> {
                let mut res = unsafe{Self::from_unchecked(elem)};
                $(
                    if !res.$field().is_valid() {
                        return Err(());
                    }
                )*
                Ok(res)
            }
        }

        #[allow(clippy::used_underscore_binding)]
        impl $crate::BitStruct<{$(<$actual as $crate::ValidCheck<$kind>>::ALWAYS_VALID &&)* true}> for $name {
            type Kind = $kind;

            unsafe fn from_unchecked(inner: $kind) -> Self {
               Self(unsafe {$crate::UnsafeStorage::new_unsafe(inner)})
            }
        }

        #[allow(clippy::used_underscore_binding)]
        impl $name {

            unsafe fn from_unchecked(inner: $kind) -> Self {
               Self(unsafe {$crate::UnsafeStorage::new_unsafe(inner)})
            }

            #[allow(clippy::too_many_arguments)]
            pub fn new($($field: $actual),*) -> Self {
                let mut res = unsafe { Self::from_unchecked(<$kind as $crate::BitStructZero>::bs_zero()) };
                $(
                    res.$field().set($field);
                )*
                res
            }

            pub fn raw(self) -> $kind {
                self.0.inner()
            }

            $crate::impl_fields!(<$kind as $crate::BitCount>::COUNT, $kind => $([$($field_meta),*], $field, $actual),*);
        }

        )*

        $(
        $crate::bit_struct_impl!(
        $(#[$meta])*
        $struct_vis struct $name ($kind) {
        $(
            $(#[$field_meta])*
            $field: $actual
        ),*
        }

        );
        )*
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! count_idents {
    ($on: expr, [$head: ident $(,$xs: ident)*]) => {
        $crate::count_idents!($on + 1, [$($xs),*])
    };
    ($on: expr, []) => {
        $on
    };
}

/// Returns the index of the leading 1 in `num`
///
/// Example:
/// ```
/// # use bit_struct::bits;
///
/// assert_eq!(bits(2), 2);
/// assert_eq!(bits(3), 2);
/// assert_eq!(bits(5), 3);
/// assert_eq!(bits(32), 6);
/// ```
pub const fn bits(num: usize) -> usize {
    /// Helper function for [`bits`]
    const fn helper(count: usize, on: usize) -> usize {
        // 0b11 = 3  log2_ceil(0b11) = 2 .. 2^2
        // 0b10 = 2 log2_ceil = 2 .. 2^1
        if on > 0 {
            helper(count + 1, on >> 1)
        } else {
            count
        }
    }

    helper(0, num)
}

/// `serde` feature is not provided, so don't implement it
#[doc(hidden)]
#[cfg(not(feature = "serde"))]
#[macro_export]
macro_rules! enum_serde_impl {
    ($enum_vis:vis $name:ident { $fst_field:ident $(, $field:ident)* }) => {};
}

/// `serde` feature is provided, so implement it
#[doc(hidden)]
#[cfg(feature = "serde")]
#[macro_export]
macro_rules! enum_serde_impl {
    ($name:ident { $($field:ident),* }) => {
        impl $crate::serde::Serialize for $name {
            fn serialize<S: $crate::serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                match self {
                    $(
                        Self::$field => {
                            serializer.serialize_unit_variant(
                                stringify!($name),
                                *self as u32,
                                stringify!($field),
                            )
                        },
                    )*
                }
            }
        }
        impl<'de> $crate::serde::Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error> where D: $crate::serde::Deserializer<'de> {
                use ::core::{fmt, result::Result::{self, Ok, Err}, convert::TryFrom};
                use $crate::serde::de::{Deserialize, Deserializer, EnumAccess, VariantAccess, Visitor};

                #[repr(u64)]
                enum Variants { $( $field ),* }
                impl TryFrom<u64> for Variants {
                    type Error = ();

                    fn try_from(v: u64) -> Result<Self, Self::Error> {
                        if v < $crate::count_idents!(0, [$( $field ),*]) {
                            // SAFETY:
                            // This is safe because we're converting a `u64` to a `repr(u64)`
                            // enum, and we've checked that the value is one of the variants.
                            unsafe { core::mem::transmute(v) }
                        } else {
                            Err(())
                        }
                    }
                }
                impl<'de> Deserialize<'de> for Variants {
                    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                        struct VariantsVisitor;
                        impl<'de> Visitor<'de> for VariantsVisitor {
                            type Value = Variants;
                            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                                formatter.write_str("variant identifier")
                            }

                            fn visit_u64<E: $crate::serde::de::Error>(self, value: u64) -> Result<Self::Value, E> {
                                Variants::try_from(value)
                                    .map_err(|()| $crate::serde::de::Error::invalid_value(
                                        $crate::serde::de::Unexpected::Unsigned(value),
                                        &"variant index"
                                    ))
                            }

                            fn visit_str<E: $crate::serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                                match value {
                                    $( stringify!($field) => Ok(Variants::$field), )*
                                    _ => Err($crate::serde::de::Error::unknown_variant(value, VARIANTS)),
                                }
                            }
                        }
                        deserializer.deserialize_identifier(VariantsVisitor)
                    }
                }

                struct EnumVisitor;
                impl<'de> Visitor<'de> for EnumVisitor {
                    type Value = $name;
                    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                        formatter.write_str(concat!("enum ", stringify!($name)))
                    }

                    fn visit_enum<A: EnumAccess<'de>>(self, data: A) -> Result<Self::Value, A::Error> {
                        match data.variant()? {
                            $(
                            (Variants::$field, variant) => {
                                let () = variant.unit_variant()?;
                                Ok($name::$field)
                            }
                            ),*
                        }
                    }
                }
                const VARIANTS: &'static [&'static str] = &[ $( stringify!( $field ) ),* ];
                deserializer.deserialize_enum(
                    stringify!($name),
                    VARIANTS,
                    EnumVisitor,
                )
            }
        }
    };
}

/// Helper macro
#[doc(hidden)]
#[macro_export]
macro_rules! enum_impl {
    (FROMS $name: ident: [$($kind: ty),*]) => {
        $(
        impl From<$name> for $kind {
            fn from(value: $name) -> Self {
                Self::from(value as u8)
            }
        }
        )*
    };
    (VALID_CORE $name: ident: [$($kind: ty),*]) => {
        $(
        unsafe impl $crate::ValidCheck<$kind> for $name {
            const ALWAYS_VALID: bool = <Self as $crate::ValidCheck<u8>>::ALWAYS_VALID;
            fn is_valid(value: $kind) -> bool {
                Self::is_valid(value as u8)
            }
        }
        )*
    };
    (COUNT $head:ident $(,$xs: ident)*) => {
       1 + $crate::enum_impl!(COUNT $($xs),*)
    };
    (COUNT) => {
        0
    };
    (VALID_BIT_STRUCT $name: ident: [$($kind: ty),*]) => {
        $(
        unsafe impl $crate::ValidCheck<$kind> for $name {
            const ALWAYS_VALID: bool = <Self as $crate::ValidCheck<u8>>::ALWAYS_VALID;
            fn is_valid(value: $kind) -> bool {
                let inner = value.value();
                Self::is_valid(inner as u8)
            }
        }
        )*
    };
    (BITS_FIT_IN $name: ident: [$($kind: ty),+ $(,)?]) => {
        $(
        impl $crate::BitsFitIn<$kind> for $name {
            fn fit(self) -> $kind {
                (self as u8).fit()
            }
        }
        )+
    };
    (FROM_IMPLS $name: ident) => {
        $crate::enum_impl!(VALID_CORE $name: [u16, u32, u64, u128]);
        $crate::enum_impl!(VALID_BIT_STRUCT $name: [$crate::u24, $crate::u40, $crate::u48, $crate::u56]);
        $crate::enum_impl!(FROMS $name: [u8, u16, u32, u64, u128, $crate::u24, $crate::u40, $crate::u48, $crate::u56]);
        $crate::enum_impl!(BITS_FIT_IN $name: [u8, u16, u32, u64, $crate::u24, $crate::u40, $crate::u48, $crate::u56]);

        impl $crate::FieldStorage for $name {
            type StoredType = u8;

            fn inner_raw(self) -> Self::StoredType {
                self as Self::StoredType
            }
        }

    };
    (
        $(#[$meta:meta])*
        $enum_vis: vis $name: ident($default: ident) {
            $(#[$fst_field_meta:meta])*
            $fst_field: ident
            $(,
                $(#[$field_meta:meta])*
                $field: ident
            )* $(,)?
        }
    ) => {
        #[repr(u8)]
        $(#[$meta])*
        #[derive(Copy, Clone, Debug, PartialOrd, PartialEq, Eq)]
        $enum_vis enum $name {
            $(#[$fst_field_meta])*
            $fst_field,
            $(
                $(#[$field_meta])*
                $field
            ),*
        }

        $crate::enum_serde_impl! { $name { $fst_field $(, $field)* } }

        unsafe impl $crate::BitCount for $name {
            const COUNT: usize = $crate::bits($crate::count_idents!(0, [$($field),*]));
        }

        impl $name {
            const VARIANT_COUNT: usize = $crate::enum_impl!(COUNT $fst_field $(,$field)*);
        }

        unsafe impl $crate::ValidCheck<u8> for $name {
            const ALWAYS_VALID: bool = Self::VARIANT_COUNT.count_ones() == 1;
            fn is_valid(value: u8) -> bool {
                if (value as usize) < Self::VARIANT_COUNT {
                    true
                } else {
                    false
                }
            }
        }

        $crate::enum_impl!(FROM_IMPLS $name);

        impl Default for $name {
            fn default() -> Self {
                Self::$default
            }
        }

    };


    (
        $(#[$meta:meta])*
        $enum_vis: vis $name: ident {
            $(#[$fst_field_meta:meta])*
            $fst_field: ident
            $(,
                $(#[$field_meta:meta])*
                $field: ident
            )* $(,)?
        }
    ) => {
        #[repr(u8)]
        $(#[$meta])*
        #[derive(Copy, Clone, Debug, PartialOrd, PartialEq, Eq)]
        $enum_vis enum $name {
            $(#[$fst_field_meta])*
            $fst_field,
            $(
                $(#[$field_meta])*
                $field
            ),*
        }

        $crate::enum_serde_impl! { $name { $fst_field $(, $field)* } }

        impl Default for $name {
            fn default() -> Self {
                Self::$fst_field
            }
        }

        impl $name {
            const VARIANT_COUNT: usize = $crate::enum_impl!(COUNT $fst_field $(,$field)*);
        }

        unsafe impl $crate::BitCount for $name {
            const COUNT: usize = $crate::bits($crate::count_idents!(0, [$($field),*]));
        }


        unsafe impl $crate::ValidCheck<u8> for $name {
            const ALWAYS_VALID: bool = Self::VARIANT_COUNT.count_ones() == 1;

            fn is_valid(value: u8) -> bool {
                if (value as usize) < Self::VARIANT_COUNT {
                    true
                } else {
                    false
                }
            }
        }

        $crate::enum_impl!(FROM_IMPLS $name);
    };
}

/// Create enums with trait implementations necessary for use in a `bit_struct`
/// field.
///
/// Example:
/// ```
/// # use bit_struct::enums;
///
/// enums! {
///     pub Colors { Red, Green, Blue }
///
///     Shapes { Triangle, Circle, Square }
/// }
/// ```
///
/// By default, this macro produces an impl of [`Default`] in which the first
/// field listed is made the default. However, you can also specify some other
/// variant as the default, as follows:
/// ```
/// # use bit_struct::enums;
///
/// enums! {
///     DefaultsToA { A, B, C }
///     DefaultsToB (B) { A, B, C }
/// }
///
/// assert_eq!(DefaultsToA::default(), DefaultsToA::A);
/// assert_eq!(DefaultsToB::default(), DefaultsToB::B);
/// ```
#[macro_export]
macro_rules! enums {
    (
        $(
        $(#[$meta:meta])*
        $enum_vis: vis $name: ident $(($enum_default: ident))? {

            $(#[$fst_field_meta:meta])*
            $fst_field: ident
            $(,
                $(#[$field_meta:meta])*
                $field: ident
            )* $(,)?
        }
        )+
    ) => {
        $(
        $crate::enum_impl!(
        $(#[$meta])*
        $enum_vis $name $(($enum_default))? {
            $(#[$fst_field_meta])*
            $fst_field
            $(,
                $(#[$field_meta])*
                $field
            )*
        }
        );
        )+
    }
}

/// Create a `bit_struct`
#[macro_export]
macro_rules! create {
    (
        $struct_kind: ty {
            $($field: ident: $value: expr),* $(,)?
        }
    ) => {
        {
            let mut res = <$struct_kind>::of_defaults();
            $(
                res.$field().set($value);
            )*
            res
        }
    };
}
