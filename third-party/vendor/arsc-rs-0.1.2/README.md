# Atomic Reference-Strongly-Counted pointer

A replacement to Rust's `std::sync::Arc` when `std::sync::Weak` references is unneeded. Typically used to slightly decrease memory usage in memory-constrained environments.

## Examples

Typical cloning and sending between threads.
```rust
use arsc_rs::Arsc;
use std::thread;

let a = Arsc::new(123);
let b = a.clone();
thread::spawn(move || println!("{b:?}"));
```

Using as a receiver.
```rust
use arsc_rs::Arsc;
#[derive(Debug)]
struct A(i32);

impl A {
    fn arsc_only(self: &Arsc<Self>) {
        println!("Arsc only: {self:?}")
    }
}
```

## Note

- Still need a global memory allocator. For environments where dynamic allocation is not supported, use some heapless structure instead.
- Be careful that `Arsc` is vulnerable to cyclic references! Use `Arc` if those cases are possible.
- This crate use some nightly-only features, so an up-to-date nightly toolchain is required to build this crate.