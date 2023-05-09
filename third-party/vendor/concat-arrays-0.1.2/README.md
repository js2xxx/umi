# `concat-arrays`: a rust macro for concatenating fixed-size arrays

This crate defines `concat_arrays!`, a macro that allows you to concatenate arrays.

### Example:

```rust
use concat_arrays::concat_arrays;

fn main() {
    let x = [0];
    let y = [1, 2];
    let z = [3, 4, 5];
    let concatenated = concat_arrays!(x, y, z);
    assert_eq!(concatenated, [0, 1, 2, 3, 4, 5]);
}
```

### Limitations

Due to limitations in rust `concat_arrays!` can't tell the compiler what the
length of the returned array is. As such, the length needs to be inferable
from the surrounding context. For example, in the example above the length is
inferred by the call to `assert_eq!`. It is safe to mis-specify the length
however since you'll get a compilation error rather than broken code.

### Credits

Inspiration for how to implement this was taken largely from the
[`const-concat` crate](https://github.com/Vurich/const-concat) (which
implements compile-time array concatenation).

