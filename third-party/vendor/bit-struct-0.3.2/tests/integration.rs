use bit_struct::*;
use num_traits::{Bounded, One, Zero};
use std::cmp::Ordering;

#[macro_use]
extern crate matches;

enums!(
    /// A doc comment
    pub ModeA(Two) { Zero, One, Two }
    pub ModeB(One) { Zero, One, Two }
    pub ModeC(Zero) { Zero, One, Two }
    ModeD { Zero, One, Two }
    OrderA {A, B}
    OrderB(B) {A, B}
);

bit_struct!(
    /// `Abc` struct
    struct Abc(u16){
        mode: ModeA,
        _padding: u4,
        count: u2,
    }

    struct FullCount(u16){
        count: u16,
    }

    struct NonCoreBase(u24){
        count: u16,
        next: u6,
        mode: ModeA,
    }

    struct Bools(u24){
        flag_a: bool,
        flag_b: bool,
    }
);

impl Default for Abc {
    fn default() -> Self {
        Self::of_defaults()
    }
}

#[test]
fn test_create() {
    let mut abc = create! {
        Abc {
            mode: ModeA::Two,
            count: u2!(2)
        }
    };
    assert_eq!(abc.mode().get(), ModeA::Two);
    assert_eq!(abc.count().get(), u2!(2));
}

#[test]
#[allow(clippy::assertions_on_constants)]
fn test_always_valid_enum() {
    assert!(!<ModeA as ValidCheck<u8>>::ALWAYS_VALID);
    assert!(!<ModeA as ValidCheck<u16>>::ALWAYS_VALID);
    assert!(<OrderA as ValidCheck<u8>>::ALWAYS_VALID);
    assert!(<OrderA as ValidCheck<u16>>::ALWAYS_VALID);
}

#[test]
fn test_from() {
    let mut bools = Bools::exact_from(u24!(0xFF_00_00));
    assert!(bools.flag_a().get());
    assert!(bools.flag_b().get());
}

#[test]
fn test_bools() {
    let mut bools = Bools::of_defaults();
    assert!(!bools.flag_a().get());
    assert!(!bools.flag_b().get());

    bools.flag_a().set(true);
    assert!(bools.flag_a().get());
    assert!(!bools.flag_b().get());

    bools.flag_a().set(false);
    bools.flag_b().set(true);
    assert!(!bools.flag_a().get());
    assert!(bools.flag_b().get());

    bools.flag_a().set(true);
    bools.flag_b().set(true);
    assert!(bools.flag_a().get());
    assert!(bools.flag_b().get());
}

#[test]
fn test_round_trip_bytes() {
    {
        let num = u24!(0x04D3);

        let bytes = num.to_be_bytes();
        assert_eq!(bytes, [0x0, 0x4, 0xD3]);

        let num_cloned = u24::from_be_bytes(bytes);

        assert_eq!(num, num_cloned);
    }

    {
        let num = u24!(1235);
        let bytes = num.to_le_bytes();
        let num_cloned = u24::from_le_bytes(bytes);

        assert_eq!(num, num_cloned);
    }

    for i in 0..10 {
        let res = u24::from(i).value() as u8;
        assert_eq!(i, res);
    }
}

#[test]
fn test_invalid() {
    assert!(ModeA::is_valid(0_u8));
    assert!(ModeA::is_valid(1_u8));
    assert!(ModeA::is_valid(2_u8));
    assert!(!ModeA::is_valid(3_u8));

    assert!(ModeD::is_valid(0_u8));
    assert!(ModeD::is_valid(1_u8));
    assert!(ModeD::is_valid(2_u8));
    assert!(!ModeD::is_valid(3_u8));
}

#[test]
fn test_non_core_base() {
    let mut non_core_base = NonCoreBase::new(123, u6!(13), ModeA::One);

    let count = non_core_base.count().get();
    assert_eq!(count, 123);

    let next = non_core_base.next().get();
    assert_eq!(next.value(), 13);

    let mode = non_core_base.mode().get();
    assert_eq!(mode, ModeA::One);

    let raw = non_core_base.raw();

    let circle = NonCoreBase::try_from(raw).unwrap();
    assert_eq!(circle, non_core_base);
}

#[test]
fn test_full_count() {
    let full_count = FullCount::new(124);
    assert_eq!(full_count.raw(), 124);
}

#[test]
fn test_of_defaults() {
    let full_count = FullCount::of_defaults();
    assert_eq!(full_count.raw(), 0);
}

#[test]
fn test_toggle() {
    let v = u1::TRUE;
    assert_eq!(v, u1::TRUE);
    assert_eq!(v.toggle(), u1::FALSE);
    assert_eq!(v.toggle().toggle(), u1::TRUE);
}

#[test]
fn test_enum_intos() {
    macro_rules! intos {
        ($enum_var: ty, $($kind: ty),*) => {
            $(
            assert_eq!(<$kind>::from(<$enum_var>::Zero), 0);
            assert_eq!(<$kind>::from(<$enum_var>::One), 1);
            assert_eq!(<$kind>::from(<$enum_var>::Two), 2);
            )*
        };
    }

    intos!(ModeA, u8, u16, u32, u64, u128);
    intos!(ModeD, u8, u16, u32, u64, u128);
}

#[test]
fn test_ord() {
    for a in -0xFF..0xFF {
        for b in -0xFF..0xFF {
            let a_i9 = i9::new(a).unwrap();
            let b_i9 = i9::new(b).unwrap();
            if a < b {
                assert!(a_i9 < b_i9);
                assert_eq!(a_i9.cmp(&b_i9), Ordering::Less);

                assert!(b_i9 > a_i9);
                assert_eq!(b_i9.cmp(&a_i9), Ordering::Greater);
            }
            if a > b {
                assert!(a_i9 > b_i9);
                assert_eq!(a_i9.cmp(&b_i9), Ordering::Greater);

                assert!(b_i9 < a_i9);
                assert_eq!(b_i9.cmp(&a_i9), Ordering::Less);
            }
            if a <= b {
                assert!(a_i9 <= b_i9);
                assert_matches!(a_i9.cmp(&b_i9), Ordering::Less | Ordering::Equal);

                assert!(b_i9 >= a_i9);
                assert_matches!(b_i9.cmp(&a_i9), Ordering::Greater | Ordering::Equal);
            }
            if a >= b {
                assert!(a_i9 >= b_i9);
                assert_matches!(a_i9.cmp(&b_i9), Ordering::Greater | Ordering::Equal);

                assert!(b_i9 <= a_i9);
                assert_matches!(b_i9.cmp(&a_i9), Ordering::Less | Ordering::Equal);
            }
            if a == b {
                assert_eq!(a_i9, b_i9);
                assert_matches!(a_i9.cmp(&b_i9), Ordering::Equal);

                assert_eq!(b_i9, a_i9);
                assert_matches!(b_i9.cmp(&a_i9), Ordering::Equal);
            }
        }
    }
}

#[test]
fn test_enum_defaults() {
    // default is manually set to last
    assert_eq!(ModeA::default(), ModeA::Two);
    assert_eq!(ModeB::default(), ModeB::One);
    assert_eq!(ModeC::default(), ModeC::Zero);
    assert_eq!(ModeD::default(), ModeD::Zero);

    assert_eq!(OrderA::default(), OrderA::A);
    assert_eq!(OrderB::default(), OrderB::B);
}

#[test]
fn test_bit_struct_defaults() {
    let mut abc = Abc::default();
    assert_eq!(abc.count().get(), u2!(0));
    assert_eq!(abc.mode().get(), ModeA::Two);
}

#[test]
fn test_bit_struct_debug() {
    let abc = Abc::default();
    assert_eq!(
        format!("{:?}", abc),
        "Abc { mode: Two, _padding: 0, count: 0 }"
    );
}

#[test]
fn test_bit_struct_raw_values() {
    let mut abc = Abc::default();
    abc.mode().set(ModeA::One); // 0b01
    abc.count().set(u2!(0b10));

    // 0b 0100 0010 0000 0000
    // 0x    4    2    0    0
    // 0x4200
    assert_eq!(abc.raw(), 0x4200);

    let eq_abc = unsafe { Abc::from_unchecked(0x4200) };

    assert_eq!(eq_abc.raw(), 0x4200);
    assert_eq!(eq_abc, abc);
}

#[test]
fn test_new_signed_types() {
    assert_eq!(i2::MAX, 1);
    assert_eq!(i2::max_value(), i2!(1));

    assert_eq!(i2::MIN, -2);
    assert_eq!(i2::min_value(), i2!(-2));

    assert_eq!(i2!(-2).inner_raw(), 0b10);
    assert_eq!(i2!(-1).inner_raw(), 0b11);
    assert_eq!(i2!(0).inner_raw(), 0b00);
    assert_eq!(i2!(1).inner_raw(), 0b01);

    assert_eq!(i2!(-2).value(), -2);
    assert_eq!(i2!(-1).value(), -1);
    assert_eq!(i2!(0).value(), 0);
    assert_eq!(i2!(1).value(), 1);

    assert_eq!(i3!(-4).inner_raw(), 0b100);
    assert_eq!(i3!(-3).inner_raw(), 0b101);
    assert_eq!(i3!(-2).inner_raw(), 0b110);
    assert_eq!(i3!(-1).inner_raw(), 0b111);
    assert_eq!(i3!(0).inner_raw(), 0b000);
    assert_eq!(i3!(1).inner_raw(), 0b001);
    assert_eq!(i3!(2).inner_raw(), 0b010);
    assert_eq!(i3!(3).inner_raw(), 0b011);

    assert_eq!(i3!(-4).inner_raw(), 0b100);
    assert_eq!(i3!(-3).inner_raw(), 0b101);
    assert_eq!(i3!(-2).inner_raw(), 0b110);
    assert_eq!(i3!(-1).inner_raw(), 0b111);
    assert_eq!(i3!(0).inner_raw(), 0b000);
    assert_eq!(i3!(1).inner_raw(), 0b001);
    assert_eq!(i3!(2).inner_raw(), 0b010);
    assert_eq!(i3!(3).inner_raw(), 0b011);

    assert!(i3::new(-5).is_none());
    assert!(i3::new(-4).is_some());
    assert!(i3::new(-3).is_some());
    assert!(i3::new(-2).is_some());
    assert!(i3::new(-1).is_some());
    assert!(i3::new(0).is_some());
    assert!(i3::new(1).is_some());
    assert!(i3::new(2).is_some());
    assert!(i3::new(3).is_some());
    assert!(i3::new(4).is_none());

    assert_eq!(i2::default().value(), 0);
    assert_eq!(i3::default().value(), 0);
    assert_eq!(i4::default().value(), 0);
}

fn all_i9s() -> impl Iterator<Item = i9> {
    (-0xFF..0xFF).filter_map(i9::new)
}

fn some_i9s() -> impl Iterator<Item = i9> {
    (-0xD..0xD).filter_map(i9::new)
}

#[test]
fn test_num_trait() {
    macro_rules! eq {
        ($a:expr, $b:expr) => {
            assert_eq!($a, $b.value());
        };
    }

    macro_rules! eq_assign {
        ($operation:ident, $a1:expr, $b1:expr, $a2:expr, $b2:expr) => {
            let mut temp1 = $a1;
            temp1.$operation($b1);

            let mut temp2 = $a2;
            temp2.$operation($b2);

            assert_eq!(temp1, temp2.value());
        };
    }

    assert!(i4::default().is_zero());
    assert!(!i4!(1).is_zero());

    assert!(i4!(1).is_one());
    assert!(u4!(1).is_one());
    assert!(!i4!(3).is_one());
    assert!(!u4!(0).is_one());

    assert!(u4::default().is_zero());

    use core::ops::*;

    for num in some_i9s() {
        for shift in 0..=2 {
            let actual = num.value();
            eq_assign!(shl_assign, actual, shift, num, shift);
            eq_assign!(shr_assign, actual, shift, num, shift);
        }
    }

    for num in all_i9s() {
        let actual = num.value();
        let from = format!("{}", actual);
        let a = str::parse::<i16>(&from);
        let b = str::parse::<i9>(&from);
        eq!(a.unwrap(), b.unwrap());
    }

    for (a, b) in some_i9s().zip(some_i9s()) {
        let actual_a = a.value();
        let actual_b = b.value();

        eq!(actual_a - actual_b, a - b);
        eq!(actual_a + actual_b, a + b);
        eq!(actual_a * actual_b, a * b);

        if !b.is_zero() {
            eq!(actual_a / actual_b, a / b);
            eq!(actual_a % actual_b, a % b);
        }

        eq!(actual_a | actual_b, a | b);
        eq!(actual_a & actual_b, a & b);
        eq!(actual_a ^ actual_b, a ^ b);
        eq!(actual_a ^ actual_b, a ^ b);

        eq_assign!(bitand_assign, actual_a, actual_b, a, b);
    }
}

#[test]
fn test_signed_types_formatting() {
    for elem in all_i9s() {
        let actual = elem.value();
        assert_eq!(format!("{:?}", elem), format!("{:?}", actual));
        assert_eq!(format!("{}", elem), format!("{}", actual));
    }
}

#[test]
fn test_new_unsigned_types() {
    assert_eq!(u1!(0).value(), 0b0);
    assert_eq!(u1!(1).value(), 0b1);

    assert_eq!(u1::new(0b0_u8).unwrap().value(), 0);
    assert_eq!(u1::new(0b1_u8).unwrap().value(), 1);
    assert!(u1::new(0b11_u8).is_none());

    assert_eq!(u2!(0).value(), 0b00);
    assert_eq!(u2!(1).value(), 0b01);
    assert_eq!(u2!(2).value(), 0b10);
    assert_eq!(u2!(3).value(), 0b11);
    assert_eq!(u2::new(0b0_u8).unwrap().value(), 0);
    assert_eq!(u2::new(0b1_u8).unwrap().value(), 1);
    assert_eq!(u2::new(0b10_u8).unwrap().value(), 2);
    assert_eq!(u2::new(0b11_u8).unwrap().value(), 3);
    assert!(u2::new(0b100_u8).is_none());

    assert_eq!(format!("{}", u1!(0)), "0");
    assert_eq!(format!("{}", u1!(1)), "1");

    assert_eq!(format!("{}", u2!(0)), "0");
    assert_eq!(format!("{}", u2!(1)), "1");
    assert_eq!(format!("{}", u2!(2)), "2");
    assert_eq!(format!("{}", u2!(3)), "3");
}

#[test]
fn test_valid_struct() {
    // 0b[AA]** **** **** ****
    // makes Abc valid where AA is 0b00, 0b01, 0b10
    // makes Abc invalid where AA is 0b11

    for first_bits in 0x0..0xF {
        let raw = first_bits << 12;
        let mode_a_bits = first_bits >> 2;
        let conversion = Abc::try_from(raw);
        let valid = match mode_a_bits {
            0b00 | 0b01 | 0b10 => conversion.is_ok(),
            0b11 => conversion.is_err(),
            _ => panic!("impossible"),
        };

        assert!(valid);
    }
}

#[test]
fn test_bits() {
    assert_eq!(bits(0b0), 0);

    assert_eq!(bits(0b1), 1);

    assert_eq!(bits(0b10), 2);
    assert_eq!(bits(0b11), 2);

    assert_eq!(bits(0b100), 3);
    assert_eq!(bits(0b101), 3);
    assert_eq!(bits(0b110), 3);
    assert_eq!(bits(0b111), 3);

    assert_eq!(bits(0b1000), 4);
    assert_eq!(bits(0b1001), 4);
    assert_eq!(bits(0b1010), 4);
    assert_eq!(bits(0b1011), 4);
    assert_eq!(bits(0b1100), 4);
    assert_eq!(bits(0b1101), 4);
    assert_eq!(bits(0b1110), 4);
    assert_eq!(bits(0b1111), 4);

    assert_eq!(bits(0b10000), 5);
}

#[test]
fn test_bit_struct_creation() {
    let mut abc = Abc::new(ModeA::Two, u4::default(), u2!(0b11));
    assert_eq!(abc.mode().get(), ModeA::Two);
    assert_eq!(abc._padding().get(), u4!(0));
    assert_eq!(abc.count().get(), u2!(0b11));
}

#[test]
fn fails() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/compile/*.rs");
}
