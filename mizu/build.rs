use std::{env, fs, path::PathBuf};

fn main() {
    if env::var("TARGET").unwrap() == "riscv64imac-unknown-none-elf" {
        let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

        // Put the linker script somewhere the linker can find it.
        fs::write(out_dir.join("memory.x"), include_bytes!("memory.x")).unwrap();
        println!("cargo:rustc-link-search={}", out_dir.display());
        println!("cargo:rerun-if-changed=memory.x");

        println!("cargo:rustc-link-arg=-Tmemory.x");
        println!("cargo:rustc-link-arg=-Tlink.x");
    }
}
