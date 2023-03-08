use std::env;

fn main() {
    if env::var("TARGET").unwrap() == "riscv64imac-unknown-none-elf" {
        let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
        println!("cargo:rustc-link-arg=-T{manifest_dir}/link.ld");
    }
}
