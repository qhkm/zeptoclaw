//! Smoke test for RISC-V: verifies Uuid::new_v4() doesn't SIGSEGV (GH-48).
//!
//! On riscv64gc-unknown-linux-musl, getrandom 0.4's libc wrapper resolves to
//! a null pointer in release builds. This binary exercises the exact crash path.
fn main() {
    let id = uuid::Uuid::new_v4();
    println!("uuid: {id}");
    println!("riscv64 getrandom: ok");
}
