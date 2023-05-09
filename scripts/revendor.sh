#! /bin/bash

RUST_DIR="$(rustc --print=sysroot)"

cp -f "$RUST_DIR"/lib/rustlib/src/rust/Cargo.lock \
    "$RUST_DIR"/lib/rustlib/src/rust/library/test/

mkdir -p .cargo
cp -rf cargo-config/* .cargo

cp -f scripts/config.patch.toml .cargo/config.toml

rm -rf third-party/vendor

cargo update

cargo vendor third-party/vendor \
    --respect-source-config --versioned-dirs \
    -s $RUST_DIR/lib/rustlib/src/rust/library/test/Cargo.toml \
    >> .cargo/config2.toml

mv -f .cargo/config2.toml .cargo/config.toml

cat scripts/config.toml >> .cargo/config.toml

cp -rf .cargo/* cargo-config
