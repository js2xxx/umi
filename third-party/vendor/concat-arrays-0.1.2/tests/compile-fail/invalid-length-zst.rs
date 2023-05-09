use concat_arrays::concat_arrays;

fn main() {
    let x = [()];
    let y = [()];
    let _: [(); 3] = concat_arrays!(x, y);
}
