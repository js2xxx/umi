#![cfg(feature = "serde")]

use quickcheck::{Arbitrary, Gen};

bit_struct::enums! {
    pub Color { Orange, Red, Blue, Yellow, Green }
}

bit_struct::bit_struct! {
    struct BitStruct(u32) {
        a_color: Color,
        b: bit_struct::u3,
    }
}

impl Arbitrary for Color {
    fn arbitrary(g: &mut Gen) -> Self {
        *g.choose(&[
            Self::Orange,
            Self::Red,
            Self::Blue,
            Self::Yellow,
            Self::Green,
        ])
        .unwrap()
    }
}

impl Arbitrary for BitStruct {
    fn arbitrary(g: &mut Gen) -> Self {
        let b = *g
            .choose(&[
                bit_struct::u3!(0),
                bit_struct::u3!(1),
                bit_struct::u3!(2),
                bit_struct::u3!(3),
                bit_struct::u3!(4),
                bit_struct::u3!(5),
                bit_struct::u3!(6),
                bit_struct::u3!(7),
            ])
            .unwrap();
        Self::new(Color::arbitrary(g), b)
    }
}

#[quickcheck_macros::quickcheck]
fn test_round_trip_serialize_json_enum(color: Color) -> bool {
    use serde_json::{from_value, to_value};
    let round_trip: Color =
        from_value(to_value(color).expect("Failed to serialize")).expect("Failed to deserialize");
    round_trip == color
}

#[quickcheck_macros::quickcheck]
fn test_round_trip_serialize_postcard_enum(color: Color) -> bool {
    use postcard::{from_bytes, to_allocvec};
    let round_trip: Color = from_bytes(&to_allocvec(&color).expect("Failed to serialize"))
        .expect("Failed to deserialize");
    round_trip == color
}

#[quickcheck_macros::quickcheck]
fn test_round_trip_serialize_json_struct(bits: BitStruct) -> bool {
    use serde_json::{from_value, to_value};
    let round_trip: BitStruct =
        from_value(to_value(bits).expect("Failed to serialize")).expect("Failed to deserialize");
    round_trip == bits
}

#[quickcheck_macros::quickcheck]
fn test_round_trip_serialize_postcard_struct(bits: BitStruct) -> bool {
    use postcard::{from_bytes, to_allocvec};
    let round_trip: BitStruct = from_bytes(&to_allocvec(&bits).expect("Failed to serialize"))
        .expect("Failed to deserialize");
    round_trip == bits
}
