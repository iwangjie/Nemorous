# Rust desktop entry points

The root crate contains Nemorous and the macOS entry point. The Linux and Windows manifests link the same library to their respective NSH desktop backend. Their locks are independent so a macOS build does not resolve GTK or Win32 desktop dependencies.

Use the pinned NSH v0.6.1 source at `build/nsh-v0.6.1` (tag commit `2f51d8611677c8a6bbad937b39756e8d0cc45350`). Use the platform SDK and Rust 1.99 or later. The NSH Linux backend requires GTK 4.14 or later and its development libraries.

```sh
git clone --depth 1 --branch v0.6.1 https://github.com/longbridge/nsh.git build/nsh-v0.6.1
git -C build/nsh-v0.6.1 rev-parse HEAD

# macOS
cargo build --locked --release --bin nemorous
scripts/package-macos-app.sh build/out target/release/nemorous

# Linux, on a GTK 4.14+ development host
cargo build --locked --release --manifest-path platforms/linux/Cargo.toml

# Windows, in the Rust toolchain's native build environment
cargo build --locked --release --manifest-path platforms/windows/Cargo.toml
```

Use an independent `NEMOROUS_CONFIG` file for compatibility checks and a copied Git workspace for write tests.
