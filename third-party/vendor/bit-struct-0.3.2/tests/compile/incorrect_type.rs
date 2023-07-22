enum EnumNoBits {
    A,
    B,
}

bit_struct::bit_struct! {
    struct Incorrect(u16) {
        a: EnumNoBits
    }
}

fn main() {}
