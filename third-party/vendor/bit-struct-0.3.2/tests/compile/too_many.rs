bit_struct::bit_struct! {
    struct TooMany(u16) {
        a: u8,
        b: u8,
        c: bit_struct::u1
    }
}

fn main() {}