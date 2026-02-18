fn main() {
    // riscv64gc-unknown-linux-musl: getrandom 0.4's libc::getrandom() wrapper
    // resolves to a null pointer in release builds, causing SIGSEGV in Uuid::new_v4().
    // Force the linux_raw backend (raw asm! syscalls) for this target. (GH-48)
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if arch == "riscv64" && os == "linux" && env == "musl" {
        println!("cargo:rustc-cfg=getrandom_backend=\"linux_raw\"");
    }
}
