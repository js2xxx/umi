use std::{env, fs, path::PathBuf};

fn main() {
    if env::var("TARGET").unwrap() == "riscv64imac-unknown-none-elf" {
        let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
        let out_dir = env::var("OUT_DIR").unwrap();
        let ldscript = fs::read_to_string(PathBuf::from(manifest_dir).join("link.ld")).unwrap();
        let new = ldscript.replace("KERNEL_START", &config::KERNEL_START.to_string());
        let dest = PathBuf::from(out_dir).join("link.ld");
        fs::write(&dest, new).unwrap();
        println!("cargo:rustc-link-arg=-T{}", dest.display());
    }
}
