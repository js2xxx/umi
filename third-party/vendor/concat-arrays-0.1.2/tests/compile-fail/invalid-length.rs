use concat_arrays::concat_arrays;

fn main() {
    let x = [0u32];
    let y = [1u32];
    let _: [u32; 3] = concat_arrays!(x, y);
}
